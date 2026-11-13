//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! The server state machine, transport-free. `main.rs` owns the socket, the
//! tests own a queue. Ported from RTCW-MP sv_main.c / sv_client.c; reply
//! strings come from docs/research/cod11-server-handshake.md.

use crate::client::{
    Client, ClientState, CmdKind, PacketCommand, Queued, QueuedCmd, sanitize_name,
};
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
mod cod1_frame;
mod cod1_game;
mod cod1_think;
use cod1_game::{Cod1Game, Cod1Module, FrameOut, Lend};
use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
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
use vcod_common::net::protocol::{CS_SERVERINFO, PROTOCOL_V1, Protocol};
use vcod_common::net::{com_hash_key, flags, info_value_for_key, snapshot};
use vcod_common::pmove::FallHeights;

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
/// How long the script's brush links hold still before the bots' graph
/// is rebuilt over them (`Server::relink_nav`).
const NAV_RELINK_MS: i32 = 2000;
/// A retrieval objective as the bots see it, with its carrier's slot.
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
    fn new(fs: &vcod_common::pk3::Pk3Fs, feed: i32, proto: &Protocol) -> Self {
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
    sim.shellshock = rt.client_shellshock(slot);
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
fn apply_weapon_ops(clients: &mut [Option<Client>], rt: &mut script::ScriptRuntime) {
    for (slot, op) in rt.take_weapon_ops() {
        if let Some(c) = clients.get_mut(slot).and_then(Option::as_mut)
            && let Some(sim) = c.sim.as_mut()
        {
            apply_weapon_op(sim, op);
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
        let Some(c) = clients.get_mut(slot).and_then(Option::as_mut) else {
            continue;
        };
        let st = c.last_processed_st;
        let Some(sim) = c.sim.as_mut() else {
            continue;
        };
        apply_sim_op(sim, rt, slot, op, anims, weapons, rng, (now_ms, st));
    }
}

/// One op of [`apply_sim_ops`]. `cmd_ms` is the client's last processed
/// cmd's serverTime, the clock its anim timers run on.
#[allow(clippy::too_many_arguments)]
fn apply_sim_op(
    sim: &mut ClientSim,
    rt: &mut script::ScriptRuntime,
    slot: usize,
    op: crate::game::host::SimOp,
    anims: Option<&vcod_common::animtree::PlayerAnims>,
    weapons: &crate::weapons::WeaponTable,
    rng: &mut u64,
    (now_ms, cmd_ms): (i32, i32),
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
        SimOp::Mount {
            turret,
            stance,
            view,
        } => {
            let rule = crate::game::turret::AimRule::of(rt.host.defs.proto);
            crate::game::turret::mount_sim(sim, rule, turret, stance, view);
        }
        SimOp::WeaponDisabled(on) => sim.ps.weapon_disabled = on,
        SimOp::ViewKick { force, from } => sim.view_kick(force, from),
        SimOp::VehiclePain { damage, from } => sim.vehicle_pain(damage, from),
        SimOp::Damaged {
            attacker,
            damage,
            dir,
            ..
        } => {
            if let Some(a) = attacker.filter(|&a| a != slot && damage > 0) {
                sim.last_hit = Some((a, now_ms, dir));
            }
            let index = sim.ps.weapon as usize;
            let inputs = anims.map(|anims| crate::spectate::AnimInputs {
                anims,
                weapon: crate::items::item_name(weapons.list(), index).unwrap_or_default(),
                weapon_class: weapons.class(index),
            });
            sim.take_damage(&op, inputs.as_ref(), rng, (now_ms, cmd_ms));
        }
    }
}

/// An attack's effects in the order it raised them: an impact goes on the
/// wire and a hit runs the damage callback there and then, so the flesh pair
/// a leg's callback raises numbers below the next leg's wall impact.
fn apply_effects(rt: &mut script::ScriptRuntime, effects: Vec<Effect>) {
    for e in effects {
        match e {
            Effect::Impact(te) => rt.push_temp_entity(te),
            Effect::Hit(h) => rt.deliver_hits(vec![h]),
            Effect::EntityHit(h) => rt.deliver_entity_hit(&h),
            Effect::TriggerSweep(t) => rt.trigger_sweep(&t),
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
    apply_effects(rt, effects);
    mirror_weapons(clients, rt);
    apply_weapon_ops(clients, rt);
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
/// The seat changes the host made this cmd, onto the riders' sims
/// (`crate::game::vehicle::seat::SeatOp`), and the script's copy of where
/// each one now stands.
fn apply_seat_ops(clients: &mut [Option<Client>], rt: &mut script::ScriptRuntime) {
    use crate::game::vehicle::seat::SeatOp;
    for op in std::mem::take(&mut rt.host.vehicles.seat_ops) {
        let slot = match op {
            SeatOp::Enter { slot, .. } | SeatOp::Exit { slot, .. } => slot,
        };
        let Some(sim) = clients
            .get_mut(slot)
            .and_then(Option::as_mut)
            .and_then(|c| c.sim.as_mut())
        else {
            continue;
        };
        match op {
            SeatOp::Enter {
                vehicle,
                seat,
                kind,
                origin,
                view,
                angles,
                link_offset,
                ..
            } => sim.enter_seat(
                crate::spectate::Seat {
                    vehicle,
                    pos: seat,
                    kind,
                    // `VEH_LinkPlayer` leaves `vehMotion` as the last ride
                    // or the spawn's memset left it; the end frame sets it.
                    motion: sim.veh_motion,
                    angles,
                    hull_view: true,
                    overheated: false,
                    link_offset,
                },
                origin,
                view,
            ),
            SeatOp::Exit {
                origin, velocity, ..
            } => sim.leave_seat(origin, velocity),
        }
        rt.set_client_origin(slot, sim.origin());
    }
}

/// What [`vehicle_fire`] traces and delivers with.
struct VehicleFireCtx<'a> {
    world: Option<&'a World>,
    hitlocs: &'a crate::game::combat::HitLocTable,
    fs: Option<&'a vcod_common::pk3::Pk3Fs>,
    anims: Option<&'a vcod_common::animtree::PlayerAnims>,
    rigs: &'a mut crate::game::hitrig::HitRigs,
    weapons: &'a crate::weapons::WeaponTable,
    rng: &'a mut u64,
    now_ms: i32,
}

/// The rounds `fireTurret`, `fireAltTurret` and `fireGunner` asked for
/// (`crate::game::vehicle::fire`): aimed off the posed vehicle, traced from
/// its muzzle against the bodies, charged to the rider with the vehicle as
/// the inflictor. A vehicle round leaves no impact of its own: the client
/// traces it (event parm bit 7).
/// `GameHost::dobj_posed`, which an engine trace stamps its bodies in
/// (`combat::DobjStamps`); empty on a host with no script.
fn dobj_stamps(
    script: Option<&mut script::ScriptRuntime>,
) -> &mut [crate::game::combat::DobjStamps] {
    match script {
        Some(rt) => &mut rt.host.dobj_posed,
        None => &mut [],
    }
}

fn vehicle_fire(
    clients: &mut [Option<Client>],
    rt: &mut script::ScriptRuntime,
    ctx: VehicleFireCtx,
) {
    let ops = std::mem::take(&mut rt.host.vehicles.fire_ops);
    if ops.is_empty() {
        return;
    }
    let VehicleFireCtx {
        world,
        hitlocs,
        fs,
        anims,
        rigs,
        weapons,
        rng,
        now_ms,
    } = ctx;
    for (id, gun) in ops {
        let Some(shot) = rt.with_host(|h, cx| crate::game::vehicle::fire::prepare(h, cx, id, gun))
        else {
            continue;
        };
        // The fire call spawned a shell gun's round itself
        // (`vehicle::fire::launch_shell`).
        if shot.projectile {
            continue;
        }
        let name = crate::items::item_name(rt.host.defs, shot.weapon).unwrap_or_default();
        let falloff = rt
            .host
            .weapons
            .get(shot.weapon)
            .map(crate::game::combat::Falloff::of);
        // `CMD_VEH_FireTurret` and kin pass the vehicle (0x8ee02): the
        // round skips it and its riders, owned by it.
        let pass = crate::game::combat::Pass {
            ent: id.0,
            owner: rt
                .host
                .vehicles
                .live
                .get(&id)
                .and_then(|v| v.seats[1])
                .map(|s| s as u32),
        };
        let models = rt.shot_models();
        let effects = {
            let sims: Vec<(usize, &ClientSim)> = clients
                .iter()
                .enumerate()
                .filter_map(|(i, c)| Some((i, c.as_ref()?.sim.as_ref()?)))
                .collect();
            let mut bones = crate::game::combat::BoneTraceCtx::of(
                fs,
                anims,
                &mut *rigs,
                now_ms,
                &mut rt.host.dobj_posed,
                vcod_common::patch::Arith::for_protocol(rt.host.defs.proto),
            );
            crate::game::combat::bullet_fire_to(
                shot.attacker,
                pass,
                shot.muzzle,
                shot.end,
                shot.dir,
                shot.damage,
                shot.rifle_bullet,
                falloff,
                name,
                &sims,
                &rt.host.missiles.shot_targets(),
                world.map(|w| &w.collision),
                &models,
                hitlocs,
                bones.as_mut(),
            )
        };
        let level_ms = rt.host.level_time_ms;
        for &id in &effects.struck {
            rt.host.missiles.kill(id, level_ms);
        }
        let effects = effects.effects;
        let effects: Vec<Effect> = effects
            .into_iter()
            .filter_map(|e| match e {
                Effect::Hit(mut h) => {
                    h.inflictor = Some(id);
                    Some(Effect::Hit(h))
                }
                Effect::EntityHit(mut h) => {
                    h.inflictor = Some(id);
                    Some(Effect::EntityHit(h))
                }
                Effect::TriggerSweep(t) => Some(Effect::TriggerSweep(t)),
                Effect::Impact(_) => None,
            })
            .collect();
        let struck = effects.iter().any(|e| matches!(e, Effect::Hit(_)));
        rt.with_host(|h, _| crate::game::vehicle::fire::landed(h, &shot, struck));
        deliver_turret_rounds(
            clients,
            rt,
            effects,
            shot.attacker,
            anims,
            weapons,
            rng,
            now_ms,
        );
    }
}

/// `G_VehicleClientThink` (`crate::game::vehicle::think::run_driven`),
/// handed each rider's last cmd and view; then what this frame's thinks,
/// the undriven ones' in the entity pass included, left for the riders:
/// dead riders out, the hull's angles for the next cmds' clamps,
/// `vehMotion`, the hull-view bit and the gunner's overheat.
fn vehicle_frame(clients: &mut [Option<Client>], rt: &mut script::ScriptRuntime) {
    if rt.host.vehicles.live.is_empty() {
        return;
    }
    let sims: Vec<(usize, &crate::spectate::ClientSim)> = clients
        .iter()
        .enumerate()
        .filter_map(|(slot, c)| Some((slot, c.as_ref()?.sim.as_ref()?)))
        .collect();
    let inputs = crate::game::vehicle::think::rider_inputs(&rt.host, &sims);
    rt.vehicle_deaths();
    rt.with_host(|host, cx| crate::game::vehicle::think::run_driven(host, cx, &inputs));
    rt.vehicle_deaths();
    apply_seat_ops(clients, rt);
    crush(clients, rt);
    for (slot, c) in clients.iter_mut().enumerate() {
        let Some(c) = c.as_mut() else { continue };
        let Some(seat) = c.sim.as_mut().and_then(|s| s.seat.as_mut()) else {
            continue;
        };
        let Some(v) = rt
            .host
            .vehicles
            .live
            .iter()
            .find(|(id, _)| id.0 == seat.vehicle)
            .map(|(_, v)| v)
        else {
            continue;
        };
        seat.angles = v.pose.angles;
        seat.motion = crate::game::vehicle::seat::motion(v.forward_speed);
        let motion = seat.motion;
        // The driver's think sets the bit, and clears it on a tank or
        // under `upmove` (0x87cd3).
        if seat.pos == 1 {
            seat.hull_view = v.hull_view;
        }
        seat.overheated = rt.host.vehicles.overheated_gunners.contains(&slot);
        if let Some(sim) = c.sim.as_mut() {
            sim.veh_motion = motion;
        }
    }
}

/// `VEH_PlayerCollision` (game.mp.uo 0x8213f) for every body a vehicle's
/// probes struck this frame: knocked away along the vehicle's motion, then
/// [`run_over`].
fn crush(clients: &mut [Option<Client>], rt: &mut script::ScriptRuntime) {
    let now = rt.host.level_time_ms;
    for (veh, strike) in std::mem::take(&mut rt.host.vehicles.strikes) {
        let slot = strike.entity as usize;
        let Some(sim) = clients
            .get_mut(slot)
            .and_then(Option::as_mut)
            .and_then(|c| c.sim.as_mut())
        else {
            continue;
        };
        if sim.seat.is_some() || sim.dead {
            continue;
        }
        let Some(push) = crate::game::vehicle::physics::player_push(
            strike.vehicle_origin,
            strike.vehicle_velocity,
            sim.ps.origin,
            sim.ps.velocity,
        ) else {
            continue;
        };
        sim.ps.velocity = push.velocity;
        let prone = sim.ps.stance == vcod_common::pmove::Stance::Prone;
        run_over(rt, slot, veh, push, prone, sim.ps.origin, now);
    }
}

/// `VEH_PlayerCollision`'s tail once the push is dealt: at most every
/// 500 ms the body is run over for `150 * min(speed / 400, 1)`, 0 when the
/// vehicle moves away from him (`VEH_PlayerDamage`), then, unless he lies
/// prone (`pm_flags` 1), the `vehicle_hit_person` alias at `at`, `1500 (1 -
/// f)` up, broadcast (`svFlags |= 8`). The 500 ms runs off
/// `client+0x4728`, zero from the spawn, which every strike past it
/// restarts, a 0-damage one too (coduo-vehicles.md 13).
fn run_over(
    rt: &mut script::ScriptRuntime,
    slot: usize,
    veh: vcod_gsc::EntId,
    push: crate::game::vehicle::physics::PlayerPush,
    prone: bool,
    at: glam::Vec3,
    now: i32,
) {
    const CRUSH_GAP_MS: i32 = 500;
    let last = rt.host.vehicles.crushed.get(&slot).copied().unwrap_or(0);
    if last >= now - CRUSH_GAP_MS {
        return;
    }
    rt.host.vehicles.crushed.insert(slot, now);
    let Some(hit_person) = rt
        .host
        .vehicles
        .live
        .get(&veh)
        .map(|v| rt.host.vehicles.info_of(v).hit_person_index)
    else {
        return;
    };
    vehicle_player_damage(rt, slot, veh, push.damage, at);
    if !prone && hit_person != 0 {
        rt.with_host(|host, cx| {
            host.add_temp_entity(
                cx,
                crate::game::temp_entity::TempEntity {
                    event: crate::game::script::EV_SOUND_ALIAS,
                    parm: hit_person,
                    surf_type: 0,
                    other: 0,
                    attacker: 0,
                    weapon: 0,
                    client_num: 0,
                    scale: 0,
                    origin: push.sound.to_array(),
                    scope: crate::game::temp_entity::Scope::Broadcast,
                    quake: None,
                    origin2: None,
                },
            )
        });
    }
}

/// `VEH_PlayerDamage(player, vehicle, damage)` (game.mp.uo 0x91ffd):
/// `G_Damage` with the vehicle as the inflictor and its `r.ownerNum` (the
/// driver, 1023 without one: the vehicle itself) as the attacker, no
/// direction, the body's `r.currentOrigin` `at` as the point, dflags 0 and
/// `MOD_CRUSH_JEEP` off a four-wheel (`info+0x40` 1), `MOD_CRUSH_TANK`
/// else.
fn vehicle_player_damage(
    rt: &mut script::ScriptRuntime,
    slot: usize,
    veh: vcod_gsc::EntId,
    damage: i32,
    at: glam::Vec3,
) {
    let Some(v) = rt.host.vehicles.live.get(&veh) else {
        return;
    };
    let four_wheel =
        rt.host.vehicles.info_of(v).kind == crate::game::vehicle::info::KIND_FOUR_WHEEL;
    let mod_ = if four_wheel {
        "MOD_CRUSH_JEEP"
    } else {
        "MOD_CRUSH_TANK"
    };
    let weapon = crate::items::item_name(rt.host.defs, v.wire.weapon as usize)
        .unwrap_or("none")
        .to_string();
    match v.seats[1] {
        Some(driver) => {
            let hit = crate::game::combat::Hit {
                victim: slot,
                attacker: driver,
                inflictor: Some(veh),
                damage,
                dflags: 0,
                mod_,
                weapon,
                point: at.into(),
                dir: [0.0; 3],
                hitloc: "none",
            };
            rt.deliver_hits(vec![hit]);
        }
        None => rt.deliver_unowned_hit(
            slot,
            [veh, veh],
            damage,
            0,
            mod_,
            &weapon,
            Some(at.into()),
            Some([0.0; 3]),
        ),
    }
}

/// `G_PlayerVehiclePositionAndBlend` (game.mp.uo 0x9048f) at the end of a
/// rider's `ClientEndFrame`: `ps.origin` and `r.currentOrigin` at its seat
/// tag of the vehicle as posed now, shifted by the seat's offset, and
/// turned with the tag (`r.currentAngles`, the frame a trace poses the body
/// in).
fn place_rider(rt: &mut script::ScriptRuntime, slot: usize, sim: &mut ClientSim) {
    let Some(seat) = sim.seat else {
        return;
    };
    let Some((at, steer)) = rt.with_host(|host, cx| {
        let id = host.ents.handle(seat.vehicle)?;
        let at = crate::game::vehicle::seat::posed_as(host, cx, id, true)?.seat(seat.pos)?;
        Some((at, host.vehicles.live.get(&id)?.wire.leanf))
    }) else {
        return;
    };
    sim.seat_steer = steer;
    sim.ps.origin = at.0;
    sim.place_seat_angles(vcod_common::turretpose::quat_angles(at.1));
    rt.set_client_origin(slot, at.0.to_array());
}

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
            sim.spectate_flags = 0;
            sim.follow = Default::default();
            return;
        }
        follow::SessionState::Intermission => {
            let sim = sim_mut(clients, slot);
            sim.own_view = false;
            sim.spectate_flags = 0;
            sim.follow = Default::default();
            return;
        }
        follow::SessionState::Spectator => {}
    }
    sim_mut(clients, slot).own_view = false;
    // 1.5's `allowSpectateTeam` rules (1.5 game.mp 0x39edd): a follow of a
    // team the mask refuses fails, and without free flight a spectator
    // following nobody cycles onto someone. A 1.1 script never sets the mask.
    let mask = session.no_spectate;
    let teams: Vec<i32> = if mask == 0 {
        Vec::new()
    } else {
        (0..clients.len()).map(|t| rt.client_team(t)).collect()
    };
    let allowed = |t: usize| mask == 0 || follow::can_spectate(mask, teams[t]);
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
            if !allowed(t) {
                source = Source::None;
            }
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
    if target.is_none() && !follow::can_spectate(mask, follow::FREELOOK) {
        target = follow::cycle(None, 1, clients.len(), |t| {
            followable(clients, t) && allowed(t)
        });
    }
    if source == Source::None
        && let Some(t) = target
        && allowed(t)
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
    let can = |team| follow::can_spectate(mask, team);
    match copied {
        Some(copied) => {
            let sim = sim_mut(clients, slot);
            sim.follow = follow::Follow {
                target,
                on: true,
                forced: forced >= 0,
                copied: Some(copied),
            };
            sim.spectate_flags = if forced < 0 && can(follow::FREELOOK) {
                flags::PMF_MAY_FREE
            } else {
                0
            };
        }
        None => {
            let sim = sim_mut(clients, slot);
            sim.stop_following(collision);
            sim.spectate_flags =
                if can(script::TEAM_NONE) || can(script::TEAM_AXIS) || can(script::TEAM_ALLIES) {
                    flags::PMF_MAY_CYCLE
                } else {
                    0
                };
            if session.spectator_client != -1 {
                rt.set_client_spectator_client(slot, -1);
            }
        }
    }
}

/// `ClientDisconnect`'s pass over the spectators following `gone`
/// (0x42b25..0x42b5b): each cycles on to the next client under `cycle`
/// (1.1), or stops.
fn pass_followers_on(
    clients: &mut [Option<Client>],
    gone: usize,
    mut rt: Option<&mut script::ScriptRuntime>,
    collision: Option<&vcod_common::collision::CollisionWorld>,
    cycle: bool,
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
        let next = (cycle && session.spectator_client < 0)
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
            ping: c.link.ping,
            bot: c.is_bot,
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
    sim.max_speed = rt.host.client_speed(slot);
    sim.gravity = rt.host.client_gravity();
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
    rt.set_client_height(slot, sim.box_height());
    // What `dropItem` hands the dropped weapon, and where it starts.
    rt.set_client_ammo(slot, sim.ps.ammo, sim.ps.ammoclip);
    rt.set_client_dobj(slot, sim.dobj(slot));
}

/// What a callback queued for `slot`, applied to its sim before its next cmd:
/// retail's script writes the playerstate there and then. `at` is the frame
/// clock and the client's last processed cmd's serverTime.
#[allow(clippy::too_many_arguments)]
fn apply_callback_ops(
    rt: &mut script::ScriptRuntime,
    sim: &mut ClientSim,
    slot: usize,
    anims: Option<&vcod_common::animtree::PlayerAnims>,
    weapons: &crate::weapons::WeaponTable,
    rng: &mut u64,
    at: (i32, i32),
) {
    let (weapon_ops, sim_ops) = rt.take_ops_of(slot);
    for op in weapon_ops {
        apply_weapon_op(sim, op);
    }
    for op in sim_ops {
        apply_sim_op(sim, rt, slot, op, anims, weapons, rng, at);
    }
    mirror_weapons_of(sim, rt, slot);
    mirror_vitals_of(sim, rt, slot);
}

/// `G_RunClient` (0x40660) for a linked client on its turn in the entity
/// pass: its origin is the parent's plus the offset it linked at, turned by
/// the parent's angles (`G_SetFixedLink`'s mode 2), and a held walk input
/// moves it not at all; then the relink (0x40713). The parent has had its
/// own turn (movers doc, section 13). Its velocity is whatever it linked
/// with: retail's plant capture holds 184,27 across the abort's two linked
/// seconds and 0 under 92 forward cmds (object-model doc 23.2). A parent
/// that is gone releases the link: `sd.gsc`'s plant success never unlinks,
/// the bombzone's `delete()` is what frees the record. This tick's per-cmd
/// touch passes already ran at the last frame's anchor, as retail's do.
fn run_client_link(rt: &mut script::ScriptRuntime, slot: usize, sim: &mut ClientSim) {
    if let (None, Some(seat)) = (sim.link_to, sim.seat) {
        run_rider_link(rt, slot, sim, seat);
        return;
    }
    let Some(link) = sim.link_to else {
        return;
    };
    match rt.link_anchor(link.parent) {
        Some((p, angles)) => {
            let [f, l, u] = vcod_common::x87::angles_to_axis(angles.into());
            let [x, y, z] = link.offset;
            sim.ps.origin = glam::Vec3::from(p) + f * x + l * y + u * z;
            sim.ps.velocity = link.velocity.into();
            rt.set_client_origin(slot, sim.origin());
            link_client(rt, slot, sim);
        }
        None => sim.link_to = None,
    }
}

/// `G_RunClient` (game.mp.uo 0x41dfc) on a seated rider: `G_SetFixedLink`
/// mode 2 puts `r.currentOrigin` and `ps.origin` at the seat tag of the
/// vehicle as it stands, shifted by the link's own offset, and links him
/// there unsnapped. His end frame puts `ps.origin` and `r.currentOrigin` back on the
/// seat (coduo-vehicles.md 13, "The tank driver's link offset").
fn run_rider_link(
    rt: &mut script::ScriptRuntime,
    slot: usize,
    sim: &mut ClientSim,
    seat: crate::spectate::Seat,
) {
    let Some(at) = rt.with_host(|host, cx| {
        let id = host.ents.handle(seat.vehicle)?;
        let posed = crate::game::vehicle::seat::posed(host, cx, id)?;
        let (p, r) = posed.tag(vcod_common::vehiclepose::seat_tag(seat.pos)?)?;
        Some(p + r * glam::Vec3::from(seat.link_offset))
    }) else {
        return;
    };
    sim.seat_linked_at(at);
    rt.set_client_origin(slot, at.to_array());
    link_client(rt, slot, sim);
}

/// A brush model mover's push on its turn in the entity pass: it carries its
/// riders and shoves the bodies and items in its way, or holds a frame when
/// a body fits nowhere (`G_MoverTeam`; movers doc, section 12).
fn push_mover(
    clients: &mut [Option<Client>],
    rt: &mut script::ScriptRuntime,
    step: &crate::game::mover::Step,
    collision: &vcod_common::collision::CollisionWorld,
) {
    let mut sims: Vec<(usize, &mut ClientSim)> = clients
        .iter_mut()
        .enumerate()
        .filter_map(|(i, c)| Some((i, c.as_mut()?.sim.as_mut()?)))
        .collect();
    let before: Vec<glam::Vec3> = sims.iter().map(|(_, s)| s.ps.origin).collect();
    rt.push_mover(step, &mut sims, collision);
    for ((slot, sim), was) in sims.iter().zip(before) {
        if sim.ps.origin != was {
            rt.set_client_origin(*slot, sim.origin());
        }
    }
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
/// the scripts (handshake doc, "Console commands over rcon"), the game
/// module's name and date being 1.1's or 1.5b's (its startup log). The team
/// count is `G_FindTeams` over `team` keys, which no stock map carries.
fn restart_game_banner(p: &Protocol) -> String {
    let (name, date) = match p.transport.version {
        6 => ("Call of Duty", "Nov 15 2004"),
        22 => ("CoD:United Offensive", "Feb 10 2005"),
        _ => ("main", "Nov 13 2003"),
    };
    format!(
        "==== RestartGame ====\n\
         ------- Game Initialization -------\n\
         gamename: {name}\n\
         gamedate: {date}\n\
         0 teams with 0 entities\n\
         -----------------------------------\n"
    )
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
type Opener = dyn Fn(&str) -> Result<Vec<u8>, crate::download::Refusal>;

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

/// The line of a connectionless packet as `SV_ConnectionlessPacket`
/// (cod_lnxded 0x808c63c) reads it: `MSG_ReadStringLine` stops at a NUL, a
/// newline or 1023 bytes and turns `%` and every byte past 127 into `.`
/// (docs/research/cod11-server-handshake.md, "The challenge key").
fn oob_line(raw: &[u8]) -> String {
    raw.get(4..)
        .unwrap_or_default()
        .iter()
        .take_while(|&&b| b != 0 && b != b'\n')
        .take(1023)
        .map(|&b| {
            if b == b'%' || b > 127 {
                '.'
            } else {
                char::from(b)
            }
        })
        .collect()
}

/// `Cmd_Argv(n)` of a connectionless packet: its line through
/// `Cmd_TokenizeString`.
fn oob_argv(raw: &[u8], n: usize) -> String {
    crate::game::say::tokenize(&oob_line(raw))
        .into_iter()
        .nth(n)
        .unwrap_or_default()
}

/// The last script menu `Cmd_MenuResponse_f` (0x486d8) will look up, and the
/// last `GScr_GetScriptMenuIndex` (0x5c73c) will hand out: both walk
/// `CsRange::Menu`'s 32 slots.
const MAX_MENUS: i64 = 31;

/// UO's `Cmd_FlameDamageClient_f` (game.mp.uo 0x4ee2c) on `fdc`'s
/// arguments: exactly one, read into a 4-byte buffer (so its first three
/// characters) and `atoi`'d, then kept when it lies in `0..=maxclients`
/// (`level+0x1e4`, `g_maxclients`). A bad one leaves the last report alone
/// (coduo-diff.md 21).
fn fdc_inflictor(args: &str, max_clients: usize) -> Option<u32> {
    let mut words = args.split_whitespace();
    let (Some(word), None) = (words.next(), words.next()) else {
        return None;
    };
    let head: String = word.chars().take(3).collect();
    u32::try_from(crate::game::builtins::cvar::atoi(&head))
        .ok()
        .filter(|&n| n as usize <= max_clients)
}

/// `Cmd_MenuResponse_f` (0x486d8) on `mr <serverId> <menuIndex> <response>`,
/// tokenized as `Cmd_TokenizeString` does (quotes group). The menu and the
/// response the `menuresponse` notify carries, or `None` for a stale
/// serverId, the one shape retail drops (gsc language doc 6):
///
/// - any argument count but four is the menu `""` and the response `"bad"`,
///   which is what a client's `mr <index> noop` and `mr <index> bad` reach;
/// - an index `strtol` reads past 31 (unsigned, so a negative one too) names
///   the menu by its own text;
/// - otherwise the menu is the slot's configstring, empty if unset.
pub(crate) fn parse_menu_response(cmd: &str, server_id: i32) -> Option<(MenuRef, String)> {
    let args = crate::game::say::tokenize(cmd);
    let [_, sid, index, response] = args.as_slice() else {
        return Some((MenuRef::Name(String::new()), "bad".to_string()));
    };
    if strtol(sid) != i64::from(server_id) {
        return None;
    }
    let menu = match strtol(index) {
        i @ 0..=MAX_MENUS => MenuRef::Index(i as usize),
        _ => MenuRef::Name(index.clone()),
    };
    Some((menu, response.clone()))
}

/// glibc `strtol(s, NULL, 10)` on the i386 `long`: leading whitespace, a
/// sign, the digits that follow, saturated; 0 when there are none.
fn strtol(s: &str) -> i64 {
    let s = s.trim_start();
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut v: i64 = 0;
    for b in digits.bytes().take_while(u8::is_ascii_digit) {
        v = (v * 10 + i64::from(b - b'0')).min(i64::from(i32::MAX) + 1);
    }
    let v = if neg { -v } else { v };
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX))
}

pub use vcod_common::net::protocol::install_protocol;

/// [`Server::fog_cull`] off the fields, for a caller holding the script.
fn fog_cull_of(p: &Protocol, configstrings: &[String]) -> Option<f32> {
    (!p.is_v1())
        .then(|| crate::world::fog_cull(configstrings.get(p.cs.fog).map_or("", String::as_str)))
        .flatten()
}

/// `ClientEndFrame`'s call of `G_GetNonPVSFriendlyInfo` for `slot`
/// (`crate::compass`): `None` when its session takes no end frame that
/// makes the call (spectator, intermission), else the packed teammate, its
/// slot and its ping bit, or `Some(None)` for nobody. The viewer needs a
/// team other than none or spectator; a candidate is a playing client on
/// it whose entity is not in a snapshot from the viewer's leaned eye.
/// `broadcast` is [`script::ScriptRuntime::in_snapshot_broadcast`] by slot.
#[allow(clippy::too_many_arguments)]
fn compass_friend(
    clients: &[Option<Client>],
    slot: usize,
    sessions: &[Option<follow::SessionState>],
    teams: &[i32],
    broadcast: &[bool],
    cull: Option<(&vcod_common::bsp::Visibility, Option<f32>)>,
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
        let (vis, fog) = cull?;
        let e = other.to_entity(p, n, c.last_processed_st);
        if crate::world::in_snapshot(vis, eye, &e, broadcast[n], time, p, fog) {
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

/// `SV_WriteDownloadToClient`'s blocks for `c`'s transfer, if it has one:
/// as many as the rate carries in a snapshot interval, or the www redirect.
/// A refusal ends it.
fn write_download(
    c: &mut Client,
    w: &mut MsgWriter,
    now: i32,
    rate: i32,
    open: &impl Fn(&str) -> Result<Vec<u8>, crate::download::Refusal>,
    www: Option<&crate::download::WwwConfig>,
) {
    let budget = crate::download::blocks_per_message(rate, c.snapshot_msec());
    let wants_www = c.wants_www();
    if let Some(dl) = c.download.as_mut()
        && !dl.write(w, now, budget, open, www, wants_www, &mut c.www)
    {
        c.download = None;
    }
}

/// A zombie's message (`SV_SendClientSnapshot` 0x808f844): its unacked
/// server commands, then the ring slot the new sequence lands on.
/// `SV_BuildClientSnapshot` (0x808f130) zeroes that slot's entity and client
/// counts for a zombie and copies no playerstate, and
/// `SV_WriteSnapshotToClient` (0x808dee0) deltas only for an active client,
/// so the snapshot is the playerstate of 32 messages back, written from
/// null, with no entities and no clients (handshake doc, "Zombie slots").
fn zombie_message(
    c: &mut Client,
    sv_time_ms: i32,
    snap_flags: u32,
    proto: &'static Protocol,
    huff: &Huffman,
) -> Vec<Vec<u8>> {
    let mut w = MsgWriter::new(huff);
    write_pending_commands(&mut w, &c.link.netchan, c.link.reliable_ack);
    c.link.reliable_sent = c.link.netchan.reliable_sequence as i32;
    let message_num = c.link.netchan.outgoing_sequence;
    let ps = c.link.frames[message_num as usize % crate::client::SV_PACKET_BACKUP]
        .as_ref()
        .map_or_else(|| msg::PlayerState::null(proto), |f| f.ps.clone());
    let frame = snapshot::Snapshot {
        server_time: sv_time_ms,
        message_num,
        delta_num: -1,
        snap_flags,
        ps,
        entities: Default::default(),
        clients: Default::default(),
        valid: true,
    };
    proto.write_snapshot(&mut w, None, &frame, frame.head(None), &HashMap::new());
    c.link.unsent.clear();
    c.link
        .netchan
        .transmit(c.link.last_client_command, &w.into_ops(), huff)
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
    write_pending_commands(&mut w, &c.link.netchan, c.link.reliable_ack);
    c.link.reliable_sent = c.link.netchan.reliable_sequence as i32;
    let message_num = c.link.netchan.outgoing_sequence;
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
        proto.write_snapshot(&mut w, base, &frame, frame.head(base), baselines);
        c.record_frame(frame);
    }
    c.link.unsent.clear();
    c.link
        .netchan
        .transmit(c.link.last_client_command, &w.into_ops(), huff)
}

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

    /// Test-facing: each client slot's playing spawns, in order, go to these
    /// `(origin, angles)` instead of the script's random pick (`None` keeps
    /// the pick), across every later level. A gate replays a retail
    /// capture's picks with it: retail seeds the script's draws from the
    /// wall clock (map-cycle doc 4.8).
    pub fn pin_spawns(&mut self, pins: Vec<Vec<crate::game::host::SpawnPin>>) {
        let queues = pins.into_iter().map(Into::into).collect();
        let pins = Rc::new(std::cell::RefCell::new(queues));
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.spawn_pins = Some(pins.clone());
        }
        self.game.module.spawn_pins = Some(pins);
    }

    /// Test-facing: `StuckInClient` draws its jitter from glibc's `rand()`
    /// seeded `seed`, past `skip` draws. A gate replays a retail capture's
    /// pushes with the seed recovered off it: retail seeds `rand()` from
    /// the clock and draws only the jitter after the joins (player-clip
    /// doc 13.4).
    pub fn pin_stuck_rand(&mut self, seed: u32, skip: usize) {
        let mut g = vcod_common::rng::GlibcRand::new(seed);
        for _ in 0..skip {
            g.next_u31();
        }
        self.game.stuck_rand = Some(g);
    }

    /// `new` with the generator seeded from `seed`; the binary passes the
    /// wall clock.
    pub fn with_seed(mut cfg: ServerConfig, now: Instant, seed: u64) -> Self {
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
            limiter: RateLimiter::new(now),
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
        // `--bots` without `--bots-shoot` keeps its peaceful bots.
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

    /// The snapshot cull's fog distance, squared: 1.5's and UO's, never
    /// 1.1's (`crate::world::fog_cull`).
    fn fog_cull(&self) -> Option<f32> {
        fog_cull_of(self.game.proto, &self.core.configstrings)
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

    /// `connect` re-parses `raw` because its body is compressed; the browser
    /// queries tokenize its line (`oob_argv`). The queries are rate limited
    /// before anything is looked at; a limited request is dropped silently.
    fn handle_oob(&mut self, from: SocketAddr, cmd: &str, raw: &[u8], now: Instant, read: Instant) {
        match cmd {
            "getinfo" | "getstatus" | "getchallenge" => {
                if self.game.limiter.reply_limited(from.ip(), now) {
                    log::debug!("{from}: {cmd} rate limited");
                    return;
                }
                match cmd {
                    "getinfo" => crate::engine::answer_status(
                        self,
                        from,
                        crate::engine::StatusQuery::Info,
                        &oob_argv(raw, 1),
                    ),
                    "getstatus" => crate::engine::answer_status(
                        self,
                        from,
                        crate::engine::StatusQuery::Status,
                        &oob_argv(raw, 1),
                    ),
                    _ => self.svc_get_challenge(from, now),
                }
            }
            "connect" => self.svc_direct_connect(from, raw, now),
            // Rate limited by its own server-wide window, not the buckets,
            // on the clock at the handling.
            "rcon" => self.svc_remote_command(from, raw, read, now),
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
        match self.game.module.script.as_ref() {
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

    /// A cvar read as `->integer`.
    fn live_int_cvar(&self, name: &str) -> i32 {
        crate::game::builtins::cvar::atoi(&self.live_cvar(name, "0"))
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
    /// rest. A challenge too long to fit gets no reply: retail's
    /// `Info_SetValueForKey` drops the level or crashes there.
    pub(super) fn svc_info(&mut self, challenge: &str) -> Option<String> {
        if self.v15() {
            return self.svc_info_v6(challenge);
        }
        let private = self.private_clients();
        let public_clients = self
            .core
            .clients
            .iter()
            .skip(private.max(0) as usize)
            .flatten()
            .count();
        let mut pairs = vec![
            ("challenge", challenge.to_string()),
            ("protocol", self.game.proto.transport.version.to_string()),
            (
                "hostname",
                self.live_cvar("sv_hostname", &self.game.cfg.hostname),
            ),
            ("mapname", self.game.cfg.map.clone()),
            ("clients", public_clients.to_string()),
            (
                "sv_maxclients",
                (self.game.cfg.max_clients as i32 - private).to_string(),
            ),
            ("gametype", self.live_gametype()),
            ("pure", u8::from(self.game.paks.pure).to_string()),
        ];
        // `->integer`, each key only when non-zero (0x808c318, 0x808c344).
        for (key, cvar) in [("minPing", "sv_minPing"), ("maxPing", "sv_maxPing")] {
            let v = self.live_int_cvar(cvar);
            if v != 0 {
                pairs.push((key, v.to_string()));
            }
        }
        // An empty `fs_game` reads as no key, as `Info_SetValueForKey` has it.
        pairs.push(("game", self.live_cvar("fs_game", "")));
        pairs.push((
            "sv_allowAnonymous",
            self.live_int_cvar("sv_allowAnonymous").to_string(),
        ));
        pairs.push(("pswrd", self.pswrd().to_string()));
        let mut i = Info::new();
        self.info_pairs(&mut i, pairs)
            .then(|| format!("infoResponse\n{i}"))
    }

    /// 1.5b's `SVC_Info` (cod_lnxded 1.5b 0x8092a74,
    /// docs/research/cod15-protocol-diff.md, "Server side"): `clients`
    /// counts every slot and `sv_maxclients` gives back the private slots
    /// in use, the zero-valued keys are left out, and five keys follow
    /// `pswrd`. A pair that does not fit is left out and the rest still go.
    fn svc_info_v6(&mut self, challenge: &str) -> Option<String> {
        let private = self.private_clients().max(0) as usize;
        let in_private = self.core.clients.iter().take(private).flatten().count() as i32;
        let clients = self.client_count() as i32;
        let max = self.game.cfg.max_clients as i32 - (private as i32 - in_private);
        let mut i = Info::new();
        i.set_value_for_key("challenge", challenge);
        i.set_value_for_key("protocol", self.game.proto.transport.version);
        i.set_value_for_key(
            "hostname",
            self.live_cvar("sv_hostname", &self.game.cfg.hostname),
        );
        i.set_value_for_key("mapname", &self.game.cfg.map);
        if clients != 0 {
            i.set_value_for_key("clients", clients);
        }
        if max > 0 {
            i.set_value_for_key("sv_maxclients", max);
        }
        i.set_value_for_key("gametype", self.live_gametype());
        if self.game.paks.pure {
            i.set_value_for_key("pure", 1);
        }
        let int = |sv: &Self, cvar: &str| sv.live_int_cvar(cvar);
        for (key, cvar) in [("minPing", "sv_minPing"), ("maxPing", "sv_maxPing")] {
            let v = int(self, cvar);
            if v != 0 {
                i.set_value_for_key(key, v);
            }
        }
        let game = self.live_cvar("fs_game", "");
        if !game.is_empty() {
            i.set_value_for_key("game", &game);
        }
        let anon = int(self, "sv_allowAnonymous");
        if anon != 0 {
            i.set_value_for_key("sv_allowAnonymous", anon);
        }
        if self.pswrd() != 0 {
            i.set_value_for_key("pswrd", 1);
        }
        // The string, when its `atoi` is non-zero; UO adds its vehicle
        // switches (coduo_lnxded 0x809392e).
        let uo: &[(&str, &str)] = if self.game.proto.is_uo() {
            &[("jps", "scr_allow_jeeps"), ("tnk", "scr_allow_tanks")]
        } else {
            &[]
        };
        for &(key, cvar) in [
            ("ff", "scr_friendlyfire"),
            ("kc", "scr_killcam"),
            ("timeoutsAllowed", "g_timeoutsallowed"),
        ]
        .iter()
        .chain(uo)
        {
            let v = self.live_cvar(cvar, "0");
            if crate::game::builtins::cvar::atoi(&v) != 0 {
                i.set_value_for_key(key, v);
            }
        }
        i.set_value_for_key("hw", 1);
        i.set_value_for_key("pb", int(self, "sv_punkbuster"));
        i.set_value_for_key("mod", u8::from(self.modded(&game)));
        Some(format!("infoResponse\n{i}"))
    }

    /// 1.5's `mod` key: 1 for a server that is not pure, runs an `fs_game`,
    /// or references a pak `FS_idPak` (cod_lnxded 1.5b 0x80754ea, the same
    /// rule as the client's) does not count as stock.
    fn modded(&self, fs_game: &str) -> bool {
        !self.game.paks.pure
            || !fs_game.is_empty()
            || self.game.paks.referenced.iter().any(|(_, name)| {
                !vcod_common::net::download::is_stock_pak(name, &self.game.proto.transport)
            })
    }

    /// `SV_MasterGameCompleteStatus` (0x808bbc4) and the packet it sends,
    /// `SVC_GameCompleteStatus` (0x808bfa8): the serverinfo with the
    /// argument as `challenge`, and a `statusResponse` player line per
    /// connected client.
    fn game_complete_status(&mut self, arg: &str) {
        let mut resolve = self.core.resolver;
        let (text, to) = self.core.masters.game_complete_status(
            self.game.dedicated,
            self.svs_time_ms(),
            &mut resolve,
        );
        self.print(&text);
        if to.is_empty() {
            return;
        }
        let mut i = self.serverinfo();
        let mut pairs = vec![("challenge", arg.to_string())];
        pairs.extend(self.demo_keywords(&i));
        if !self.info_pairs(&mut i, pairs) {
            return;
        }
        let msg = format!("gameCompleteStatus\n{i}\n{}", self.status_lines());
        for a in to {
            self.send_oob(a, &msg);
        }
    }

    /// The player lines `SVC_Status` and `SVC_GameCompleteStatus` share:
    /// score, ping and name per client.
    fn status_lines(&mut self) -> String {
        let mut lines = String::new();
        for slot in 0..self.core.clients.len() {
            let Some(c) = self.core.clients[slot].as_ref() else {
                continue;
            };
            let (ping, name) = (c.link.ping, c.name.clone());
            let score = self.client_score(slot);
            lines.push_str(&format!("{score} {ping} \"{name}\"\n"));
        }
        lines
    }

    /// Appends `pairs` through `Info_SetValueForKey`. A pair past
    /// `MAX_INFO_STRING` is skipped on 1.5 and UO, which warn and go on;
    /// 1.1's `Com_Error` drops the level (or crashes), so there the reply
    /// is abandoned and false comes back (cod11-server-handshake.md, "The
    /// challenge key").
    fn info_pairs(&self, i: &mut Info, pairs: Vec<(&str, String)>) -> bool {
        for (key, value) in pairs {
            if !i.set_value_for_key(key, value) && !self.v15() {
                return false;
            }
        }
        true
    }

    /// The `demo` keyword `SVC_Status` and `SVC_GameCompleteStatus` add
    /// after `challenge` when `fs_restrict` is non-zero (0x808bda3,
    /// 0x808bff3; 1.5b 0x80924c5, UO 0x809336d): `sv_keywords` becomes
    /// `demo <sv_keywords>`, moved to the end.
    fn demo_keywords(&self, info: &Info) -> Option<(&'static str, String)> {
        let restrict = crate::game::builtins::cvar::atof(&self.live_cvar("fs_restrict", ""));
        (restrict != 0.0).then(|| {
            let kw = info.get("sv_keywords").unwrap_or("");
            ("sv_keywords", format!("demo {kw}"))
        })
    }

    /// `SVC_Status` (0x808bd50; 1.5b 0x8092538 adds `con_disabled` and
    /// `mod`).
    pub(super) fn svc_status(&mut self, challenge: &str) -> Option<String> {
        let mut i = self.serverinfo();
        let mut pairs = vec![("challenge", challenge.to_string())];
        pairs.extend(self.demo_keywords(&i));
        if self.v15() {
            let disabled = self.live_int_cvar("sv_disableClientConsole");
            if disabled != 0 {
                pairs.push(("con_disabled", disabled.to_string()));
            }
        }
        pairs.push(("pswrd", self.pswrd().to_string()));
        if self.v15() {
            let game = self.live_cvar("fs_game", "");
            pairs.push(("mod", u8::from(self.modded(&game)).to_string()));
        }
        if !self.info_pairs(&mut i, pairs) {
            return None;
        }
        let lines = self.status_lines();
        Some(format!("statusResponse\n{i}\n{lines}"))
    }

    /// `SVC_RemoteCommand` (0x808c404). The reply is whatever the command
    /// printed, in `print\n` packets of up to `rcon::OUTPUT_BUF` bytes; a
    /// rate-limited request gets nothing.
    fn svc_remote_command(&mut self, from: SocketAddr, raw: &[u8], read: Instant, now: Instant) {
        use crate::rcon::Verdict;
        let args = crate::game::say::tokenize(&oob_line(raw));
        let verdict = self.core.rcon.check(&self.game.rcon_password, &args, read);
        let shown = args.get(2).map_or("", String::as_str);
        match verdict {
            Verdict::Limited => return,
            Verdict::Run(_) => log::info!("Rcon from {from}:\n{shown}"),
            _ => log::info!("Bad rcon from {from}:\n{shown}"),
        }
        self.game.redirect = Some(crate::rcon::Redirect::default());
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
        if self.core.quit || self.game.killed {
            self.game.redirect = None;
            return;
        }
        for reply in self.game.redirect.take().unwrap_or_default().finish() {
            self.send_oob(from, &reply);
        }
    }

    /// `SV_GetChallenge` without the authorize-server detour, on the table
    /// all four retail servers share (`engine::challenge`). A banned guid
    /// gets the error retail relays from an authorize denial (1.1) or its
    /// own refusal at the `accept` (1.5); the guid stands in for an
    /// off-LAN address, and a LAN client has none, as on retail.
    fn svc_get_challenge(&mut self, from: SocketAddr, _now: Instant) {
        let guid = guid_of(crate::engine::Lan::Cod1, from);
        if self.core.bans.is_banned(guid) {
            self.core.challenges.forget(from);
            if self.v15() {
                // 1.5's `ban.txt` check at the authorize `accept` (0x80896bc).
                self.print(&format!(
                    "rejected connection from permanently banned GUID {guid}\n"
                ));
                self.send_oob(
                    from,
                    "error\n\u{15}You are permanently banned from this server",
                );
            } else {
                self.send_oob(from, "error\nEXE_ERR_BAD_CDKEY");
            }
            return;
        }
        if self
            .core
            .temp_bans
            .banned(guid, self.core.sv_time_ms, self.kick_ban_time())
        {
            self.core.challenges.forget(from);
            self.print(&format!(
                "rejected connection from temporarily banned GUID {guid}\n"
            ));
            self.send_oob(
                from,
                "error\n\u{15}You are temporarily banned from this server",
            );
            return;
        }
        // Slots on the unscaled clock (1.5b 0x83ccd88), the draw's xor on
        // the game's (0x83ccd84); one clock on 1.1d (0x83b67a4).
        let (svs, game) = (self.svs_time_ms(), self.core.sv_time_ms);
        let rng = &mut self.core.rng;
        let challenge = self
            .core
            .challenges
            .issue(from, svs, || (rand_from(rng) << 16) ^ rand_from(rng) ^ game);
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
        if val("protocol") != Some(self.game.proto.transport.version as i32) {
            let v = self.game.proto.transport.short_version;
            self.send_oob(
                from,
                &format!("error\nEXE_SERVER_IS_DIFFERENT_VER\x15{v}\n"),
            );
            return;
        }
        let (Some(challenge), Some(qport)) = (val("challenge"), val("qport")) else {
            self.send_oob(from, "error\nEXE_BAD_CHALLENGE");
            return;
        };
        let qport = qport as u16;
        let same_peer = |c: &Client| {
            c.link.addr.ip() == from.ip()
                && (c.link.netchan.qport == qport || c.link.addr.port() == from.port())
        };
        if self
            .core
            .clients
            .iter()
            .chain(&self.game.zombies)
            .flatten()
            .any(|c| same_peer(c) && now.duration_since(c.last_connect) < RECONNECT_LIMIT)
        {
            log::debug!("{from}: reconnect rejected, too soon");
            return;
        }
        // The whole address, port included (`NET_CompareAdr` 0x8080dec).
        let svs = self.svs_time_ms();
        let Some((ci, ping)) = self.core.challenges.connect(from, challenge, svs) else {
            self.send_oob(from, "error\nEXE_BAD_CHALLENGE");
            return;
        };
        log::debug!("Client {ci} connecting with {ping} challenge ping");
        // Off the LAN only, against `->value`; 0 turns each test off
        // (0x80857ee..0x80858a4).
        if !crate::engine::Lan::Cod1.is_lan(from.ip()) {
            let min = crate::game::builtins::cvar::atof(&self.live_cvar("sv_minPing", "0"));
            let max = crate::game::builtins::cvar::atof(&self.live_cvar("sv_maxPing", "0"));
            if min != 0.0 && (ping as f32) < min {
                log::debug!("Client {ci} rejected on a too low ping");
                self.send_oob(from, "error\nEXE_ERR_HIGH_PING_ONLY");
                return;
            }
            if max != 0.0 && (ping as f32) > max {
                log::debug!("Client {ci} rejected on a too high ping: {ping}");
                self.send_oob(from, "error\nEXE_ERR_LOW_PING_ONLY");
                return;
            }
        }
        let reconnect = self
            .core
            .clients
            .iter()
            .position(|c| c.as_ref().is_some_and(same_peer));
        if let Some(i) = reconnect {
            // Retail hands the slot to any challenge issued to this ip, so
            // a neighbour behind the same NAT who knows the qport can take
            // over a live player. Only the slot's own challenge (the
            // client's connect retry) may replace a client still heard
            // from; a silent slot (crash, lost disconnect) is reclaimable.
            let c = self.core.clients[i].as_ref().unwrap();
            if c.link.netchan.challenge != challenge
                && now.duration_since(c.last_packet) < RECONNECT_LIMIT
            {
                log::info!("{from}: connect with a foreign challenge refused, client {i} is live");
                return;
            }
        }
        // A zombie of this peer is a reconnect into its own slot; the game
        // saw it leave at the drop, so there is nothing to tear down.
        let Some(slot) = reconnect.or_else(|| {
            self.game
                .zombies
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
            // 1.5's and UO's `SV_FreeClient` (1.5b 0x808a6e3, UO 0x808b4e7)
            // runs the game's `ClientDisconnect` here, inside the connect.
            // 1.1d frees only the client's script id (0x80859c8) and the
            // game's `ClientConnect` starts the held player over: no
            // callback, and its threads live on (cod11-server-handshake.md,
            // "A reconnect into a held slot").
            if self.v15() {
                self.core.clients[slot] = None;
                self.client_disconnect(slot);
            }
        } else {
            self.game.zombies[slot] = None;
        }
        // `Info_SetValueForKey(userinfo, "ip", NET_AdrToString(from))`
        // (0x8085498, key at 0x80d43da): any `ip` the client sent goes, the real one is last.
        let userinfo = format!(
            "{}\\ip\\{}:{}",
            remove_info_key(&userinfo, "ip"),
            from.ip(),
            from.port() as i16
        );
        let mut client = Client::new(from, qport, challenge, userinfo, now);
        client.link.last_packet_ms = self.svs_time_ms();
        log::info!("client {slot} {:?} connected from {from}", client.name);
        let name = client.name.clone();
        self.core.clients[slot] = Some(client);
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.push_client_event(ClientEvent::Connect { slot, name });
        }
        // A taken connect clears the stored ping (0x8085c16).
        self.core.challenges.taken(ci);
        self.send_oob(from, "connectResponse");
        // The first client, or the last the server holds (0x8085cd3).
        let count = self.client_count();
        if count == 1 || count == self.core.clients.len() {
            self.core.masters.force();
        }
    }

    /// The first slot at `start` or above neither a client nor a zombie
    /// holds.
    fn free_slot_from(&self, start: usize) -> Option<usize> {
        (start..self.core.clients.len())
            .find(|&i| self.core.clients[i].is_none() && self.game.zombies[i].is_none())
    }

    /// The netchan half of `SV_PacketEvent`, then `SV_ExecuteClientMessage`.
    /// The slot changes only once the message has passed every check the
    /// plain header and the op stream allow (`Client::accept`), so a packet
    /// forged with the client's ip and qport cannot stall it behind a huge
    /// sequence, redirect its replies or keep a dead slot alive.
    fn handle_client_packet(&mut self, from: SocketAddr, pkt: &[u8], now: Instant) {
        let sv_time_ms = self.svs_time_ms();
        if pkt.len() < 6 {
            log::debug!("{from}: netchan packet too short ({} bytes)", pkt.len());
            return;
        }
        let qport = u16::from_le_bytes([pkt[4], pkt[5]]);
        let peer = |c: &Option<Client>| {
            c.as_ref()
                .is_some_and(|c| c.link.addr.ip() == from.ip() && c.link.netchan.qport == qport)
        };
        let Some(slot) = self.core.clients.iter().position(peer) else {
            match self.game.zombies.iter().position(peer) {
                Some(slot) => self.zombie_packet(slot, pkt),
                None => log::debug!("{from}: netchan packet from no client (qport {qport})"),
            }
            return;
        };
        let Some(c) = self.core.clients[slot].as_ref() else {
            return;
        };
        let Some(m) = c.link.netchan.process_in(pkt) else {
            return;
        };
        // messageAcknowledge names a message we sent, so it is below
        // outgoing_sequence; retail only rejects a negative one.
        if m.message_ack < 0 || m.message_ack >= c.link.netchan.outgoing_sequence as i32 {
            log::debug!(
                "client {slot}: messageAcknowledge {} out of range",
                m.message_ack
            );
            return;
        }
        // 0x808ca1a drops an ack too far behind. One ahead of what we sent is
        // bogus too; `write_pending_commands` walks `reliable_ack + 1 ..=
        // reliable_sequence`, so an unbounded value overflows or loops for ages.
        let reliable_seq = c.link.netchan.reliable_sequence as i32;
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

        if m.server_id != self.game.server_id {
            let c = self.core.clients[slot].as_mut().unwrap();
            if m.server_id & 0xf0 != self.game.server_id & 0xf0 {
                // A map change from the client's view (a fresh client, serverId
                // 0, looks the same). Resend the gamestate once its ack is past
                // the last one. Until snapshots exist the gamestate is our only
                // message, so a lost one never advances the ack and the client
                // falls to its own timeout.
                if i64::from(m.message_ack) > c.gamestate_message_num {
                    c.accept(&m, now, sv_time_ms);
                    self.send_gamestate(slot);
                }
            } else if c.state == ClientState::Primed {
                // Restart path; retail promotes to CS_ACTIVE here. No
                // usercmd came with it, so entry has no entering cmd.
                c.accept(&m, now, sv_time_ms);
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
        self.core.clients[slot].as_mut().unwrap().last_cmd = last_cmd;
        let c = self.core.clients[slot].as_mut().unwrap();
        c.accept(&m, now, sv_time_ms);
        c.link.addr = from; // NAT may move the port; the qport is the identity
        self.game.packet_seq += 1;
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
        let mut r = MsgReader::new(&m.ops, &self.core.huff);
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
        let cmd = &c.link.netchan.reliable[m.reliable_ack as usize & (MAX_RELIABLE_COMMANDS - 1)];
        let key = self.core.checksum_feed ^ m.message_ack ^ com_hash_key(cmd, 32);
        let mut out = Vec::with_capacity(count as usize);
        for _ in 0..count {
            match read_delta_usercmd_in(r, self.game.proto, key, prev) {
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
        let svs_ms = self.svs_time_ms();
        let Some(c) = self.core.clients[slot].as_mut() else {
            return false;
        };
        if seq <= c.link.last_client_command {
            return true;
        }
        if seq > c.link.last_client_command.saturating_add(1) {
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
            if c.state == ClientState::Active && svs_ms < c.link.next_reliable_ms {
                client_ok = false;
                log::debug!("client text ignored for {}: {trimmed}", c.name);
            }
            c.link.next_reliable_ms = svs_ms.wrapping_add(FLOOD_WINDOW_MS);
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
                | "wwwdl"
        );
        if !client_ok && !engine_command {
            c.link.last_client_command = seq;
            c.link.netchan.last_client_command_string = s;
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
                let ok = self
                    .game
                    .pure_check
                    .as_ref()
                    .is_some_and(|p| p.verify(trimmed));
                if let Some(c) = self.core.clients[slot].as_mut() {
                    c.pure = if ok {
                        crate::client::Pure::Authentic
                    } else {
                        crate::client::Pure::Unpure
                    };
                }
            }
            "vdr" => {
                if let Some(c) = self.core.clients[slot].as_mut() {
                    c.pure = crate::client::Pure::Unchecked;
                }
            }
            // The download ucmds (cod_lnxded 0x8087a64, 0x8086168, 0x8087960,
            // 0x80879fc, 0x8087a2c).
            "download" => {
                let name = args.split_whitespace().next().unwrap_or("");
                if let Some(c) = self.core.clients[slot].as_mut() {
                    c.download = Some(crate::download::ServerDownload::new(name));
                }
            }
            "nextdl" => {
                let block = args.trim().parse::<i32>().unwrap_or(0);
                let now = self.svs_time_ms();
                let Some(c) = self.core.clients[slot].as_mut() else {
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
                if let Some(c) = self.core.clients[slot].as_mut() {
                    c.download = None;
                }
            }
            "donedl" => self.send_gamestate(slot),
            // `SV_WWWDownload` (UO 0x808c298), the redirect's replies.
            "wwwdl" => {
                let sub = args.split_whitespace().next().unwrap_or("");
                let uo = self.game.proto.is_uo();
                let Some(c) = self.core.clients[slot].as_mut() else {
                    return false;
                };
                // 1.5b's redirect clears the name it sent (0x808b81e), so its
                // `chkfail` warning prints ''; UO's keeps it
                // (cod15-protocol-diff.md 14).
                let name = match &c.download {
                    Some(d) if uo => d.name.as_str(),
                    _ => "",
                };
                use crate::download::WwwReply;
                match crate::download::wwwdl(&mut c.www, sub, &c.name, name) {
                    WwwReply::None => {}
                    WwwReply::Close => c.download = None,
                    WwwReply::CloseAndGamestate => {
                        c.download = None;
                        self.send_gamestate(slot);
                    }
                    WwwReply::Drop(reason) => {
                        self.drop_client(slot, reason);
                        return false;
                    }
                }
            }
            "retransdl" => {
                let block = args.trim().parse::<i32>().unwrap_or(0);
                if let Some(d) = self.core.clients[slot]
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
                if let Some(c) = self.core.clients[slot].as_mut() {
                    if let Some(name) = info_value_for_key(&ui, "name") {
                        c.name = sanitize_name(name);
                    }
                    c.link.userinfo = ui;
                }
            }
            // `Cmd_Say_f` (0x47050): nothing without an argument. 1.5 adds
            // `say_squad` (0x434b7 with mode 3).
            "say" | "say_team" | "say_squad" if word != "say_squad" || self.v15() => {
                let mode = match word {
                    "say" => SayMode::All,
                    "say_team" => SayMode::Team,
                    _ => SayMode::Squad,
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
                    && self.core.clients.get(to).is_some_and(Option::is_some)
                {
                    let text = crate::game::say::concat_args(text).unwrap_or_default();
                    self.log_tell(slot, to, &text);
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
                let packet = self.game.packet_seq;
                if let Some(c) = self.core.clients[slot].as_mut()
                    && !c.commands_at.iter().any(|(_, k)| *k == PacketCommand::Kill)
                {
                    c.commands_at.push((packet, PacketCommand::Kill));
                }
            }
            // `Cmd_MenuResponse_f`: the client answering a menu `openMenu`
            // opened, or giving one up (`noop`, `bad`). Unlike the entry
            // notify this fires straight through: nothing is armed by it,
            // and a notify no thread is parked on is simply lost, which is
            // what retail does too. It runs in the move pass, in packet
            // order with `kill` (`client-probes/probe_threadorder`).
            // 1.5's `ClientCommand` (0x46341) runs both past its
            // intermission test (cod15-protocol-diff.md, section 12).
            "matchtimeout" | "matchtimein" if self.v15() => {
                self.match_timeout(slot, word == "matchtimein");
            }
            // `Cmd_CallVote_f` and `Cmd_Vote_f`, past `ClientCommand`'s
            // intermission return (docs/research/cod11-votes.md).
            "callvote" | "vote" => self.vote_command(slot, trimmed),
            // `Cmd_NextVehSlot_f` / `Cmd_PrevVehSlot_f` (game.mp.uo 0x4ed9b,
            // 0x4ede1): a living rider changes seats, in the move pass
            // after the cmds of every earlier packet, so the seat's
            // `SetClientViewAngle` bases `delta_angles` on the last of them.
            "nextvehslot" | "prevvehslot" if self.game.proto.is_uo() => {
                let packet = self.game.packet_seq;
                let back = word == "prevvehslot";
                if let Some(c) = self.core.clients[slot].as_mut() {
                    c.commands_at
                        .push((packet, PacketCommand::CycleSeat { back }));
                }
            }
            "fdc" if self.game.proto.is_uo() => {
                let now = self.core.sv_time_ms;
                match fdc_inflictor(args, self.core.clients.len()) {
                    Some(n) => {
                        if let Some(c) = self.core.clients[slot].as_mut() {
                            c.flame = Some((n, now));
                        }
                    }
                    None => log::debug!("client {slot}: bad fdc {args:?}"),
                }
            }
            "mr" => {
                if let Some((menu, response)) =
                    parse_menu_response(trimmed, i32::from(self.game.server_id))
                {
                    let packet = self.game.packet_seq;
                    if let Some(c) = self.core.clients[slot].as_mut() {
                        c.commands_at
                            .push((packet, PacketCommand::MenuResponse(menu, response)));
                    }
                }
            }
            other => log::debug!("client {slot}: command {other:?} ignored"),
        }
        let Some(c) = self.core.clients[slot].as_mut() else {
            return false;
        };
        c.link.last_client_command = seq;
        c.link.netchan.last_client_command_string = s;
        true
    }

    /// `SV_UserMove` past the parse: queue the block for the next tick's
    /// replay. A flood past the cap drops the oldest cmds, not the newest.
    fn user_move(&mut self, slot: usize, cmds: Vec<UserCmd>) {
        let Some(c) = self.core.clients[slot].as_mut() else {
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
        if self.game.paks.pure
            && self.core.clients[slot]
                .as_ref()
                .is_some_and(|c| !c.is_bot && c.pure == crate::client::Pure::Unchecked)
        {
            self.drop_client(slot, "EXE_CANNOTVALIDATEPURECLIENT");
            return;
        }
        let packet = self.game.packet_seq;
        let acked = self.game.ack_time_ms.unwrap_or(self.svs_time_ms());
        let Some(c) = self.core.clients[slot].as_mut() else {
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
        let unpure = self.game.paks.pure
            && self.core.clients[slot]
                .as_ref()
                .is_some_and(|c| c.pure == crate::client::Pure::Unpure);
        if unpure {
            self.drop_client(slot, "EXE_UNPURECLIENTDETECTED");
        }
        !unpure
    }

    /// `SV_SendClientGameState`.
    fn send_gamestate(&mut self, slot: usize) {
        let is_bot = self.core.clients[slot].as_ref().is_some_and(|c| c.is_bot);
        let (dedicated, max_rate) = (self.game.dedicated, self.live_int_cvar("sv_maxRate"));
        let svs_ms = self.svs_time_ms();
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        c.state = ClientState::Primed;
        // The client restarts its usercmd chain on every gamestate it
        // receives, so the base it deltas against restarts here too.
        c.last_cmd = NULL_USERCMD;
        c.gamestate_message_num = i64::from(c.link.netchan.outgoing_sequence);
        let mut w = MsgWriter::new(&self.core.huff);
        write_pending_commands(&mut w, &c.link.netchan, c.link.reliable_ack);
        c.link.reliable_sent = c.link.netchan.reliable_sequence as i32;
        let gs = Gamestate {
            configstrings: self.core.configstrings.clone(),
            baselines: self.game.baselines.clone(),
            client_num: slot as i32,
            checksum_feed: self.core.checksum_feed,
            server_command_sequence: c.link.netchan.reliable_sequence as i32,
        };
        self.game.proto.write_gamestate(&mut w, &gs);
        let ops = w.into_ops();
        log::info!("client {slot}: gamestate, {} bytes", ops.len());
        let max = if self.game.proto.is_uo() {
            MAX_GAMESTATE_BYTES_UO
        } else {
            MAX_GAMESTATE_BYTES
        };
        if ops.len() > max {
            // Retail's message buffer would cut it here, unchecked.
            log::warn!(
                "client {slot}: gamestate is {} bytes, past retail's {max}",
                ops.len()
            );
        }
        // A bot pulls its own gamestate in step_bots and reads nothing back.
        if is_bot {
            return;
        }
        // The fragments left of an earlier message all go first, in this
        // turn (0x8085f34).
        self.core
            .outbox
            .extend(c.link.unsent.drain(..).map(|pkt| (c.link.addr, pkt)));
        c.stamp_sent(c.link.netchan.outgoing_sequence, svs_ms);
        let rate = c.send_rate(dedicated, max_rate);
        let sent = c
            .link
            .netchan
            .transmit(c.link.last_client_command, &ops, &self.core.huff);
        let msec = c.snapshot_msec();
        c.pace(svs_ms, sent.iter().map(Vec::len).sum(), rate, msec);
        // The first fragment now, the rest one per turn as any message's.
        if let Some(pkt) = c.first_packet(sent) {
            self.core.outbox.push((c.link.addr, pkt));
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
        let Some(c) = self.core.clients[slot].as_mut() else {
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
        let proto = self.game.proto;
        self.game
            .module
            .script
            .as_mut()
            .map_or_else(BTreeMap::new, |rt| rt.packet_entities(proto))
    }

    /// Puts a client where the caller says, the way the `spawn` builtin does.
    /// Test-facing: a gate about what two clients see of each other needs them
    /// somewhere known, and both spawn weighted-random.
    pub fn place_client(&mut self, slot: usize, origin: [f32; 3], yaw_deg: f32) {
        let Some(c) = self.core.clients.get_mut(slot).and_then(Option::as_mut) else {
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
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.set_client_takedamage(slot, true);
        }
    }

    /// Mounts `slot` on the turret numbered `gun` as a use press would, from
    /// `mount_origin` and past the reach and arc tests. Test-facing, like
    /// `place_client`: it puts a second gunner on a map with one reachable
    /// gun. False when either is missing or the gun is manned.
    pub fn test_mount(&mut self, slot: usize, gun: u32, mount_origin: [f32; 3]) -> bool {
        let Some(rt) = self.game.module.script.as_mut() else {
            return false;
        };
        let Some(sim) = self
            .core
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
        let rule = crate::game::turret::AimRule::of(rt.host.defs.proto);
        for (turret, stance, view) in &mounts {
            crate::game::turret::mount_sim(sim, rule, *turret, *stance, *view);
        }
        !mounts.is_empty()
    }

    /// Writes a client's `sessionteam` and nothing else. Test-facing: a 1.5
    /// match timeout needs a side, which a `dm` probe gametype gives no one.
    pub fn test_set_client_team(&mut self, slot: usize, team: &str) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.set_client_team_for_test(slot, team);
        }
    }

    /// Moves a client's playerstate origin and nothing else: no teleport
    /// bit, no event, no view. Test-facing: a replay starts from a retail
    /// capture's settled origin where the join left ours elsewhere.
    pub fn test_set_client_origin(&mut self, slot: usize, origin: [f32; 3]) {
        if let Some(sim) = self
            .core
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
            .game
            .module
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
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.place_entity(num, origin, angles);
        }
    }

    /// The blasts the last tick's missile pass set off, after the same
    /// tick's radius damage pass charged them. Test-facing: the replay in
    /// `tests/common` counts them per frame.
    pub fn pending_explosions(&self) -> &[crate::game::missile::Explosion] {
        &self.game.module.pending_explosions
    }

    /// [`scoreboard`] for this server's clients.
    fn scoreboard(&mut self) -> String {
        scoreboard(&self.core.clients, self.game.module.script.as_mut())
    }

    /// `Cmd_MatchTimeout_f` or `Cmd_MatchTimein_f`, inside the packet as
    /// retail runs it: the configstrings go out as `trap_SetConfigstring`
    /// sends them, ahead of the `e` lines, and the `timescale` write
    /// reaches the cvar mirror at the next frame's flush.
    fn match_timeout(&mut self, slot: usize, timein: bool) {
        let intermission = self.core.clients[slot]
            .as_ref()
            .and_then(|c| c.sim.as_ref())
            .is_some_and(|s| s.pm_type == crate::spectate::PmType::Intermission);
        let now = self.svs_time_ms();
        let Some(rt) = self.game.module.script.as_mut() else {
            return;
        };
        if intermission {
            return;
        }
        mirror_roster(&self.core.clients, rt);
        let out = rt.match_timeout(slot, timein, now);
        self.copy_timeout_configstrings();
        for (to, cmd) in out {
            match to {
                Some(to) => self.send_server_command(to, &cmd),
                None => self.send_to_all(&cmd),
            }
        }
    }

    /// `callvote` or `vote` inside the packet: the lines go out first, then
    /// the vote configstrings the command wrote, as retail sends them.
    fn vote_command(&mut self, slot: usize, line: &str) {
        let intermission = self.core.clients[slot]
            .as_ref()
            .and_then(|c| c.sim.as_ref())
            .is_some_and(|s| s.pm_type == crate::spectate::PmType::Intermission);
        let svs_ms = self.svs_time_ms();
        let Some(rt) = self.game.module.script.as_mut() else {
            return;
        };
        if intermission {
            return;
        }
        mirror_roster(&self.core.clients, rt);
        let argv = crate::game::say::tokenize(line);
        let out = rt.vote_command(slot, &argv, svs_ms);
        // `trap_DropClient` comes before the voter's `m -1`.
        if let Some(target) = rt.host.complaint_drop.take() {
            self.drop_client(target, "GAME_KICKEDFROMCOMPLAINTS");
        }
        self.send_vote_lines(out);
    }

    /// A vote command's or `CheckVote`'s lines, then configstrings 15..18
    /// off the game's table.
    fn send_vote_lines(&mut self, out: crate::game::vote::Sends) {
        for (to, cmd) in out {
            match to {
                Some(to) => self.send_server_command(to, &cmd),
                None => self.send_to_all(&cmd),
            }
        }
        let at = self.game.proto.cs.vote;
        self.copy_game_configstrings(at..=at + 3);
    }

    /// The two match timeout configstrings off the game's table, sent at
    /// once: the game writes them with `trap_SetConfigstring`, which does
    /// not wait for the frame's diff.
    fn copy_timeout_configstrings(&mut self) {
        let at = self
            .game
            .proto
            .cs
            .match_timeout
            .unwrap_or(crate::game::timeout::CS_TIMEOUT_MS);
        self.copy_game_configstrings([at, at + 1]);
    }

    /// Configstrings off the game's table, sent at once, as a
    /// `trap_SetConfigstring` outside the frame's diff sends them.
    fn copy_game_configstrings(&mut self, slots: impl IntoIterator<Item = usize>) {
        let Some(rt) = self.game.module.script.as_ref() else {
            return;
        };
        for i in slots {
            if let (Some(to), Some(from)) = (
                self.core.configstrings.get_mut(i),
                rt.configstrings().get(i),
            ) {
                to.clone_from(from);
            }
        }
        self.broadcast_configstring_changes();
    }

    /// `SV_SendServerCommand(NULL, ..)`: every client that has its gamestate.
    fn send_to_all(&mut self, cmd: &str) {
        for slot in 0..self.core.clients.len() {
            if self.core.clients[slot]
                .as_ref()
                .is_some_and(|c| c.state != ClientState::Connected)
            {
                self.send_server_command(slot, cmd);
            }
        }
    }

    /// `G_Say`: retail's `trap_SendServerCommand` queues the line from
    /// inside `ClientCommand`, and so does this. Nothing without a script, which is
    /// where the teams and session states live.
    /// `Cmd_Tell_f`'s log line ahead of its two `G_Say`s: 1.1's `tell: %s
    /// to %s: %s` (0x47210), 1.5's and UO's `tell;%d;%d;%s;%d;%d;%s;%s`
    /// (each side's GUID, 0 with no authorize, slot and name), the names
    /// as sent (cod11-map-cycle.md 9).
    fn log_tell(&mut self, slot: usize, to: usize, text: &str) {
        let name = |s: usize| -> String {
            self.core
                .clients
                .get(s)
                .and_then(Option::as_ref)
                .map_or(String::new(), |c| c.name.clone())
        };
        let (from_name, to_name) = (name(slot), name(to));
        let Some(rt) = self.game.module.script.as_mut() else {
            return;
        };
        let line = if rt.host.logs_1_1() {
            format!("tell: {from_name} to {to_name}: {text}\n")
        } else {
            format!("tell;0;{slot};{from_name};0;{to};{to_name};{text}\n")
        };
        rt.host.log_printf(line);
    }

    fn say(&mut self, slot: usize, target: Option<usize>, mode: SayMode, text: &str) {
        let Some(rt) = self.game.module.script.as_mut() else {
            return;
        };
        mirror_roster(&self.core.clients, rt);
        for (to, cmd) in rt.say(slot, target, mode, text) {
            self.send_server_command(to, &cmd);
        }
    }

    /// Test-facing: every item landing asks `f(level_time, floor)` for its
    /// lift instead of drawing retail's random half unit (items doc 14.2), so
    /// a gate can see each landing and rest it where a retail capture's draw
    /// put it.
    pub fn test_item_lifts(&mut self, f: crate::game::host::ItemLift) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.test_item_lift = Some(f);
        }
    }

    /// Test-facing: the `rand()` values the next item draws take in place of
    /// the server's own (`game::item`'s `unit_draw`), so a gate can launch a
    /// drop with the climb and spin a retail capture shows.
    pub fn pin_item_draws(&mut self, draws: Vec<i32>) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.item_draw_pins = draws.into();
        }
    }

    /// Test-facing: the missile pool's next draws, a retail capture's
    /// tumble rates and bounce factors (`Missiles::pin_draws`).
    pub fn pin_missile_draws(&mut self, draws: Vec<f64>) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.missiles.pin_draws(draws);
        }
    }

    /// Test-facing: the points the next artillery incomings land on
    /// (`GameHost::barrage_pins`).
    pub fn pin_barrage_points(&mut self, points: Vec<[f32; 3]>) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.barrage_pins = points.into_iter().map(glam::Vec3::from).collect();
        }
    }

    /// Test-facing: the running level's `holdrand` from `seed`, as
    /// `G_InitGame`'s `Rand_Init(trap_Milliseconds())` left it (map-cycle
    /// doc 4.8): a gate replays a retail capture's script draws with the
    /// seed recovered off them.
    pub fn pin_script_rand(&mut self, seed: u32) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.holdrand = vcod_common::rng::HoldRand(seed);
        }
    }

    /// Test-facing: the `gunrandom` pairs the next rockets' spread takes,
    /// read off retail under gdb (combat doc 13.6).
    pub fn pin_spread_draws(&mut self, draws: Vec<(f32, f32)>) {
        self.game.module.spread_draws = draws.into();
    }

    /// Test-facing: the next rockets' `trDelta`, a retail capture's
    /// (`Missiles::pin_rocket_deltas`).
    pub fn pin_rocket_deltas(&mut self, deltas: Vec<[f32; 3]>) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host
                .missiles
                .pin_rocket_deltas(deltas.into_iter().map(glam::Vec3::from));
        }
    }

    /// Test-facing: the outcomes of the next UO turret rounds' tracer rolls,
    /// one per player hit, as a retail capture drew them.
    pub fn pin_tracer_rolls(&mut self, rolls: Vec<bool>) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.tracer_pins = rolls.into();
        }
    }

    /// Test-facing: raise entity `num` by `dz` (an `origin` write) and return
    /// its origin; `dz` 0 only reads it.
    pub fn test_raise_entity(&mut self, num: u32, dz: f32) -> Option<[f32; 3]> {
        self.game.module.script.as_mut()?.raise_entity(num, dz)
    }

    /// Test-facing: `self pingPlayer()` on `slot`, with no script to call
    /// it.
    pub fn test_ping_player(&mut self, slot: usize) {
        if let Some(rt) = self.game.module.script.as_mut() {
            let until = rt.host.level_time_ms + 3000;
            if let Some(p) = rt.host.client_ping_until.get_mut(slot) {
                *p = until;
            }
        }
    }

    /// Test-facing: `self giveWeapon(name)` on `slot` without its ammo, for
    /// a weapon no stock loadout hands out. False for an unknown name.
    pub fn test_give_weapon(&mut self, slot: usize, name: &str) -> bool {
        let Some(rt) = self.game.module.script.as_mut() else {
            return false;
        };
        let Some(index) = rt.host.defs.weapon_index(name) else {
            return false;
        };
        let weapons = rt.host.weapons.clone();
        rt.host.client_weapons[slot].give_chain(&weapons, index, weapons.slot(index));
        true
    }

    /// Test-facing: `self giveMaxAmmo(name)` on `slot`, a full clip and
    /// reserve, as the frame's weapon pass applies it. False for an unknown
    /// name.
    pub fn test_give_max_ammo(&mut self, slot: usize, name: &str) -> bool {
        use crate::game::host::WeaponOp;
        let Some(rt) = self.game.module.script.as_mut() else {
            return false;
        };
        let Some(index) = rt.host.defs.weapon_index(name) else {
            return false;
        };
        let weapons = rt.host.weapons.clone();
        let Some(def) = weapons.get(index) else {
            return false;
        };
        if !def.clip_only {
            rt.host.weapon_op(
                slot,
                WeaponOp::SetAmmo {
                    ammo_index: def.ammo_index,
                    rounds: def.max_ammo as i16,
                },
            );
        }
        rt.host.weapon_op(
            slot,
            WeaponOp::SetClip {
                clip_index: def.clip_index,
                rounds: def.clip_size as i16,
            },
        );
        true
    }

    /// Test-facing: the seed the spread and the melee rolls draw from, so a
    /// gate that needs a shot to land where it did once can have it again.
    /// The bots added after it draw their own streams off `seed` too.
    pub fn test_seed_rng(&mut self, seed: u64) {
        self.core.rng = seed | 1;
        self.game.module.bot_admin.seed_rng = bot_admin::seed_stream(seed);
    }

    /// Whether a shot fired from eye height at `origin` along `yaw_deg`
    /// reaches `dist` without hitting anything. Test-facing, like
    /// `place_client`: a gate that puts one client in front of another has to
    /// say the sightline between them is real, or a spawn point that moved
    /// into a wall reads as "the damage path is broken". A server with no
    /// collision world answers yes.
    pub fn test_clear_line(&self, origin: [f32; 3], yaw_deg: f32, dist: f32) -> bool {
        let Some(world) = self.game.world.as_ref() else {
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
        let Some(world) = self.game.world.as_ref() else {
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
        let Some(world) = self.game.world.as_ref() else {
            return 1.0;
        };
        let bodies: Vec<crate::game::combat::HitBody> = self
            .core
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.as_ref()?.sim.as_ref()?.hit_body(i))
            .collect();
        let models = self
            .game
            .module
            .script
            .as_mut()
            .map_or_else(Vec::new, |rt| rt.placed_script_models());
        let mut bones = crate::game::combat::BoneTraceCtx::of(
            self.game.fs.as_deref(),
            self.game.module.anims.as_deref(),
            &mut self.game.module.hit_rigs,
            self.core.sv_time_ms,
            dobj_stamps(self.game.module.script.as_mut()),
            vcod_common::patch::Arith::for_protocol(self.game.proto),
        );
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
        let g = self.game.module.driver.nav.as_ref()?;
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

    /// Test-facing: what a standing box dropped from `p` lands on, and
    /// where, as `CollisionWorld::describe` names it.
    pub fn test_floor_under(&self, p: [f32; 3]) -> Option<(f32, String)> {
        let world = self.game.world.as_ref()?;
        use vcod_common::pmove::{HALF_WIDTH, Stance};
        let mins = glam::Vec3::new(-HALF_WIDTH, -HALF_WIDTH, 0.0);
        let maxs = glam::Vec3::new(HALF_WIDTH, HALF_WIDTH, Stance::Stand.height());
        let start = glam::Vec3::from(p);
        let t = world
            .collision
            .box_trace(start, start - glam::Vec3::Z * 256.0, mins, maxs);
        Some((
            t.endpos.z,
            t.hit
                .map(|h| world.collision.describe(h))
                .unwrap_or_default(),
        ))
    }

    /// Test-facing: entity `ent`'s box in the area tree (`r.absmin`,
    /// `r.absmax`), `None` while it is not linked.
    pub fn test_area_link(&self, ent: u32) -> Option<([f32; 3], [f32; 3])> {
        let link = self.game.module.script.as_ref()?.host.area.last_link(ent)?;
        Some((link.absmin, link.absmax))
    }

    /// Test-facing: whether submodel `m`'s brushes clip.
    pub fn test_model_linked(&self, m: usize) -> bool {
        self.game
            .world
            .as_ref()
            .is_some_and(|w| w.collision.model_linked(m))
    }

    /// Test-facing: where a standing player dropped at `p` comes to rest,
    /// or `None` when it starts inside geometry or finds no floor within
    /// 256 units. What a test needs to put a second client somewhere the map
    /// actually holds one.
    pub fn test_ground_under(&self, p: [f32; 3]) -> Option<[f32; 3]> {
        let world = self.game.world.as_ref()?;
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
        let c = self.core.clients.get(slot)?.as_ref()?;
        let state = c
            .sim
            .as_ref()?
            .to_entity(self.game.proto, slot, c.last_processed_st);
        let (now, proto) = (self.core.sv_time_ms, self.game.proto);
        let collision = self.game.world.as_ref().map(|w| &w.collision);
        self.game
            .module
            .script
            .as_mut()
            .map(|rt| rt.bodies_mut().push(state, None, now, collision, proto))
    }

    /// Raises one event for the next snapshot build. Test-facing, like
    /// `test_push_body`: the script builtins are what raise these in earnest.
    pub fn test_push_temp_entity(&mut self, te: temp_entity::TempEntity) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.push_temp_entity(te);
        }
    }

    /// Puts a client back to spectating, where every client starts and where
    /// a dead one waits. Test-facing, like `place_client`.
    pub fn spectate_client(&mut self, slot: usize, origin: [f32; 3]) {
        let Some(c) = self.core.clients.get_mut(slot).and_then(Option::as_mut) else {
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
        let Some(rt) = self.game.module.script.as_mut() else {
            return Vec::new();
        };
        let live: Vec<usize> = (0..self.core.clients.len())
            .filter(|slot| self.core.clients[*slot].is_some())
            .collect();
        let mut out = Vec::new();
        for slot in live {
            if rt.client_field(slot, "classname").as_deref() == Some("player") {
                out.push((slot, rt.client_origin(slot)));
            }
        }
        out
    }

    /// `SV_DropClient` (cod_lnxded 0x8085cf4). The game's `ClientDisconnect`
    /// runs inside it: on 1.1 after the drop notice, on 1.5 and UO before it
    /// (cod11-server-handshake.md, "When the game hears a drop"). The slot
    /// goes `CS_ZOMBIE` with the drop notice queued, and its slot's turn
    /// keeps sending it until `check_timeouts` frees it, [`ZOMBIE_TIME`]
    /// past the client's last packet.
    /// A bot has nobody to tell and is freed now.
    fn drop_client(&mut self, slot: usize, reason: &str) {
        let Some(mut c) = self.core.clients[slot].take() else {
            return;
        };
        let v15 = self.v15();
        let mut own = Vec::new();
        if v15 {
            own = self.client_disconnect(slot);
        }
        // The first slot with the client's address (0x8085cf4).
        if !c.is_bot {
            self.core.challenges.release(c.link.addr);
        }
        log::debug!("Going to CS_ZOMBIE for {}", c.name);
        // Everyone else is told unless the client left on its own.
        if reason != "EXE_DISCONNECTED" {
            let notice = format!("e \"\u{15}{}^7 \u{14}{reason}\"", c.name);
            for other in 0..self.core.clients.len() {
                if self.core.clients[other]
                    .as_ref()
                    .is_some_and(|o| o.state != ClientState::Connected)
                {
                    self.send_server_command(other, &notice);
                }
            }
        }
        self.print(&format!("{slot}:{} {reason}\n", c.name));
        if !v15 {
            own = self.client_disconnect(slot);
        }
        // What the callback sent the leaver alone goes into its ring, and
        // the zombie's `w` squashes every unsent ignorable command, the
        // callback's `iPrintLn` with them: a `setClientCvar` reaches the
        // zombie on all three, a print on none (`client-probes/probe_reconnect`).
        for cmd in own {
            c.queue_server_command(&cmd);
        }
        c.squash_ignorable();
        // Appended with no guard: the notice must not be dropped, and a full
        // ring cannot drop a client twice.
        c.link.netchan.reliable_sequence += 1;
        let at = c.link.netchan.reliable_sequence as usize & (MAX_RELIABLE_COMMANDS - 1);
        c.link.netchan.reliable[at] = format!("w \"{reason}\"");
        c.reliable_kind[at] = CmdKind::Reliable;
        if !c.is_bot {
            c.zombie = true;
            self.game.zombies[slot] = Some(c);
        }
        // The last client gone (0x8085ebc).
        if self.client_count() == 0 {
            self.core.masters.force();
        }
    }

    /// The game's `ClientDisconnect` for a slot already out of `clients`,
    /// whose server commands and configstrings go out at once, as the
    /// game's `trap_SendServerCommand` sends them: a drop after the frame
    /// still reaches that frame's message. Neither the zombie nor, on 1.5
    /// and UO, the client still in its slot hears the callback's lines
    /// (`client-probes/probe_dropclient`). Returns the commands addressed
    /// to the leaver's own slot, for its zombie.
    fn client_disconnect(&mut self, slot: usize) -> Vec<String> {
        let collision = self.game.world.as_ref().map(|w| &w.collision);
        // 1.5's and UO's `ClientDisconnect` stops a follower outright
        // (1.5b 0x4cf5f, UO 0x55433); 1.1's cycles first.
        let cycle = !self.v15();
        pass_followers_on(
            &mut self.core.clients,
            slot,
            self.game.module.script.as_mut(),
            collision,
            cycle,
        );
        let Some(rt) = self.game.module.script.as_mut() else {
            return Vec::new();
        };
        mirror_roster(&self.core.clients, rt);
        rt.client_disconnect(slot);
        let written: Vec<usize> = rt
            .host
            .configstrings
            .writes()
            .iter()
            .map(|(i, _)| *i)
            .collect();
        let commands = rt.take_client_commands();
        let console = rt.take_console();
        self.core.console.extend(console);
        self.copy_game_configstrings(written);
        let mut own = Vec::new();
        for (to, cmd) in commands {
            if to == slot {
                own.push(cmd);
            } else {
                self.send_server_command(to, &cmd);
            }
        }
        own
    }

    /// A zombie's packet still runs the netchan and records the acks, so a
    /// drop notice the client has seen stops being resent; nothing in it is
    /// executed and it does not hold the slot open (0x808ca44).
    fn zombie_packet(&mut self, slot: usize, pkt: &[u8]) {
        let Some(z) = self.game.zombies[slot].as_mut() else {
            return;
        };
        let Some(m) = z.link.netchan.process_in(pkt) else {
            return;
        };
        let seq = z.link.netchan.reliable_sequence as i32;
        if (0..z.link.netchan.outgoing_sequence as i32).contains(&m.message_ack) {
            z.message_ack = m.message_ack;
        }
        if (0..=seq).contains(&m.reliable_ack)
            && seq - m.reliable_ack < MAX_RELIABLE_COMMANDS as i32
        {
            z.link.reliable_ack = m.reliable_ack;
        }
    }

    /// What `SV_WriteDownloadToClient` (0x8086290) opens a download with:
    /// the bytes, or the refusal. Only paks this server mounts and lists in
    /// `sv_referencedPakNames` go out. Retail's `FS_SV_FOpenFileRead`
    /// opens any path under its game directories, `..` included, so a
    /// `download main/server.cfg` hands out the rcon password there
    /// (docs/research/cod11-server-handshake.md, "Serving a download").
    fn download_opener(
        &self,
    ) -> impl Fn(&str) -> Result<Vec<u8>, crate::download::Refusal> + use<> {
        let allow = self
            .level_cvar("sv_allowDownload")
            .is_none_or(|v| v.trim().parse::<i32>().unwrap_or(0) != 0);
        let pure = self.game.paks.pure;
        let fs = self.game.fs.clone();
        // No stock-pak refusal: retail's `FS_idPak` test gets the name with
        // its `.pk3` and never matches, so both retail servers send a stock
        // pak a client asks for (handshake doc, "Serving a download").
        move |name: &str| {
            use crate::download::Refusal;
            let stem = name.strip_suffix(".pk3").unwrap_or(name);
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
        }
    }

    /// `sv_wwwDownload`, `sv_wwwBaseURL` and `sv_wwwDlDisconnected`, read
    /// per message as retail reads them (UO 0x808c559); `None` with the
    /// first off or on protocol 1, which has no redirect.
    fn www_config(&self) -> Option<crate::download::WwwConfig> {
        (self.v15() && self.live_int_cvar("sv_wwwDownload") != 0).then(|| {
            crate::download::WwwConfig {
                base_url: self.live_cvar("sv_wwwBaseURL", ""),
                disconnected: self.live_int_cvar("sv_wwwDlDisconnected") != 0,
                udp_fallback: !self.game.proto.is_uo(),
            }
        })
    }

    /// `SV_SendClientSnapshot` (0x808f844) for a client not in the world:
    /// retail writes the server commands and the snapshot only for an
    /// active client (or a zombie), so the message is the header and the
    /// download blocks, or the bare header (9 bytes on the wire) for a
    /// client that is not downloading. It goes out on the slot's own turn
    /// once its `nextSnapshotTime` comes round, as every message does
    /// ([`Client::pace`]: every frame on a LAN, else at least a second
    /// apart while not downloading); a client in the world gets its blocks
    /// after its snapshot in [`Self::send_client_snapshot`].
    fn send_unentered(&mut self, slot: usize, snaps: &Cod1Snapshots) {
        let (dedicated, now) = (self.game.dedicated, self.svs_time_ms());
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        let rate = c.send_rate(dedicated, snaps.max_rate);
        let mut w = MsgWriter::new(&self.core.huff);
        write_download(c, &mut w, now, rate, &snaps.open, snaps.www.as_ref());
        c.stamp_sent(c.link.netchan.outgoing_sequence, now);
        let sent =
            c.link
                .netchan
                .transmit(c.link.last_client_command, &w.into_ops(), &self.core.huff);
        let msec = c.snapshot_msec();
        c.pace(now, sent.iter().map(Vec::len).sum(), rate, msec);
        if let Some(pkt) = c.first_packet(sent) {
            self.core.outbox.push((c.link.addr, pkt));
        }
    }

    /// `SV_SendClientSnapshot` for a zombie (0x808f844), on its slot's
    /// turn: its unacked server commands, the drop notice among them, and
    /// the stale ring slot ([`zombie_message`]). Paced like any message:
    /// off the LAN a zombie, not being `CS_ACTIVE`, waits a second between
    /// them (`docs/research/cod11-server-handshake.md`, "Zombie slots").
    fn send_zombie(&mut self, slot: usize, max_rate: i32) {
        let (dedicated, now, game_ms) = (
            self.game.dedicated,
            self.svs_time_ms(),
            self.core.sv_time_ms,
        );
        let snap_flags = self.game.snap_flag_server_bit;
        let Some(z) = self.game.zombies[slot].as_mut() else {
            return;
        };
        let not_active = if z.link.was_active {
            0
        } else {
            SNAPFLAG_NOT_ACTIVE
        };
        let flags = snap_flags | u32::from(z.link.rate_delayed) | not_active;
        let sent = zombie_message(z, game_ms, flags, self.game.proto, &self.core.huff);
        let rate = z.send_rate(dedicated, max_rate);
        let msec = z.snapshot_msec();
        z.pace(now, sent.iter().map(Vec::len).sum(), rate, msec);
        self.core
            .outbox
            .extend(sent.into_iter().map(|p| (z.link.addr, p)));
    }

    /// `SV_FinalMessage` (0x808ad8c), which `SV_Shutdown` runs before the
    /// process exits: twice over, every client past `CS_ZOMBIE` is queued
    /// `e "<message>"` and a bare `w` and sent a snapshot carrying them.
    /// The `e` is dropped for a client not yet active.
    fn final_message(&mut self, message: &str) {
        let notice = format!("e \"{message}\"");
        for _ in 0..2 {
            for slot in 0..self.core.clients.len() {
                if self.core.clients[slot].as_ref().is_none_or(|c| c.is_bot) {
                    continue;
                }
                self.send_server_command(slot, &notice);
                self.send_server_command(slot, "w");
                let Some(c) = self.core.clients[slot].as_mut() else {
                    continue;
                };
                let sent = resend_last_frame(
                    c,
                    self.core.sv_time_ms,
                    self.game.proto,
                    &self.core.huff,
                    &self.game.baselines,
                );
                self.core
                    .outbox
                    .extend(sent.into_iter().map(|p| (c.link.addr, p)));
            }
        }
    }

    /// `SV_CalcPings` (0x808cab8), once a frame after the packets and before
    /// the clock moves. Only an active client in the world is measured; a
    /// test client acks nothing and reads 999 (retail's `status`), vcod's
    /// own bots, which retail does not have, 0 as Q3's do.
    fn calc_pings(&mut self) {
        for (slot, c) in self.core.clients.iter_mut().enumerate() {
            let Some(c) = c else { continue };
            c.link.ping = if c.is_bot && self.game.module.driver.bots.contains_key(&slot) {
                0
            } else if c.is_bot || c.state != ClientState::Active || c.sim.is_none() {
                999
            } else {
                c.calc_ping()
            };
        }
    }

    /// `SV_CheckTimeouts` (0x808cbc0) for the clients in a slot, after the
    /// frame's clock advance and game frame. A last-packet time ahead of the
    /// clock is pulled back to it; one more than `sv_timeout` seconds old
    /// counts a frame, a fresh one resets the count, and the sixth stale
    /// frame in a row drops the client and frees the slot at once, its
    /// notice never sent. A zombie whose last packet is more than
    /// `sv_zombietime` old goes `CS_FREE` first, before any message.
    fn check_timeouts(&mut self) {
        let now = self.svs_time_ms();
        let zombie_ms = ZOMBIE_TIME.as_millis() as i32;
        for z in &mut self.game.zombies {
            if let Some(c) = z.take_if(|c| now.wrapping_sub(c.link.last_packet_ms) > zombie_ms) {
                log::debug!("Going from CS_ZOMBIE to CS_FREE for {}", c.name);
            }
        }
        let timeout = self.live_cvar("sv_timeout", TIMEOUT_DEFAULT);
        let timeout_ms = crate::game::builtins::cvar::atoi(&timeout).wrapping_mul(1000);
        let mut stale = Vec::new();
        for (slot, c) in self.core.clients.iter_mut().enumerate() {
            let Some(c) = c.as_mut().filter(|c| !c.is_bot) else {
                continue;
            };
            if now.wrapping_sub(c.link.last_packet_ms) < 0 {
                c.link.last_packet_ms = now;
            }
            if now.wrapping_sub(c.link.last_packet_ms) > timeout_ms {
                c.link.timeout_count += 1;
                if c.link.timeout_count > 5 {
                    stale.push(slot);
                }
            } else {
                c.link.timeout_count = 0;
            }
        }
        for slot in stale {
            self.drop_client(slot, "EXE_TIMEDOUT");
            self.game.zombies[slot] = None;
        }
    }

    /// The `Client` behind an `addTestClient` slot, whose `ClientConnect` and
    /// `ClientBegin` the builtin already ran: `CS_ACTIVE` at once, a
    /// spectator sim at the origin `ClientConnect`'s zeroing leaves, the
    /// userinfo `SV_AddTestClient` connects with (0x808759b) and no
    /// netchan. It sends no cmds and no brain drives it.
    fn install_test_client(
        clients: &mut [Option<Client>],
        slot: usize,
        port: u16,
        name: String,
        proto: &'static Protocol,
        sv_time_ms: i32,
    ) {
        let Some(entry) = clients.get_mut(slot) else {
            return;
        };
        let userinfo = format!(
            "\\cg_predictItems\\1\\cl_anonymous\\0\\handicap\\100\\color\\4\\head\\default\\model\\multi\\snaps\\20\\rate\\5000\\name\\{name}\\protocol\\{}\\ip\\bot",
            proto.transport.version
        );
        let addr = SocketAddr::from(([127, 0, 0, 1], 65000 - slot as u16));
        let mut client = Client::new(addr, 0, 0, userinfo, Instant::now());
        client.is_bot = true;
        client.bot_port = Some(port);
        client.state = ClientState::Active;
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.ps.rules = vcod_common::pmove::MoveRules::for_protocol(proto);
        client.sim = Some(sim);
        client.last_processed_st = sv_time_ms;
        client.last_usercmd_st = sv_time_ms;
        log::info!("client {slot} {name:?} connected (test client)");
        *entry = Some(client);
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
                seated: false,
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
            speed: sim.ps.speed,
            seated: sim.seat.is_some(),
        })
    }

    /// Test-facing: the shots and blasts the bots hear next tick.
    pub fn bot_noises(&self) -> &[crate::bots::Noise] {
        &self.game.module.driver.noises
    }

    /// Test-facing, for the bot gates: the slots the bots hold.
    pub fn bot_slots(&self) -> Vec<usize> {
        self.game.module.driver.bots.keys().copied().collect()
    }

    /// Test-facing: where a bot's body is and what it is doing.
    pub fn bot_body(&self, slot: usize) -> Option<crate::bots::BotBody> {
        let sim = self.core.clients[slot].as_ref()?.sim.as_ref()?;
        Some(crate::bots::BotBody {
            origin: sim.ps.origin.into(),
            on_ladder: sim.ps.on_ladder,
            on_ground: sim.ps.on_ground,
            playing: sim.pm_type == crate::spectate::PmType::Normal && !sim.dead,
            dead: sim.dead,
            health: sim.health,
            clip: {
                let d = self.game.module.weapon_table.get(sim.ps.weapon as usize);
                d.map_or(-1, |d| sim.ps.ammoclip[d.clip_index])
            },
            weapon: sim.ps.weapon,
            gun: self
                .game
                .module
                .weapon_table
                .get(sim.ps.weapon as usize)
                .is_some_and(|d| d.weapon_type == "bullet"),
            unsticking: self
                .game
                .module
                .driver
                .bots
                .get(&slot)
                .is_some_and(|b| b.unsticking()),
            sprinting: sim.ps.sprinting,
            sprint_leg: self
                .game
                .module
                .driver
                .bots
                .get(&slot)
                .is_some_and(|b| b.sprint_leg()),
            deployed: sim.ps.deployed(),
            engaged: self
                .game
                .module
                .driver
                .bots
                .get(&slot)
                .is_some_and(|b| b.engaged()),
            retreating: self
                .game
                .module
                .driver
                .bots
                .get(&slot)
                .map_or(0, |b| b.retreating()),
            seat: sim.seat.map(|s| (s.vehicle, s.pos)),
            crew_wait: self
                .game
                .module
                .driver
                .bots
                .get(&slot)
                .is_some_and(|b| b.waiting_for_crew()),
            boardings: self
                .game
                .module
                .driver
                .bots
                .get(&slot)
                .map_or(0, |b| b.boardings()),
            rockets_at_armor: self
                .game
                .module
                .driver
                .bots
                .get(&slot)
                .map_or(0, |b| b.rockets_at_armor()),
            target: self
                .game
                .module
                .driver
                .bots
                .get(&slot)
                .and_then(|b| b.target_slot()),
            firing: self
                .game
                .module
                .driver
                .bots
                .get(&slot)
                .is_some_and(|b| b.fired()),
            clip_size: self
                .game
                .module
                .weapon_table
                .get(sim.ps.weapon as usize)
                .filter(|d| d.weapon_type == "bullet")
                .map_or(0, |d| d.clip_size.min(i16::MAX as u32) as i16),
            throws: self
                .game
                .module
                .driver
                .bots
                .get(&slot)
                .map_or(0, |b| b.throws()),
            yaw: sim.view_angles()[1],
            walking: self
                .game
                .module
                .driver
                .bots
                .get(&slot)
                .is_some_and(|b| b.walking()),
            holds_cover: self
                .game
                .module
                .driver
                .bots
                .get(&slot)
                .is_some_and(|b| b.holds_cover()),
            on_objective: self
                .game
                .module
                .driver
                .bots
                .get(&slot)
                .is_some_and(|b| b.on_objective()),
        })
    }

    /// Read-only, for `bot_match`'s `DIGEST=`: a client's last cmd that ran
    /// (`pers.cmd`), its playerstate and its health.
    pub fn bot_digest_state(
        &self,
        slot: usize,
    ) -> Option<(UserCmd, &vcod_common::pmove::PlayerState, i32)> {
        let sim = self.core.clients.get(slot)?.as_ref()?.sim.as_ref()?;
        Some((sim.pers_cmd, &sim.ps, sim.health))
    }

    /// Test-facing: turns a bot's retreat on or off (`Bot::set_retreat`).
    pub fn test_bot_retreat(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.set_retreat(on);
        }
    }

    /// Test-facing: whether a bot varies its peeks and hunts from a
    /// vantage (`Bot::wary`).
    pub fn test_bot_wary(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.wary = on;
        }
    }

    /// Test-facing: a bot's peeks, re-peeks and hunt arrivals
    /// (`Bot::tactics`).
    pub fn bot_tactics(&self, slot: usize) -> Option<[u32; 3]> {
        Some(self.game.module.driver.bots.get(&slot)?.tactics())
    }

    /// Test-facing: whether a bot fights from cover (`bots::cover`).
    pub fn test_bot_cover(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.cover.enabled = on;
        }
    }

    /// Test-facing: whether a bot moves while it fights (`Bot::footwork`).
    pub fn test_bot_footwork(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.footwork = on;
        }
    }

    /// Test-facing: whether a bot aims as a hand does (`Bot::human_aim`).
    pub fn test_bot_human_aim(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.human_aim = on;
        }
    }

    /// Test-facing: whether a bot reacts to hits, keeps its target and
    /// sees in a cone (`Bot::aware`).
    pub fn test_bot_aware(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.aware = on;
        }
    }

    /// Test-facing: whether a bot plays as one of its team (`bots::team`).
    pub fn test_bot_team_play(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.team_play = on;
        }
    }

    /// Test-facing: whether a bot keeps apart from its team (`Bot::squad`).
    pub fn test_bot_squad(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.squad = on;
        }
    }

    /// Test-facing: whether a bot holds a cover spot, and its `ps.lean`.
    pub fn bot_cover_state(&self, slot: usize) -> Option<(bool, f32)> {
        let sim = self.core.clients[slot].as_ref()?.sim.as_ref()?;
        Some((
            self.game.module.driver.bots.get(&slot)?.holds_cover(),
            sim.ps.lean,
        ))
    }

    /// Test-facing: pins a bot's goal to a point (`Bot::force_goal`).
    pub fn test_bot_goal(&mut self, slot: usize, p: Option<[f32; 3]>) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.force_goal(p);
        }
    }

    /// Test-facing: a bot's current waypoint node and the next, as points.
    pub fn bot_waypoints(&self, slot: usize) -> (Option<[f32; 3]>, Option<[f32; 3]>) {
        let (Some(g), Some(f)) = (
            self.game.module.driver.nav.as_ref(),
            self.game.module.driver.paths.get(&slot),
        ) else {
            return (None, None);
        };
        let at = |n: Option<u32>| n.map(|n| g.nodes[n as usize].into());
        let (a, b) = f.current();
        (at(a), at(b))
    }

    /// Test-facing: the ticks a bot has waited aside for a climber
    /// (`nav::Follower::yielding`).
    pub fn bot_yielding(&self, slot: usize) -> u32 {
        self.game
            .module
            .driver
            .paths
            .get(&slot)
            .map_or(0, |f| f.yielding())
    }

    /// Test-facing: the name a bot joined with.
    pub fn bot_name(&self, slot: usize) -> Option<&str> {
        self.game
            .module
            .driver
            .bots
            .get(&slot)
            .map(|b| b.name.as_str())
    }

    /// Test-facing: a client command a bot sends with its next replies.
    pub fn test_bot_command(&mut self, slot: usize, cmd: &str) {
        self.game
            .module
            .bot_admin
            .orders
            .push((slot, cmd.to_string()));
    }

    /// Test-facing: the script team value a bot landed in.
    pub fn bot_team(&mut self, slot: usize) -> i32 {
        self.game
            .module
            .script
            .as_mut()
            .map_or(0, |rt| rt.client_team(slot))
    }

    /// Builds the bots' navigation graph off the tick thread: the server
    /// keeps real time through the build and the bots wander until it lands.
    /// Off by default, so a test's bots find the graph on their first tick.
    pub fn build_nav_in_background(&mut self, on: bool) {
        self.game.module.driver.nav_background = on;
    }

    /// Swap in the map built by the binary; tests run without one.
    pub fn load_world(&mut self, world: World) {
        world
            .collision
            .set_arith(vcod_common::patch::Arith::for_protocol(self.game.proto));
        self.game.world = Some(Rc::new(world));
        self.game.module.driver.nav = None;
        self.game.module.driver.nav_job = None;
        self.game.module.nav_relink = None;
        self.game.module.nav_rebuild = None;
        self.game.module.driver.paths.clear();
        self.game.module.driver.sites.clear();
        self.game.module.bot_breach_stands.clear();
        self.game.module.driver.noises.clear();
        self.game.module.driver.covers.clear();
        self.game.module.driver.hunts.clear();
        self.game.module.driver.creases = Default::default();
    }

    /// The cvar table a gametype script starts with: the engine defaults,
    /// then `default_mp.cfg`, then `g_gametype` and `sv_maxclients`
    /// mirroring this run's `ServerConfig` (`sv_hostname` too, at the first
    /// load only), and the engine
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
    /// both boundaries. The `--set`s go in at the first load only, as
    /// `Com_StartupVariable` runs once; [`Self::set_cvar`] writes the
    /// running table after that.
    fn cvars(&self, fs: &dyn vcod_common::asset_source::AssetSource) -> crate::cvars::Cvars {
        let mut cvars = match self.game.carried_cvars.clone() {
            Some(c) => c,
            None => {
                let mut cvars =
                    crate::cvars::Cvars::for_protocol(self.game.proto.transport.version);
                match fs.load::<RawFile>("default_mp.cfg").ok() {
                    Some(raw) => {
                        let n = cvars.exec_cfg(&raw.text());
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
        cvars.set("g_gametype", &self.game.cfg.gametype);
        // `SV_SpawnServer`'s `Cvar_Set("mapname", ...)` (map-cycle doc,
        // section 3 step 15), which a script's `getCvar("mapname")` reads.
        cvars.set("mapname", &self.game.cfg.map);
        // Latched: a `set` while a level ran waits in the overrides.
        cvars.set(
            "sv_maxclients",
            &self.pending_cvar("sv_maxclients", &self.game.cfg.max_clients.to_string()),
        );
        cvars.set("dedicated", &self.game.dedicated.to_string());
        cvars.set("sv_running", "1");
        cvars.set("sv_serverid", &self.game.server_id.to_string());
        cvars.set("sv_cheats", if self.game.cheats { "1" } else { "0" });
        // `Com_StartupVariable` runs from `Com_Init` only, never from
        // `SV_SpawnServer`, so the command line's sets go in once, over
        // `default_mp.cfg`; past that a script's `setCvar` stands
        // (map-cycle doc 4.5).
        if self.game.carried_cvars.is_none() {
            cvars.set("sv_hostname", &self.game.cfg.hostname);
            for (name, value) in &self.game.cvar_overrides {
                cvars.set(name, value);
            }
        }
        if self.v15() {
            cvars.set_ui_cvars();
            // 1.5b's `map` and `map_restart` (0x80876aa, 0x8087777) put a
            // `timescale` at or under 0 back to 1, which is what ends a
            // match timeout a level boundary cut short.
            if crate::game::builtins::cvar::atof(cvars.get("timescale")) <= 0.0 {
                cvars.set("timescale", "1");
            }
        }
        cvars
    }

    /// The movement rules this server's players and bots move by.
    pub fn move_rules(&self) -> vcod_common::pmove::MoveRules {
        vcod_common::pmove::MoveRules::for_protocol(self.game.proto)
    }

    /// Test seam: the paks a download is served from, with no level load.
    /// Every pak in `fs` is listed in the systeminfo as if the level had
    /// read it, so a client checks a transfer against its checksum.
    pub fn test_mount_paks(&mut self, fs: Rc<vcod_common::pk3::Pk3Fs>) {
        self.game.paks = configstrings::PakLists::of(
            false,
            fs.paks_read().map(|(p, _)| (p, true)),
            self.game.proto,
            "",
        );
        self.game.fs = Some(fs);
        self.set_protocol(self.game.proto);
    }

    /// The protocol this server speaks: one per process, as each retail
    /// binary speaks one (docs/research/cod15-protocol-diff.md, "Server
    /// side"). Set before the first level loads.
    pub fn set_protocol(&mut self, p: &'static Protocol) {
        self.game.proto = p;
        self.core.transport = &p.transport;
        // `sv_master1` defaults to the binary's own master unless set.
        if !self
            .game
            .cvar_overrides
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("sv_master1"))
        {
            self.core.masters.set(0, p.transport.master().host);
        }
        self.core.masters.set_port(p.transport.master().port);
        if let Some(w) = &self.game.world {
            w.collision
                .set_arith(vcod_common::patch::Arith::for_protocol(p));
        }
        // The static slots move with the layout (UO's start time, winner
        // and hint strings).
        self.core.configstrings = configstrings::static_configstrings(
            &self.game.cfg,
            self.game.server_id,
            self.game.module.fall_heights,
            self.game.cheats,
            &self.game.paks,
            self.systeminfo_v6().as_deref(),
            p,
        );
        self.core.sent_configstrings = self.core.configstrings.clone();
    }

    pub fn protocol(&self) -> &'static Protocol {
        self.game.proto
    }

    /// Whether this server speaks 1.5 or United Offensive, whose engine and
    /// game module are 1.5's with UO's additions (coduo-diff.md, "Server").
    pub fn v15(&self) -> bool {
        !self.game.proto.is_v1()
    }

    /// The game module's tables for this protocol.
    pub fn defs(&self) -> &'static crate::defs::Defs {
        crate::defs::Defs::of(self.game.proto)
    }

    /// Configstring 1's 1.5 rows (`registry::SYSTEMINFO_V6`), `None` on a
    /// 1.1 server.
    fn systeminfo_v6(&self) -> Option<Vec<(String, String)>> {
        if !self.v15() {
            return None;
        }
        let fresh;
        let cvars = match self.game.module.script.as_ref() {
            Some(rt) => rt.cvars(),
            None => {
                let mut c = crate::cvars::Cvars::for_protocol(self.game.proto.transport.version);
                c.set_ui_cvars();
                fresh = c;
                &fresh
            }
        };
        Some(
            cvars
                .info_pairs(crate::cvars::flag::SYSTEMINFO)
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
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
    /// spawn, and the error path has to leave `self.core.configstrings` exactly as
    /// it was, so the error the caller reports is about the script rather
    /// than about a half-cleared table. `main.rs` exits on it.
    pub fn load_scripts(&mut self, fs: Rc<vcod_common::pk3::Pk3Fs>) -> anyhow::Result<()> {
        self.load_scripts_with(fs, false, crate::game::script::Carry::default(), false)?;
        // Configstring 0 from the cvar table the scripts now own, as the
        // first frame's flush would write it.
        self.flush_serverinfo();
        // The first map's `SV_SpawnServer` steps 20 and 21, as a map change
        // runs them.
        self.settle_frames();
        self.rebuild_baselines();
        Ok(())
    }

    /// The test seam for a gametype script that ships in no pak: `path` is a
    /// canonical script path (no extension) and `text` its source, answered
    /// instead of the paks on every later level load. `--gametype-script`
    /// uses it to run a client probe as the gametype, and the tests that need
    /// the real console and restart paths under a probe script call it
    /// directly.
    pub fn overlay_script(&mut self, path: &str, text: &str) {
        self.game.module.script_overlay = Some((path.to_string(), text.to_string()));
        // A restart reuses the running level's compile; new text needs a new one.
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.forget_compiled();
        }
    }

    /// Every line the running level's script has passed to `logPrint`.
    /// Test-facing: a new level starts a new log, so a caller spanning a
    /// restart collects this per level.
    pub fn script_log(&self) -> &[String] {
        self.game
            .module
            .script
            .as_ref()
            .map_or(&self.game.shutdown_log, |rt| rt.script_log())
    }

    /// Whether `killserver` or a `G_Error` drop has shut the level down.
    pub fn is_killed(&self) -> bool {
        self.game.killed
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
        let cvars = self.cvars(&*fs);
        self.game.level_cvars = Some((
            cvars.get("g_gametype").to_string(),
            cvars.get("sv_maxclients").to_string(),
        ));
        self.game.fs = Some(fs.clone());
        // `sv_pure`: retail's default 1, a `--set` over it, kept across levels.
        let pure = cvars.get("sv_pure").trim().parse::<i32>().unwrap_or(0) != 0;
        if let Some(cs0) = self.core.configstrings.get_mut(0) {
            *cs0 = configstrings::with_sv_pure(cs0, pure);
        }
        self.game.pure_check = Some(PureCheck::new(
            &fs,
            self.core.checksum_feed,
            self.game.proto,
        ));
        if !restart {
            self.game.module.anims = match vcod_common::animtree::PlayerAnims::load(&*fs) {
                Ok(a) => Some(Rc::new(a)),
                Err(e) => {
                    log::warn!("player anims: {e:#}, players will not animate");
                    None
                }
            };
            self.game.module.weapon_table =
                Rc::new(crate::weapons::WeaponTable::load(&fs, self.defs()));
            self.game.module.hitlocs = crate::game::combat::HitLocTable::load(&fs);
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
            map: carry.map.filter(|_| restart),
            scripts: carry.scripts.filter(|_| restart),
            models: carry.models.filter(|_| restart),
            gametype: carry.gametype,
            seed: Some(self.game.module.level_seed),
            spawn_pins: self.game.module.spawn_pins.clone(),
        };
        self.game.module.level_seed =
            vcod_common::rng::xorshift(&mut self.game.module.level_seed) | 1;
        // `SV_MapRestart`'s `G_ShutdownGame(1)`: the running level's log
        // closes ahead of the new one's `G_InitGame`.
        self.close_game_log(if restart {
            crate::engine::gamelog::RESTART
        } else {
            crate::engine::gamelog::SHUTDOWN
        });
        let fs_paks = fs.clone();
        let source = crate::game::script::PakScripts::new(
            fs.clone(),
            self.game.module.script_overlay.clone(),
        );
        let mut rt = crate::game::script::ScriptRuntime::load_from(
            Box::new(source),
            fs,
            &self.game.cfg.map,
            &self.game.cfg.gametype,
            self.core.configstrings.clone(),
            cvars,
            self.game.world.clone(),
            self.game.module.weapon_table.clone(),
            self.core.sv_time_ms,
            carry,
        )?;
        rt.set_player_anims(self.game.module.anims.clone());
        // `SV_SpawnServer` step 23, once the level has read its files; a
        // restart keeps the lists, as `SV_MapRestart` never sets them.
        if !restart {
            self.game.paks = self.level_paks(&fs_paks, &rt, pure);
        }
        let mut configstrings = rt.configstrings().to_vec();
        rt.cvars()
            .write_mirror(&mut configstrings)
            .map_err(|e| anyhow::anyhow!("writing the cvar mirror: {e:?}"))?;
        self.core.configstrings = configstrings;
        // `G_InitGame`'s open, at the new `level.time` (cod11-map-cycle.md 9).
        let game = if rt.cvars().get("fs_game").is_empty() {
            fs_paks
                .dirs()
                .last()
                .and_then(|d| d.file_name())
                .map_or("main".to_string(), |n| n.to_string_lossy().into_owned())
        } else {
            rt.cvars().get("fs_game").to_string()
        };
        let mut info = Info::new();
        for (k, v) in rt.cvars().info_pairs(crate::cvars::flag::SERVERINFO) {
            info.set(k, v);
        }
        info.set("sv_pure", u8::from(self.game.paks.pure));
        let info = info.to_string();
        self.game
            .game_log
            .init(rt.cvars(), &game, &info, self.core.sv_time_ms);
        self.game.module.script = Some(rt);
        self.refresh_fall_heights();
        self.sync_sent_configstrings();
        // `SV_SpawnServer` and `SV_MapRestart` both clear it, the flag
        // included, and the new level's `main` turns it back on.
        self.game.archive.clear();
        Ok(())
    }

    /// The four pak lists for a level `rt` has just loaded off `fs`
    /// (cod11-server-handshake.md, "The four cvars").
    fn level_paks(
        &self,
        fs: &vcod_common::pk3::Pk3Fs,
        rt: &crate::game::script::ScriptRuntime,
        pure: bool,
    ) -> configstrings::PakLists {
        use vcod_common::net::protocol::PROTOCOL_V6;
        // 1.1 lists every pak, read or not.
        if !self.game.proto.is_v1() {
            // `G_ModelIndex` precaches each model as it indexes it, which
            // reads the descriptor and LOD 0's `xmodelparts`; vcod reads
            // them when it needs them, so the reads that flag their paks
            // are made here.
            let (first, last) = rt
                .host
                .allocators
                .layout()
                .bounds(configstrings::CsRange::Model);
            for name in &rt.configstrings()[first..=last] {
                if let Some(name) = name.strip_prefix("xmodel/") {
                    let _ = vcod_common::model::load_bones(fs, name);
                }
            }
            // So does the load's sound alias pass: every `soundaliases/*.csv`
            // on 1.5; on UO the map's sound load spec, or the default one
            // when the map ships none, whose aliases UO leaves unflagged.
            if self.game.proto.transport.version == PROTOCOL_V6.transport.version {
                for name in fs.list_prefix("soundaliases/") {
                    if name.ends_with(".csv") {
                        fs.touch(&name);
                    }
                }
            } else {
                let spec = format!("soundloadspecs/mp/{}.csv", self.game.cfg.map);
                if fs.contains(&spec) {
                    fs.touch(&spec);
                } else {
                    fs.touch("soundloadspecs/mp/default.csv");
                }
            }
            // And the script load reads each `#using_animtree`'s tree.
            for tree in rt.animtrees() {
                fs.touch(&format!("animtrees/{tree}.atr"));
            }
        }
        let cvars = rt.cvars();
        let mut paks = configstrings::PakLists::of(
            pure,
            fs.paks_read(),
            self.game.proto,
            cvars.get("fs_basegame"),
        );
        paks.fs_game = cvars.get("fs_game").to_string();
        paks
    }

    /// Every script thread that has died of an error, rendered
    /// `file::func:line: Kind`. Empty is the healthy reading: a thread that
    /// aborts stops running, silently as far as the wire is concerned, so
    /// this is what a test checks a clean bootstrap against.
    pub fn script_aborts(&self) -> Vec<String> {
        self.game
            .module
            .script
            .as_ref()
            .map_or_else(Vec::new, |rt| rt.aborts())
    }

    /// One field off a client's script entity, rendered as text. `None` when
    /// no script is loaded or the slot holds no client entity. This is the
    /// only way into a joined client's script state from outside the crate,
    /// and the tests that assert what the stock scripts left there are its
    /// only callers.
    pub fn client_field(&mut self, slot: usize, name: &str) -> Option<String> {
        self.game.module.script.as_mut()?.client_field(slot, name)
    }

    /// One key out of a client's `.pers`, rendered the same way.
    pub fn client_pers(&mut self, slot: usize, key: &str) -> Option<String> {
        self.game.module.script.as_mut()?.client_pers(slot, key)
    }

    /// One cvar as the running script reads it. `None` before
    /// `load_scripts`. Test-facing: `cfg.gametype` picks which gametype
    /// script loads and this table is what that script's own `getCvar`
    /// answers from, so a rotation that wrote only one of the two is
    /// invisible everywhere else.
    pub fn script_cvar(&self, name: &str) -> Option<String> {
        Some(
            self.game
                .module
                .script
                .as_ref()?
                .cvars()
                .get(name)
                .to_string(),
        )
    }

    /// `Cvar_Set`: before the first load a `+set` for it to stamp
    /// (`Com_StartupVariable`), after it a write to the running table, or to
    /// the carried one between levels. `g_gametype` and `sv_maxclients` are
    /// latched and wait for the next load. `sv_mapRotation` and `g_gametype`
    /// also mirror into their own fields, so `--set` and `map_rotate`'s
    /// `gametype` token both work through one value
    /// (docs/research/cod11-map-cycle.md section 5).
    pub fn set_cvar(&mut self, name: &str, value: &str) {
        // 1.5b registers `sv_punkbuster` read-only ahead of the command
        // line's `+set`s, so the server runs without PunkBuster
        // (cod15-protocol-diff.md, "PunkBuster").
        if self.v15() && name.eq_ignore_ascii_case("sv_punkbuster") {
            log::info!("{name} is read only.");
            return;
        }
        // In place when the name is already there, so the last write wins
        // whether it came from `--set` or from a rotation token, and a
        // rotation running for days does not grow the list a map at a time.
        match self.game.cvar_overrides.iter_mut().find(|(n, _)| n == name) {
            Some((_, v)) => *v = value.to_string(),
            None => self
                .game
                .cvar_overrides
                .push((name.to_string(), value.to_string())),
        }
        let latched =
            name.eq_ignore_ascii_case("g_gametype") || name.eq_ignore_ascii_case("sv_maxclients");
        if !latched {
            if let Some(rt) = self.game.module.script.as_mut() {
                rt.host.cvars.set(name, value);
            } else if let Some(c) = self.game.carried_cvars.as_mut() {
                c.set(name, value);
            }
        }
        self.apply_engine_cvar(name, value);
        // Latched: serverinfo keeps the running level's value until a load,
        // and takes this one at once only while no level has loaded.
        if name == "g_gametype" {
            self.game.cfg.gametype = value.to_string();
            self.refresh_serverinfo();
        }
    }

    /// The engine cvars `Server` keeps in its own fields. Names fold case.
    fn apply_engine_cvar(&mut self, name: &str, value: &str) {
        let lower = name.to_ascii_lowercase();
        if let Some(i) = crate::master::Masters::index(&lower) {
            self.core.masters.set(i, value);
        }
        match lower.as_str() {
            "dedicated" => self.game.dedicated = value.trim().parse().unwrap_or(0),
            "rconpassword" => self.game.rcon_password = value.to_string(),
            "sv_maprotation" => self.game.sv_map_rotation = value.to_string(),
            "sv_hostname" => {
                self.game.cfg.hostname = value.to_string();
                self.refresh_serverinfo();
            }
            "sv_privateclients" => self.refresh_serverinfo(),
            _ => {}
        }
    }

    /// `Cvar_InfoString(CVAR_SERVERINFO)`: the running level's table, so a
    /// script `setCvar` of a serverinfo cvar shows as a console write does.
    /// With no level, the config and the `--set`s the first load stamps.
    /// `sv_pure` is the one the paks were listed under.
    fn serverinfo(&self) -> Info {
        let mut i = match self.game.module.script.as_ref() {
            Some(rt) => {
                let mut i = Info::new();
                for (k, v) in rt.cvars().info_pairs(crate::cvars::flag::SERVERINFO) {
                    i.set(k, v);
                }
                i
            }
            None => {
                let mut i = configstrings::serverinfo(&self.live_cfg());
                i.set("sv_privateClients", self.private_clients())
                    .set("protocol", self.game.proto.transport.version)
                    .set("shortversion", self.game.proto.transport.short_version);
                i
            }
        };
        i.set("sv_pure", u8::from(self.game.paks.pure));
        i
    }

    /// `SV_Frame`'s `cvar_modifiedFlags & CVAR_SERVERINFO` test, as a
    /// compare: whatever wrote a serverinfo cvar this frame, configstring 0
    /// follows before the snapshots go out.
    fn flush_serverinfo(&mut self) {
        if self.core.configstrings.first() != Some(&self.serverinfo().to_string()) {
            self.refresh_serverinfo();
        }
    }

    /// `SV_Frame`'s cvar flush, the serverinfo half: a write to a cvar the
    /// serverinfo string carries is followed by
    /// `SV_SetConfigstring(0, Cvar_InfoString(CVAR_SERVERINFO))`
    /// (docs/research/cod11-map-cycle.md, 4.3). Both tables, because the
    /// script owns its own copy between level loads and `tick` reads that one
    /// back over this one every frame.
    fn refresh_serverinfo(&mut self) {
        let info = self.serverinfo().to_string();
        if let Some(slot) = self.core.configstrings.get_mut(0) {
            *slot = info.clone();
        }
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.configstrings.set(0, info);
        }
    }

    /// `G_UpdateCvars` for the two fall bounds and `SV_Frame`'s systeminfo
    /// flush behind them: the moves read the values the script's table holds
    /// now, and slot 1 carries them to the clients that predict with them.
    /// Both tables, as [`Self::refresh_serverinfo`] writes both.
    fn refresh_fall_heights(&mut self) {
        use crate::game::builtins::cvar::atof;
        let v6 = self.systeminfo_v6();
        let Some(rt) = self.game.module.script.as_mut() else {
            return;
        };
        let cvars = rt.cvars();
        self.game.module.fall_heights = FallHeights {
            min: atof(cvars.get("bg_fallDamageMinHeight")),
            max: atof(cvars.get("bg_fallDamageMaxHeight")),
        };
        let info = configstrings::systeminfo(
            self.game.server_id,
            self.game.module.fall_heights,
            self.game.cheats,
            &self.game.paks,
            v6.as_deref(),
        )
        .to_string();
        rt.host.configstrings.set(1, info.clone());
        if let Some(slot) = self.core.configstrings.get_mut(1) {
            *slot = info;
        }
    }

    /// What the next level load would stamp for `name`, given the config's
    /// own value: a `+set` override if one names it, since [`Self::cvars`]
    /// replays the override list last.
    fn pending_cvar(&self, name: &str, from_cfg: &str) -> String {
        self.game
            .cvar_overrides
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
            .game
            .fs
            .clone()
            .ok_or_else(|| kept(anyhow::anyhow!("no paks are mounted")))?;
        // Step 15's `CM_LoadMap`, pulled ahead of the teardown, and with it
        // step 9's `FS_ClearPakReferences`.
        fs.clear_references();
        let bsp = vcod_common::mapworld::find(&*fs, map).map_err(kept)?;

        // Step 2: the flag is read off the outgoing level and handed to the
        // incoming one's `G_InitGame`, along with the two tables it decides
        // the fate of.
        let (save_persist, carry) = self.lift_persistence();
        // Step 3: every client whose state is `CS_PRIMED` or above, which is
        // every one that has a gamestate to be told is stale.
        let text = format!("loadingnewmap\n{map}\n{}", self.game.cfg.gametype);
        let told: Vec<SocketAddr> = self
            .core
            .clients
            .iter()
            .flatten()
            .filter(|c| c.state != ClientState::Connected)
            .map(|c| c.link.addr)
            .collect();
        for addr in told {
            self.send_oob(addr, &text);
        }
        // Step 5: the game module is unloaded, its object table with it, and
        // step 8's `memset(&sv, 0, ...)` takes everything the level queued.
        self.shutdown_game_log();
        self.game.module.script = None;
        self.game.module.pending_explosions.clear();
        self.game.module.weapon_changes.clear();
        // Step 9.
        self.core.checksum_feed = (self.rand() << 16) ^ self.rand() ^ self.core.sv_time_ms;
        // Step 13.
        self.game.snap_flag_server_bit ^= console::SNAPFLAG_SERVERCOUNT;
        // Step 15's `Cvar_Set("mapname", ...)` and the collision with it.
        self.game.cfg.map = map.to_string();
        self.load_world(World::from_bsp(&bsp, Some(&fs)));
        // Step 16, then steps 7 and 10 with step 24 folded in: the table is
        // rebuilt empty around the serverinfo and systeminfo the new id
        // belongs in, which is where the client reads the id back from.
        self.game.server_id = console::next_map_id(self.game.server_id);
        self.core.configstrings = configstrings::static_configstrings(
            &self.game.cfg,
            self.game.server_id,
            self.game.module.fall_heights,
            self.game.cheats,
            &self.game.paks,
            self.systeminfo_v6().as_deref(),
            self.game.proto,
        );
        self.core.configstrings[CS_SERVERINFO] = self.serverinfo().to_string();
        // Step 19. Past the teardown: a failure here leaves no level.
        self.load_scripts_with(fs, false, carry, save_persist)
            .map_err(LoadFailure::Fatal)?;
        // Step 20. Every client here is `CS_CONNECTED` and pulls the whole
        // table in the gamestate it asks for, so the diff has nothing to say
        // about a level boundary (the restart path re-syncs the same way).
        self.settle_frames();
        // Step 21.
        self.rebuild_baselines();
        // Step 22, after the settle frames and the baselines: `ClientConnect`
        // again for everyone still on a netchan, then back to `CS_CONNECTED`.
        // A denial drops the client (0x808a76f).
        for slot in 0..self.core.clients.len() {
            let Some(c) = self.core.clients[slot].as_ref() else {
                continue;
            };
            if self.password_denied(&c.link.userinfo, c.is_bot) {
                self.drop_client(slot, "GAME_INVALIDPASSWORD");
                continue;
            }
            let Some(c) = self.core.clients[slot].as_mut() else {
                continue;
            };
            let name = c.name.clone();
            c.reset_for_level();
            if let Some(rt) = self.game.module.script.as_mut() {
                rt.reconnect_client(slot, name);
            }
        }
        // Step 24 proper: configstring 0 again from the table the scripts
        // and settle frames left, so a `setCvar` of a serverinfo cvar in the
        // level's init is in the gamestate.
        self.flush_serverinfo();
        self.game.last_spawn_tick = Some(self.svs_time_ms());
        log::info!(
            "map {map} ({}), serverId {:#04x}, {} clients kept",
            self.game.cfg.gametype,
            self.game.server_id,
            self.client_count()
        );
        // The masters hear of the new map on the next frame (0x808a915).
        self.core.masters.force();
        Ok(())
    }

    /// What the outgoing level asked to keep and the two tables it names
    /// (map-cycle doc, section 1). Also stashes its cvar table, which is
    /// process-global on retail and outlives both boundaries whatever
    /// `savePersist` says. `false` and an empty carry when no level is
    /// running.
    fn lift_persistence(&mut self) -> (bool, crate::game::script::Carry) {
        let Some(rt) = self.game.module.script.as_ref() else {
            return (false, crate::game::script::Carry::default());
        };
        self.game.carried_cvars = Some(rt.cvars().clone());
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
        if self.game.last_spawn_tick == Some(self.svs_time_ms()) {
            return Ok(());
        }
        // Step 2.
        if self.game.module.script.is_none() {
            log::info!("Server is not running.");
            return Ok(());
        }
        let Some(fs) = self.game.fs.clone() else {
            return Err(LoadFailure::KeptLevel(anyhow::anyhow!(
                "no paks are mounted"
            )));
        };
        // Step 3: the escalation. A level that asked to keep its
        // persistence gets the in-place restart even across one of these.
        let save_persist = self
            .game
            .module
            .script
            .as_ref()
            .is_some_and(|rt| rt.host.save_persist);
        if !save_persist {
            let changed = self
                .game
                .level_cvars
                .as_ref()
                .and_then(|(gametype, max_clients)| {
                    if *gametype != self.pending_cvar("g_gametype", &self.game.cfg.gametype) {
                        Some("g_gametype")
                    } else if *max_clients
                        != self
                            .pending_cvar("sv_maxclients", &self.game.cfg.max_clients.to_string())
                    {
                        Some("sv_maxclients")
                    } else {
                        None
                    }
                });
            if let Some(name) = changed {
                log::info!("{name} variable change -- restarting.");
                let map = self.game.cfg.map.clone();
                return self.spawn_server(&map);
            }
        }
        let (_, carry) = self.lift_persistence();
        // What every client holds of the table, which the incoming level's
        // writes are measured against below.
        let held = self.core.sent_configstrings.clone();
        // Step 4's six counters: what this level queued and nothing else,
        // since a restart keeps the map and its collision.
        self.game.module.pending_explosions.clear();
        self.game.module.weapon_changes.clear();
        self.game.snap_flag_server_bit ^= console::SNAPFLAG_SERVERCOUNT;
        // Step 5: only the low nibble moves.
        self.game.server_id = console::next_restart_id(self.game.server_id);
        // `sv.core.configstrings` survives a restart: only a spawn clears it, so
        // every slot the outgoing level allocated keeps its index and its
        // text, and the incoming level's `G_ModelIndex` finds the same slot
        // (map-cycle doc, 4.6). The static slots are rebuilt around the new
        // id on top; 4.3 is what carries slot 1 to a client that already
        // has a gamestate.
        for (i, s) in configstrings::static_configstrings(
            &self.game.cfg,
            self.game.server_id,
            self.game.module.fall_heights,
            self.game.cheats,
            &self.game.paks,
            self.systeminfo_v6().as_deref(),
            self.game.proto,
        )
        .into_iter()
        .enumerate()
        {
            if !s.is_empty() {
                self.core.configstrings[i] = s;
            }
        }
        self.core.configstrings[CS_SERVERINFO] = self.serverinfo().to_string();
        // Step 7: `SV_RestartGameProgs(savePersist)`. Past the teardown: the
        // outgoing level's script is gone and a failure here leaves none.
        self.print(&restart_game_banner(self.game.proto));
        let p = crate::tick_profile::phase("console/map_restart/load");
        self.load_scripts_with(fs, true, carry, save_persist)
            .map_err(LoadFailure::Fatal)?;
        drop(p);
        // Step 8. The incoming level's whole table goes into the diff's copy
        // in one go: what a client learns of it is the burst below, measured
        // against `held` (doc 4.5).
        let p = crate::tick_profile::phase("console/map_restart/settle");
        self.settle_frames();
        drop(p);
        let reenter = crate::tick_profile::phase("console/map_restart/reenter");
        // The incoming level's writes, ahead of the `n`: `sv.restarting`
        // lets `SV_SetConfigstring` broadcast every write that changes its
        // slot, and retail's settle frames flush them before step 9 does
        // (`tests/fixtures/netchan/mp_carentan-dm-mapchange.txt`, seq 40-42;
        // a team score the outgoing level's `ExitLevel` zeroed, coduo-diff.md
        // 22). The game's writes go in their order (`ConfigStrings::set`,
        // from the load through the settle frames), then what the engine
        // itself left different: the cvar mirror, in slot order. The
        // queue's squash keeps each slot's last `d` at its last position, so
        // the fog `G_setfog("0")` cleared and the map set again still goes
        // out. Slots 0 and 1 are the frame's cvar flush (4.3).
        let game_writes = self
            .game
            .module
            .script
            .as_mut()
            .map(|rt| rt.host.configstrings.take_writes())
            .unwrap_or_default();
        let mut table = held;
        let mut writes: Vec<(usize, String)> = Vec::new();
        for (i, v) in game_writes {
            if i >= 2 && table.get(i) != Some(&v) {
                table[i].clone_from(&v);
                writes.push((i, v));
            }
        }
        for i in 2..self.core.configstrings.len() {
            if table.get(i) != self.core.configstrings.get(i) {
                writes.push((i, self.core.configstrings[i].clone()));
            }
        }
        for slot in 0..self.core.clients.len() {
            if self.core.clients[slot].is_some() {
                for (i, v) in &writes {
                    for cmd in vcod_common::net::configstring_commands(*i, v) {
                        self.send_server_command(slot, &cmd);
                    }
                }
            }
        }
        // Steps 9 to 11: `n`, `ClientConnect` again, and back into the world
        // for a client that was already in it.
        for slot in 0..self.core.clients.len() {
            let Some(c) = self.core.clients[slot].as_mut() else {
                continue;
            };
            let was_active = c.state == ClientState::Active;
            let name = c.name.clone();
            let entering = c.last_cmd;
            c.reset_for_restart();
            // Through the guarded path, like every other reliable: a client
            // whose acks have fallen a ring behind is dropped rather than
            // handed an overwritten slot.
            // 1.5's and UO's `SV_MapRestart_f` send `n` for a restart that
            // keeps `pers[]` and `B` for one that does not (coduo_lnxded
            // 0x8088325, the letters at 0x80e35b1/0x80e35b3; CoDMP15.exe's
            // at 0x59df40/0x59df44); 1.1 sends `n` either way.
            let restart = if save_persist {
                "n"
            } else {
                self.game.proto.map_restart_command
            };
            self.send_server_command(slot, restart);
            // That guard can drop the slot; a client that is gone has no
            // `ClientConnect` to run and no world to enter.
            let Some(c) = self.core.clients[slot].as_ref() else {
                continue;
            };
            // A denial drops the client (0x8083f99).
            if self.password_denied(&c.link.userinfo, c.is_bot) {
                self.drop_client(slot, "GAME_INVALIDPASSWORD");
                self.print(&format!(
                    "SV_MapRestart_f: dropped client {slot} - denied!\n"
                ));
                continue;
            }
            if let Some(rt) = self.game.module.script.as_mut() {
                rt.reconnect_client(slot, name);
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
        for slot in 0..self.core.clients.len() {
            if self.core.clients[slot].is_some() {
                self.send_configstring_update(slot, 1);
            }
        }
        drop(reenter);
        self.game.last_spawn_tick = Some(self.svs_time_ms());
        log::info!(
            "map_restart {} ({}), serverId {:#04x}, {} clients kept",
            self.game.cfg.map,
            self.game.cfg.gametype,
            self.game.server_id,
            self.client_count()
        );
        Ok(())
    }

    /// The three settle frames a load or a restart runs, 100 ms of
    /// `svs.time` each (map-cycle doc, section 3 step 20, section 4 step 8),
    /// then what they allocated read back into the configstring table and
    /// the diff's copy of it.
    fn settle_frames(&mut self) {
        for _ in 0..3 {
            self.core.sv_time_ms = self.core.sv_time_ms.wrapping_add(100);
            if let Some(rt) = self.game.module.script.as_mut() {
                rt.run_frame(self.core.sv_time_ms);
                // The vehicles' first think (`Scr_Vehicle_Init`, spawn + 50)
                // settles them before the baselines.
                vehicle_frame(&mut self.core.clients, rt);
            }
        }
        if let Some(rt) = self.game.module.script.as_ref() {
            self.core.configstrings = rt.configstrings().to_vec();
            if let Err(e) = rt.cvars().write_mirror(&mut self.core.configstrings) {
                log::warn!("rebuilding the cvar mirror: {e:?}");
            }
        }
        self.refresh_fall_heights();
        self.sync_sent_configstrings();
    }

    /// `SV_CreateBaseline` (map-cycle doc, section 3 step 21): every entity
    /// linked after the load, the `SVF_NOCLIENT` ones included (3.3).
    fn rebuild_baselines(&mut self) {
        let proto = self.game.proto;
        let mut baselines: HashMap<u32, msg::EntityState> = self
            .game
            .module
            .script
            .as_mut()
            .map_or_else(HashMap::new, |rt| {
                rt.baseline_entities(proto).into_iter().collect()
            });
        if let Some(te) = self.game.module.test_entities.as_ref() {
            baselines.extend(te.baselines(proto));
        }
        for es in baselines.values_mut() {
            crate::game::wire::entity_to_protocol(proto, es);
        }
        self.game.baselines = baselines;
    }

    /// `SV_SetConfigstring`'s per-client half, the `d <index> <text>` server
    /// command. Two callers: the restart's slot 1, sent to a client that
    /// keeps its gamestate after the burst of the incoming level's writes
    /// (map-cycle doc, 4.3 and 4.5), and
    /// [`Self::broadcast_configstring_changes`] once a frame. A map change
    /// reaches neither: it re-syncs the table after its settle frames, so the
    /// diff has nothing to say about the boundary, and every client it leaves
    /// behind is `CS_CONNECTED`, which the diff skips. That is what keeps a
    /// map change off the reliable stream entirely, the gamestate each client
    /// pulls carrying the whole table instead (3.1).
    ///
    /// A value of 1000 bytes or more goes as `x`, `y`..., `z` chunks of 999
    /// (cod11-server-handshake.md, "Configstring 1, systeminfo").
    fn send_configstring_update(&mut self, slot: usize, index: usize) {
        for cmd in vcod_common::net::configstring_commands(index, self.configstring(index)) {
            self.send_server_command(slot, &cmd);
        }
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
    ///
    /// The game's slots go in the order of their last write
    /// (`ConfigStrings::writes`), as retail queues each `d` at its write;
    /// the rest (the cvar mirror) after them, in slot order.
    fn broadcast_configstring_changes(&mut self) {
        let mut changed: Vec<usize> = (0..self.core.configstrings.len())
            .filter(|&i| self.core.sent_configstrings.get(i) != self.core.configstrings.get(i))
            .collect();
        if let Some(rt) = self.game.module.script.as_ref() {
            let writes = rt.host.configstrings.writes();
            changed.sort_by_key(|&i| {
                writes
                    .iter()
                    .rposition(|(w, _)| *w == i)
                    .unwrap_or(usize::MAX)
            });
        }
        if changed.is_empty() {
            return;
        }
        self.sync_sent_configstrings();
        for slot in 0..self.core.clients.len() {
            let has_table = self.core.clients[slot]
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
        self.core.sent_configstrings = self.core.configstrings.clone();
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
                self.game.fatal = Some(e.context(what.to_string()));
                false
            }
        }
    }

    /// The load failure that left the server with no level, taken. The binary
    /// polls it after every tick and ends the process on it, which is what
    /// retail's `Com_Error` does: nothing can call `exitLevel` any more and
    /// every client would pull an empty gamestate.
    pub fn take_fatal(&mut self) -> Option<anyhow::Error> {
        self.game.fatal.take()
    }

    /// A command-line `+set` placed after `+map`: `Com_AddStartupCommands`
    /// queues it behind the map, so it runs as a console `set` once the
    /// level has loaded, over the script's load-time writes (map-cycle doc
    /// 4.5).
    pub fn push_set(&mut self, name: &str, value: &str) {
        self.push_console(&format!("set {name} \"{value}\""));
    }

    /// `Cbuf_Execute`: every queued line through [`Self::exec_console_line`].
    /// A command pushed by a builtin during the script pass runs here, at the
    /// top of the next tick, which is `EXEC_APPEND`. The frame loop runs it
    /// ahead of [`crate::engine::tick`], as `Com_Frame` runs it ahead of
    /// `SV_Frame`, so the time a command took is owed by the frames after it.
    pub fn drain_console(&mut self, now: Instant) {
        while let Some(text) = self.core.console.pop_front() {
            let mut lines = console::split_commands(&text).into_iter();
            let Some(line) = lines.next() else {
                continue;
            };
            for rest in lines.rev() {
                self.core.console.push_front(rest);
            }
            if !self.exec_console_line(&line, now) || self.core.quit {
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
                    .game
                    .fs
                    .as_ref()
                    .is_some_and(|fs| fs.resolve_map(&map).is_none())
                {
                    self.print(&format!("Can't find map {bsp}\n"));
                    return true;
                }
                // `map <the map already serving>` is a restart, not a
                // spawn (doc section 4.2).
                let r = if map.eq_ignore_ascii_case(&self.game.cfg.map)
                    && self.game.module.script.is_some()
                {
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
                if self.game.module.script.is_some() {
                    self.game.killed = false;
                }
            }
            console::Command::MapRestart => {
                let _p = crate::tick_profile::phase("console/map_restart");
                let r = self.map_restart();
                return self.absorb_load("map_restart", r);
            }
            console::Command::MapRotate => {
                // Both rotation cvars are the one table a script's
                // `setCvar` and `getCvar` reach on retail (doc section 5.2).
                let mut full = self.game.sv_map_rotation.clone();
                if let Some(rt) = self.game.module.script.as_ref() {
                    let script = rt.host.cvars.get("sv_mapRotation");
                    if !script.is_empty() {
                        full = script.to_string();
                    }
                    self.game.rotation.current =
                        rt.host.cvars.get("sv_mapRotationCurrent").to_string();
                }
                let before = self.game.cfg.gametype.clone();
                let mut gametype = before.clone();
                let next = self.game.rotation.rotate(&full, &mut gametype);
                if let Some(rt) = self.game.module.script.as_mut() {
                    rt.host
                        .cvars
                        .set("sv_mapRotationCurrent", &self.game.rotation.current);
                }
                for w in self.game.rotation.warnings.drain(..) {
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
                    if let Some(rt) = self.game.module.script.as_mut() {
                        rt.host.save_persist = false;
                    }
                }
                match next {
                    console::Rotate::Map(m) => self.core.console.push_front(format!("map {m}")),
                    console::Rotate::Restart => self.core.console.push_front("map_restart".into()),
                }
            }
            console::Command::Status => {
                let text = self.status_text(now);
                self.print(&text);
            }
            console::Command::ClientKick(arg) => self.client_kick(arg.as_deref(), now),
            console::Command::Kick(arg) => self.kick(arg.as_deref(), now),
            console::Command::AddBot(args) => bot_admin::add_bot_command(self, &args, &mut ()),
            console::Command::RemoveBot(arg) => {
                bot_admin::remove_bot_command(self, arg.as_deref(), &mut ())
            }
            console::Command::ListBots => bot_admin::list_bots(self, &mut ()),
            console::Command::DumpUser(arg) => self.dump_user(arg.as_deref()),
            console::Command::ServerInfo => {
                let info = self.core.configstrings.first().cloned().unwrap_or_default();
                self.print(&format!(
                    "Server info settings:\n{}",
                    console::info_print(&info)
                ));
            }
            console::Command::SystemInfo => {
                let info = self.core.configstrings.get(1).cloned().unwrap_or_default();
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
                    && let Some(rt) = self.game.module.script.as_mut()
                {
                    rt.host.cvars.add_flags(&name, crate::cvars::flag::ARCHIVE);
                }
            }
            console::Command::Heartbeat => self.core.masters.force(),
            console::Command::GameCompleteStatus(arg) => self.game_complete_status(&arg),
            console::Command::ScriptUsage => {
                if let Some(text) = self.game.module.script.as_mut().map(|rt| rt.script_usage()) {
                    self.print(&text);
                }
            }
            console::Command::StringUsage => {
                if let Some(text) = self.game.module.script.as_ref().map(|rt| rt.string_usage()) {
                    self.print(&text);
                }
            }
            console::Command::Quit => {
                self.final_message("EXE_SERVERQUIT");
                self.shutdown_game_log();
                let mut resolve = self.core.resolver;
                let beats = self.core.masters.shutdown(
                    self.game.dedicated,
                    self.svs_time_ms(),
                    &mut resolve,
                );
                self.core.outbox.extend(beats);
                self.core.quit = true;
            }
            console::Command::KillServer => self.kill_server(),
            console::Command::BanUser(arg) => {
                let Some(arg) = arg else {
                    self.print("Usage: banUser <player name>\n");
                    return true;
                };
                if let Some(slot) = self.player_by_name(&arg) {
                    self.ban_slot(slot, now);
                }
            }
            console::Command::BanClient(arg) => {
                let Some(arg) = arg else {
                    self.print("Usage: banClient <client number>\n");
                    return true;
                };
                if let Some(slot) = self.player_by_num(&arg) {
                    self.ban_slot(slot, now);
                }
            }
            console::Command::Unban(arg) if self.v15() => {
                let Some(arg) = arg else {
                    self.print("Usage: unban <client name>\n");
                    return true;
                };
                let name = crate::game::say::clean_name(&arg);
                match self.core.bans.unban(&name) {
                    Some(0) => self.print(&format!("no banned user has name {name}\n")),
                    Some(n) => self.print(&format!("unbanned {n} user(s) named {name}\n")),
                    None => {}
                }
            }
            console::Command::CvarList(filter) => {
                let text = match self.game.module.script.as_ref() {
                    Some(rt) => rt.cvars().list(filter.as_deref()),
                    None => self
                        .game
                        .carried_cvars
                        .clone()
                        .unwrap_or_default()
                        .list(filter.as_deref()),
                };
                self.print(&text);
            }
            console::Command::TempBan { by_num, arg } if self.v15() => {
                let Some(arg) = arg else {
                    self.print(if by_num {
                        "Usage: tempBanClient <client number>\n"
                    } else {
                        "Usage: tempBanUser <player name>\n"
                    });
                    return true;
                };
                let slot = if by_num {
                    self.player_by_num(&arg)
                } else {
                    self.player_by_name(&arg)
                };
                if let Some(slot) = slot {
                    self.temp_ban_slot(slot, now);
                }
            }
            console::Command::SetScr { word, name, value } if self.v15() => {
                let Some(value) = value else {
                    self.print(&format!("Usage: {word} <integer>\n"));
                    return true;
                };
                for cvar in [format!("scr_{name}"), format!("ui_{name}")] {
                    self.set_cvar(&cvar, &value);
                    if let Some(rt) = self.game.module.script.as_mut() {
                        rt.host.cvars.set(&cvar, &value);
                    }
                }
            }
            console::Command::TempBan { .. }
            | console::Command::SetScr { .. }
            | console::Command::Unban(_) => {
                log::warn!("console: unknown command {line:?}");
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
        if n >= self.core.clients.len() {
            self.print(&format!(
                "Bad client slot: {}\n",
                arg.parse::<i64>().unwrap_or(-1)
            ));
            return None;
        }
        if self.core.clients[n].is_none() && self.game.zombies[n].is_none() {
            self.print(&format!("Client {n} is not active\n"));
            return None;
        }
        Some(n)
    }

    /// The tail both bans share (0x8084394, 0x8084524). Retail 1.1 sends
    /// `banUser <ip>` to the authorize server and leaves the client
    /// connected; vcod keeps the client's guid in `ban.txt` instead, where
    /// 1.5 keeps its own (docs/research/cod11-server-handshake.md, "Bans").
    fn ban_slot(&mut self, slot: usize, now: Instant) {
        let Some(c) = self.core.clients[slot]
            .as_ref()
            .or(self.game.zombies[slot].as_ref())
        else {
            return;
        };
        let name = if self.core.clients[slot].is_some() {
            c.name.clone()
        } else {
            String::new()
        };
        let guid = guid_of(crate::engine::Lan::Cod1, c.link.addr);
        if self.v15() {
            self.ban_slot_v15(slot, guid, &name, now);
            return;
        }
        // A LAN client's guid is 0, which no check refuses, as the
        // authorize server is never asked about one.
        if guid != 0 && !self.core.bans.is_banned(guid) {
            self.core
                .bans
                .append(guid, &crate::game::say::clean_name(&name));
        }
        self.print(&format!("{name} was banned from coming back\n"));
    }

    /// 1.5's ban tail (cod_lnxded 1.5b 0x8089121): nothing for a client
    /// without a guid or one already in `ban.txt`, else the guid and the
    /// cleaned name appended and the client kicked, with no print
    /// (docs/research/cod11-server-handshake.md, "1.5 permanent bans").
    fn ban_slot_v15(&mut self, slot: usize, guid: i32, name: &str, now: Instant) {
        if guid == 0 || self.core.bans.is_banned(guid) {
            return;
        }
        self.core
            .bans
            .append(guid, &crate::game::say::clean_name(name));
        self.kick_slot(slot, now);
    }

    /// 1.5's `tempBanUser`/`tempBanClient` past the lookup (cod_lnxded 1.5b
    /// 0x8087f2a, 0x8088087): the kick, then, for a client with a guid,
    /// `%s (guid %i) was kicked for cheating` and the guid into the table
    /// (docs/research/cod11-server-handshake.md, "Bans").
    fn temp_ban_slot(&mut self, slot: usize, now: Instant) {
        let banned = self.core.clients[slot].as_ref().map(|c| {
            (
                guid_of(crate::engine::Lan::Cod1, c.link.addr),
                crate::game::say::clean_name(&c.name),
            )
        });
        self.kick_slot(slot, now);
        if let Some((guid, name)) = banned
            && guid != 0
        {
            self.print(&format!("{name} (guid {guid}) was kicked for cheating\n"));
            // 0x80890f5 stamps the game clock (0x83ccd84).
            self.core.temp_bans.add(guid, self.core.sv_time_ms);
        }
    }

    /// `sv_kickBanTime`, read at each check (0x8088f75), so a change
    /// reaches bans already made.
    fn kick_ban_time(&self) -> f32 {
        crate::game::builtins::cvar::atof(&self.live_cvar("sv_kickBanTime", "300"))
    }

    /// Replaces `ban.txt`; the binary opens it from `--ban-file`.
    pub fn set_bans(&mut self, bans: BanFile) {
        self.core.bans = bans;
    }

    /// The console's commands for this version, then every cvar, for the
    /// tty console's tab ([`crate::tty_console`]).
    pub(crate) fn console_names(&self) -> Vec<String> {
        let extra: &[&str] = if self.v15() {
            console::COMMANDS_15
        } else {
            &[]
        };
        let mut names: Vec<String> = console::COMMANDS
            .iter()
            .chain(extra)
            .map(|c| c.to_string())
            .collect();
        let cvars = match self.game.module.script.as_ref() {
            Some(rt) => Some(rt.cvars()),
            None => self.game.carried_cvars.as_ref(),
        };
        names.extend(
            cvars
                .into_iter()
                .flat_map(|c| c.names())
                .map(str::to_string),
        );
        names
    }

    /// `SV_KillServer_f` (0x8084d3c) into `SV_Shutdown("EXE_SERVERKILLED")`:
    /// the final message to every client, the flatline to the masters,
    /// every slot freed and the level gone. The process stays; it reads
    /// no packet and runs no frame until a console `map` loads a level
    /// (docs/research/cod11-server-handshake.md, "Shutdown").
    fn kill_server(&mut self) {
        self.final_message("EXE_SERVERKILLED");
        self.shutdown();
    }

    /// `Com_Error(ERR_DROP)` (cod_lnxded 0x806b93c) on a dedicated server:
    /// the `ERROR:` banner, then `SV_Shutdown` with `com_errorEntered` set,
    /// which skips `SV_FinalMessage`, so a client hears nothing and times
    /// out. The process stays, idle, like after `killserver`
    /// (docs/research/cod11-movers.md 18).
    fn drop_level(&mut self, error: &str) {
        log::error!("ERROR: {error}");
        self.shutdown();
    }

    /// `SV_Shutdown` past its final message.
    fn shutdown(&mut self) {
        if let Some(rt) = self.game.module.script.as_ref() {
            self.game.shutdown_log = rt.script_log().to_vec();
        }
        self.shutdown_game_log();
        let mut resolve = self.core.resolver;
        let beats =
            self.core
                .masters
                .shutdown(self.game.dedicated, self.svs_time_ms(), &mut resolve);
        self.core.outbox.extend(beats);
        // The cvar table is process-global and outlives the shutdown.
        let _ = self.lift_persistence();
        self.game.module.script = None;
        self.core.clients.iter_mut().for_each(|c| *c = None);
        self.game.zombies.iter_mut().for_each(|z| *z = None);
        self.core.challenges.clear();
        self.game.module.driver.bots.clear();
        self.game.module.pending_explosions.clear();
        self.game.module.weapon_changes.clear();
        self.game.killed = true;
    }

    /// The level's `G_LogPrintf` lines since the last call into the file.
    pub(crate) fn drain_game_log(&mut self) {
        if let Some(rt) = self.game.module.script.as_mut() {
            let lines = std::mem::take(&mut rt.host.game_log);
            self.game.game_log.write(lines);
        }
    }

    /// `G_ShutdownGame`: the level's last lines, `ShutdownGame:` and the
    /// separator at its last `level.time`, the file closed.
    fn shutdown_game_log(&mut self) {
        self.close_game_log(crate::engine::gamelog::SHUTDOWN);
    }

    /// [`Self::shutdown_game_log`] with `word`: 1.1's `G_ShutdownGame(1)`
    /// writes `RestartGame:` (VERIFIED live, cod11-map-cycle.md 9).
    fn close_game_log(&mut self, word: &str) {
        if let Some(rt) = self.game.module.script.as_mut() {
            let lines = std::mem::take(&mut rt.host.game_log);
            let at = rt.host.level_time_ms;
            self.game.game_log.shutdown(lines, at, word);
        }
    }

    /// `sv_cheats`, which only `SV_Map_f` writes: into systeminfo on the
    /// next frame's flush, and into the cvar table a query reads.
    fn set_cheats(&mut self, on: bool) {
        self.game.cheats = on;
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.cvars.set("sv_cheats", if on { "1" } else { "0" });
        }
        self.refresh_fall_heights();
    }

    /// The drop both kicks end in; a zombie's clock restarts, so a kicked
    /// zombie only lingers longer.
    fn kick_slot(&mut self, slot: usize, now: Instant) {
        self.drop_client(slot, "EXE_PLAYERKICKED");
        let svs_ms = self.svs_time_ms();
        if let Some(z) = self.game.zombies[slot].as_mut() {
            z.last_packet = now;
            z.link.last_packet_ms = svs_ms;
        }
    }

    /// `SV_GetPlayerByName` (0x8083aa0): the first slot in use whose name
    /// matches `name` case-insensitively, as is or with its colour codes
    /// stripped. A zombie's name is gone with its userinfo, so it matches
    /// only an empty one.
    fn player_by_name(&mut self, name: &str) -> Option<usize> {
        let found = (0..self.core.clients.len()).find(|&slot| {
            let n = match (&self.core.clients[slot], &self.game.zombies[slot]) {
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
            for slot in 0..self.core.clients.len() {
                if self.core.clients[slot].is_some() || self.game.zombies[slot].is_some() {
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
        let info = self.core.clients[slot]
            .as_ref()
            .map_or_else(String::new, |c| c.link.userinfo.clone());
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
        for slot in 0..self.core.clients.len() {
            if self.core.clients[slot]
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
                self.game.cfg.gametype.clone(),
            )),
            "sv_maxclients" => Some((
                "sv_maxclients",
                self.level_cvar("sv_maxclients")
                    .unwrap_or_else(|| self.game.cfg.max_clients.to_string()),
                self.pending_cvar("sv_maxclients", &self.game.cfg.max_clients.to_string()),
            )),
            _ => None,
        }
    }

    /// A cvar in the running level's table; `None` with no level or no
    /// value. The level stamps `g_gametype` and `sv_maxclients` at load and
    /// a console write never reaches them there, so this is the live half
    /// of the latch.
    fn level_cvar(&self, name: &str) -> Option<String> {
        let v = self.game.module.script.as_ref()?.cvars().get(name);
        (!v.is_empty()).then(|| v.to_string())
    }

    /// `g_gametype` as the running level loaded it; serverinfo, `getinfo`
    /// and `getstatus` carry this one until a load takes a latched value.
    fn live_gametype(&self) -> String {
        self.level_cvar("g_gametype")
            .unwrap_or_else(|| self.game.cfg.gametype.clone())
    }

    /// `cfg` with the running level's `g_gametype`.
    fn live_cfg(&self) -> ServerConfig {
        let mut cfg = self.game.cfg.clone();
        cfg.gametype = self.live_gametype();
        cfg
    }

    /// `Cvar_Set_f`: a read-only, init or (without `sv_cheats`) cheat cvar
    /// refuses, a latched one keeps its value until the next load and says
    /// so, anything else is written to the running level's table at once.
    fn console_set(&mut self, name: &str, value: &str) {
        if let Some((canon, live, pending)) = self.latched_cvar(name) {
            let mut waiting = (pending != live).then_some(pending);
            if let Some(msg) = crate::cvars::latch(&mut waiting, &live, name, value) {
                self.print(&msg);
            }
            self.set_cvar(canon, waiting.as_deref().unwrap_or(&live));
            return;
        }
        let cheats = self.game.cheats;
        let outcome = match self.game.module.script.as_mut() {
            Some(rt) => rt.host.cvars.console_set(name, value, cheats),
            None => {
                self.set_cvar(name, value);
                return;
            }
        };
        match outcome {
            crate::cvars::ConsoleSet::Refused(msg) => self.print(&msg),
            crate::cvars::ConsoleSet::Latched(msg) => {
                if let Some(msg) = msg {
                    self.print(&msg);
                }
            }
            crate::cvars::ConsoleSet::Applied => self.apply_engine_cvar(name, value),
        }
    }

    /// `Cvar_Command`: a line whose first word names a cvar prints it, or
    /// sets it to the second word. False when no such cvar exists, which
    /// leaves the line an unknown command.
    fn cvar_command(&mut self, argv: &[String]) -> bool {
        let Some(name) = argv.first() else {
            return false;
        };
        let Some((canon, value, default, waiting)) =
            self.game.module.script.as_ref().and_then(|rt| {
                let cv = rt.cvars();
                let (n, v, d) = cv.lookup(name)?;
                Some((
                    n.to_string(),
                    v.to_string(),
                    d.to_string(),
                    cv.latched(name).map(str::to_string),
                ))
            })
        else {
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
        self.game
            .module
            .script
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
            self.game.cfg.map
        );
        for slot in 0..self.core.clients.len() {
            let zombie = self.core.clients[slot].is_none();
            let score = if zombie { 0 } else { self.client_score(slot) };
            let Some(c) = self.core.clients[slot]
                .as_ref()
                .or(self.game.zombies[slot].as_ref())
            else {
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
                    console::Ping::Ms(c.link.ping)
                },
                // The drop nukes the userinfo, and the name with it.
                name: if zombie { "" } else { &c.name },
                last_msg_ms: now.saturating_duration_since(c.last_packet).as_millis() as i64,
                addr: c
                    .bot_port
                    .map_or(console::Adr::Ip(c.link.addr), console::Adr::Bot),
                qport: c.link.netchan.qport,
                rate: c.rate(self.game.dedicated),
            };
            out.push_str(&row.to_string());
        }
        out.push('\n');
        out
    }

    /// `Com_Printf`: the log, and the rcon reply when one is being collected.
    fn print(&mut self, msg: &str) {
        if let Some(r) = self.game.redirect.as_mut() {
            r.print(msg);
        }
        log::info!("{}", msg.trim_end());
    }

    const FALLBACK_SPAWN: ([f32; 3], f32) = ([0.0, 0.0, 64.0], 0.0);

    /// `SV_ClientEnterWorld` for a spectator: park the sim, start
    /// snapping. `entering` is the usercmd that brought the client in, which
    /// is `None` on the paths that have none, as ioq3's is null there; its
    /// angles are what the spawn's `delta_angles` subtract.
    fn enter_world(&mut self, slot: usize, entering: Option<&UserCmd>) {
        let svs_ms = self.svs_time_ms();
        // Under a game module the client stays where `ClientConnect`'s
        // zeroing left it, origin and `delta_angles` 0, until a script
        // spawns it (cod11-gametypes-re-bel.md 8.3). Without one nothing
        // ever would, so it stands at the map's spawn.
        let (spawn, cmd_angles) = if self.game.module.script.is_some() {
            (([0.0; 3], 0.0), [0; 3])
        } else {
            (
                self.game
                    .world
                    .as_ref()
                    .map_or(Self::FALLBACK_SPAWN, |w| w.spawn),
                entering.map_or(NULL_USERCMD.angles, |c| c.angles),
            )
        };
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        c.state = ClientState::Active;
        // The first snapshot goes this frame, not after the primed client's
        // 1 s keepalive interval (0x80877d8).
        c.link.next_message_ms = svs_ms;
        let mut sim = ClientSim::spectator(spawn.0, spawn.1, cmd_angles);
        // A 1.5 server moves every client, bots too, under 1.5's rules, as
        // its game module's one pmove does.
        sim.ps.rules = vcod_common::pmove::MoveRules::for_protocol(self.game.proto);
        c.sim = Some(sim);
        // The entering cmd is not simulated: retail's execute loop skips
        // every cmd at or before `lastUsercmd`, which entry has just set to
        // it. The clock is the client's own, held to the window
        // `replay_moves` holds every cmd to, so a handshake cmd stamped far
        // ahead cannot leave `commandTime` past every cmd that follows. With
        // no entering cmd, one frame back, so the first cmd's dt is a sane
        // 50 ms rather than the whole age of the client's clock.
        c.last_processed_st = entering.map_or(self.core.sv_time_ms.wrapping_sub(FRAME_MS), |c| {
            c.server_time.clamp(
                self.core.sv_time_ms.wrapping_sub(1000),
                self.core.sv_time_ms.wrapping_add(200),
            )
        });
        c.last_usercmd_st = entering.map_or(c.last_processed_st, |c| c.server_time);
        log::info!("client {slot} {:?} begin (spectator)", c.name);
        // `ClientBegin`: the notify that releases the connect callback's
        // `waittill("begin")`. The event queues rather than fires here, so it
        // is drained after the `Connect` that armed the wait.
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.push_client_event(ClientEvent::Begin(slot));
        }
    }

    /// One `SV_Frame` that runs a single game frame.
    pub fn tick(&mut self, now: Instant) {
        self.tick_frames(now, 1);
    }

    /// `SV_Frame`'s head: `Cbuf_Execute`, then whether the frame runs at
    /// all. Keeps the wall gap for `--trace` and starts the tick's moves.
    fn frame_head(&mut self, now: Instant, frames: u32) -> bool {
        crate::tick_trace::step!("Cbuf_Execute");
        self.drain_console(now);
        // `Com_Quit_f` exits inside the command; nothing after it runs. A
        // killed server runs no frame (`SV_Frame` returns while
        // `sv_running` is 0).
        if self.core.quit || self.game.killed {
            return false;
        }
        // Wall gap between ticks: sv_time advances FRAME_MS a frame, so a
        // gap far off it means the frames the client interpolates between
        // are not arriving at the rate their timestamps claim.
        let wall_ms = self
            .game
            .last_tick
            .map(|t| now.saturating_duration_since(t).as_secs_f32() * 1000.0);
        self.game.last_tick = Some(now);
        self.game.tick_wall = (wall_ms, frames);
        self.game.tick_moved = None;
        true
    }

    /// The bots' cmds, once a `SV_Frame`.
    fn bot_frame(&mut self) {
        // The bots' walks move by this server's rules, as their bodies do.
        crate::tick_trace::step!("SV_BotFrame");
        crate::nav::with_rules(self.move_rules(), || self.step_bots());
    }

    /// `SV_ArchiveSnapshot` between the frames (0x808d1bb), while script
    /// has the archive on.
    fn archive_between(&mut self) {
        if self.game.archive.is_on() {
            crate::tick_trace::step!("SV_ArchiveSnapshot");
            let world = self.build_world_frame();
            self.archive_frame(world);
        }
    }

    /// How far this frame moves the game clock. 1.1's `SV_Frame` steps
    /// it a whole frame whatever `timescale` says; 1.5b's (0x80942a1)
    /// steps `svs.time` a frame and the game clock by `Com_ModifyMsec`
    /// (0x80716b2) of it, which truncates the scaled frame, bumps a
    /// non-zero result under 1 to 1 and lets 0 stand. A negative
    /// `timescale` is set to 0 first (cod15-protocol-diff.md, section 12).
    fn game_step_ms(&mut self) -> i32 {
        if !self.v15() {
            return FRAME_MS;
        }
        let Some(rt) = self.game.module.script.as_mut() else {
            return FRAME_MS;
        };
        let mut scale = crate::game::builtins::cvar::atof(rt.cvars().get("timescale"));
        if scale < 0.0 {
            rt.host.cvars.set("timescale", "0");
            scale = 0.0;
        }
        let step = (FRAME_MS as f32 * scale) as i32;
        if step <= 0 && scale != 0.0 {
            1
        } else {
            step.clamp(0, 5000)
        }
    }

    /// The frame's clock: 1.5's game clock steps by `timescale`, `svs.time`
    /// a whole frame. Returns the game clock's step, which the engine adds.
    fn frame_clock(&mut self) -> i32 {
        let step = self.game_step_ms();
        self.game.paused_ms = self.game.paused_ms.wrapping_add(FRAME_MS - step);
        crate::tick_trace::step!(
            "G_RunFrame time {}",
            self.core.sv_time_ms.wrapping_add(step)
        );
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.frozen = step == 0;
        }
        step
    }

    /// One game frame past its clock and its moves (`moved`, by slot, which
    /// left the world at this frame before script or anyone's snapshot reads
    /// it): the script frame and the entity pass, and what they queued for
    /// the wire. False when a `G_Error` dropped the level mid-frame.
    fn game_frame(&mut self, moved: &[MoveSummary]) -> bool {
        let svs_ms = self.svs_time_ms();
        // `G_RunFrame`, the script threads through every `ClientEndFrame`.
        let (game, eng) = self.split(svs_ms);
        let FrameOut {
            vote_lines,
            timeout_written,
            console_lines,
            client_commands,
            ranks_dirty,
        } = match game.run_frame_cod1(eng, moved) {
            Ok(out) => out,
            // A `G_Error` in the pass: `Com_Error`'s drop runs instead of
            // the rest of the frame (docs/research/cod11-movers.md 18).
            Err(e) => {
                self.drop_level(&e);
                return false;
            }
        };
        crate::tick_trace::step!("SV_GameFrameOut");
        self.send_vote_lines(vote_lines.into_iter().map(|l| (None, l)).collect());
        if timeout_written {
            self.copy_timeout_configstrings();
        }
        if let Some(rt) = self.game.module.script.as_mut() {
            // The script owns the table while it runs and allocates into it
            // from any thread, so the server re-reads it rather than trusting
            // the copy `load_scripts` took. A whole-table copy per frame is
            // cheap next to a snapshot, and there is no single write choke
            // point on the host's table to hang a dirty flag off. The cvar
            // mirror gets the same treatment: a thread past a `wait` can
            // still call `setCvar`.
            self.core.configstrings = rt.configstrings().to_vec();
            if let Err(e) = rt.cvars().write_mirror(&mut self.core.configstrings) {
                log::warn!("rebuilding the cvar mirror: {e:?}");
            }
        }
        self.refresh_fall_heights();
        self.flush_serverinfo();
        // `trap_SendConsoleCommand`'s `EXEC_APPEND`: what a builtin queued
        // this frame runs at the top of the next tick, never mid-frame.
        self.core.console.extend(console_lines);
        // Ahead of the frame's client commands, so a slot the script just
        // allocated is named before anything points at it.
        crate::tick_trace::step!("SV_SetConfigstring broadcast");
        self.broadcast_configstring_changes();
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.configstrings.take_writes();
        }
        // Outside the borrow: `setClientCvar` and `openMenu` queue rather
        // than send, and this is where the queue reaches the netchan.
        for (slot, cmd) in client_commands {
            crate::tick_trace::step!("SV_GameSendServerCommand slot {slot}");
            self.send_server_command(slot, &cmd);
        }
        // `G_RunFrame`'s inlined drain (map-cycle doc, 6.3): a frame that
        // moved a score pushes the scoreboard to every client in
        // intermission, and no frame pushes one otherwise. The map-change
        // capture carries no `b` its probe did not ask for, so a timer here
        // would be a command retail never sends.
        if ranks_dirty {
            let watching: Vec<usize> = self
                .core
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
                crate::tick_trace::step!("scoreboard slot {slot}");
                let text = self.scoreboard();
                self.send_server_command(slot, &text);
            }
        }

        true
    }

    /// The game and what the engine lends it for one call, borrowed apart.
    fn split(&mut self, svs_time_ms: i32) -> (&mut Cod1Module, Lend<'_>) {
        (
            &mut self.game.module,
            Lend {
                clients: &mut self.core.clients,
                world: &self.game.world,
                fs: &self.game.fs,
                proto: self.game.proto,
                sv_time_ms: self.core.sv_time_ms,
                svs_time_ms,
                rng: &mut self.core.rng,
                stuck_rand: &mut self.game.stuck_rand,
                configstrings: &self.core.configstrings,
                archive: &mut self.game.archive,
                test_client_counter: &mut self.game.test_client_counter,
            },
        )
    }

    /// `SV_Frame`'s last step (0x808d258).
    fn master_heartbeat(&mut self) {
        let mut resolve = self.core.resolver;
        let beats = self.core.masters.heartbeat(
            self.game.dedicated,
            self.svs_time_ms(),
            crate::master::HEARTBEAT_GAME,
            &mut resolve,
        );
        self.core.outbox.extend(beats);
        // The weapon changes are already drained when a script is loaded,
        // and a server without one has nothing to write them to.
        self.game.module.weapon_changes.clear();
    }

    /// `SV_ExecuteClientMessage` for every packet since the last tick, in
    /// arrival order, each cmd a `ClientThink_real`
    /// ([`Cod1Module::client_think_cod1`]). Returns what each slot
    /// replayed.
    fn replay_moves(&mut self) -> Vec<MoveSummary> {
        let svs = self.svs_time_ms();
        let (game, mut eng) = self.split(svs);
        game.client_think_cod1(&mut eng)
    }

    /// What every snapshot of this frame takes its entities and roster
    /// from, and what `SV_ArchiveSnapshot` keeps of the frame.
    fn build_world_frame(&mut self) -> crate::archive::WorldFrame {
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
        let per_slot: Vec<Roster> = match self.game.module.script.as_mut() {
            Some(rt) => (0..self.core.clients.len())
                .map(|slot| {
                    (
                        rt.client_model_index(slot),
                        rt.client_attachments(slot),
                        rt.client_team(slot),
                    )
                })
                .collect(),
            None => vec![(0, Vec::new(), script::TEAM_SPECTATOR); self.core.clients.len()],
        };
        let roster: BTreeMap<u32, msg::ClientState> = self
            .core
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let c = c.as_ref()?;
                let (model, attachments, team) = &per_slot[i];
                let mut cs = msg::ClientState::named(self.game.proto, 0, *team, &c.name);
                if let Some(idx) = msg::ClientState::field_index(self.game.proto, "modelindex") {
                    cs.fields[idx] = *model;
                }
                for (n, (am, at)) in attachments.iter().enumerate() {
                    for (name, v) in [
                        (format!("attachModelIndex[{n}]"), am),
                        (format!("attachTagIndex[{n}]"), at),
                    ] {
                        if let Some(idx) = msg::ClientState::field_index(self.game.proto, &name) {
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
            .game
            .module
            .script
            .as_mut()
            .map_or_else(BTreeMap::new, |rt| rt.packet_entities(self.game.proto));
        if let Some(te) = self.game.module.test_entities.as_ref() {
            entities.extend(te.at(self.game.proto, self.core.sv_time_ms));
        }

        // One entity per client with a sim, so a client can be told what the
        // others are doing. The receiver's own is dropped below: retail sends
        // a client no entity for itself.
        let client_entities: BTreeMap<u32, msg::EntityState> = self
            .core
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let sim = c.as_ref()?.sim.as_ref()?;
                // A spectator, an intermission camera and a dead player are
                // each unlinked or `SVF_NOCLIENT`, so nobody is sent an
                // entity for one ([`crate::spectate::ClientSim::linked`]).
                // A tank's driver is never sent to anyone (the engine's
                // snapshot build, coduo_lnxded 0x8096b97).
                let hidden = sim
                    .seat
                    .is_some_and(|s| crate::game::vehicle::seat::hidden_rider(s.kind, s.pos));
                // The state this frame's own end frame copied, which a
                // higher slot's turret round after it changes only in the
                // playerstate: its knockback, and a death, which still goes
                // out that frame, `player_die`'s relink as a corpse clearing
                // `solid` (combat doc 5.1 step 13, 5.4; turrets doc 13.1).
                let copied = sim
                    .end_frame_entity
                    .as_ref()
                    .filter(|(t, _)| *t == self.core.sv_time_ms)
                    .map(|(_, e)| e.clone());
                if hidden {
                    None
                } else if sim.linked() {
                    let mut e = copied.unwrap_or_else(|| {
                        sim.to_entity(self.game.proto, i, c.as_ref().unwrap().last_processed_st)
                    });
                    // Killed after its end frame copied it, its session
                    // still `playing` there: the corpse relink still lands.
                    if sim.dead
                        && let Some(f) = msg::EntityState::field_index(self.game.proto, "solid")
                    {
                        e.fields[f] = sim.linked_solid;
                    }
                    Some((i as u32, e))
                } else if sim.dead {
                    let mut e = copied?;
                    if let Some(f) = msg::EntityState::field_index(self.game.proto, "solid") {
                        e.fields[f] = 0;
                    }
                    Some((i as u32, e))
                } else {
                    None
                }
            })
            .collect();

        // A body born since the last build is re-read from its source
        // client's sim once, here: the script clones the player before it
        // raises the death animation, so the state the clone copied is a
        // frame stale (`crate::game::bodies`). Then the queue's corpses join the map's
        // entities; they are culled per client like anything else.
        let proto = self.game.proto;
        let collision = self.game.world.as_ref().map(|w| &w.collision);
        let clients = &self.core.clients;
        if let Some(rt) = self.game.module.script.as_mut() {
            // Straight off the sim, not `client_entities`: the source is
            // dead by now and has no entity there.
            rt.bodies_mut().refresh_newborn(
                |slot| {
                    let c = clients.get(slot)?.as_ref()?;
                    Some(c.sim.as_ref()?.to_entity(proto, slot, c.last_processed_st))
                },
                collision,
                proto,
            );
            entities.extend(rt.body_entities(proto));
        }

        // Every live temp entity, at the number `G_TempEntity` took for it,
        // on every snapshot until the entity pass frees it.
        let (temps, temp_states): (Vec<temp_entity::Scope>, Vec<(u32, msg::EntityState)>) = self
            .game
            .module
            .script
            .as_ref()
            .map(|rt| {
                rt.temp_entities()
                    .map(|(n, te)| (te.scope, (n, temp_entity::build(te, n, self.game.proto))))
                    .unzip()
            })
            .unwrap_or_default();

        // What every snapshot this frame takes its entities from, and what
        // the archive keeps of the frame. A missile carries `SVF_BROADCAST`
        // (combat doc, 11.1), so it skips the cull the way a broadcast temp
        // entity does.
        // So does anything `playSound` touched, a player included.
        let mut culled: BTreeMap<u32, Rc<msg::EntityState>> =
            entities.into_iter().map(|(n, e)| (n, Rc::new(e))).collect();
        culled.extend(client_entities.into_iter().map(|(n, e)| (n, Rc::new(e))));
        let mut broadcast: BTreeMap<u32, Rc<msg::EntityState>> = self
            .game
            .module
            .script
            .as_ref()
            .map_or_else(BTreeMap::new, |rt| {
                rt.missiles()
                    .entities(self.game.proto)
                    .map(|(n, e)| (n, Rc::new(e)))
                    .collect()
            });
        if let Some(rt) = self.game.module.script.as_ref() {
            for n in rt.broadcast_entities(self.svs_time_ms()) {
                if let Some(e) = culled.remove(&n) {
                    broadcast.insert(n, e);
                }
            }
        }
        crate::archive::WorldFrame {
            culled,
            temps: temps
                .into_iter()
                .zip(temp_states)
                .map(|(scope, (_, e))| (scope, Rc::new(e)))
                .collect(),
            broadcast,
            roster: Rc::new(roster),
            time: self.core.sv_time_ms,
            fog: self.fog_cull(),
        }
    }

    /// `SV_SendClientMessages`' head ([`crate::engine::send_messages`]):
    /// every entity state built once from the world the moves and the
    /// script frame left, to be culled and written per client, and each
    /// client's follow frame and archive source. `moved` and `wall` are
    /// `--trace`'s: what each slot replayed, the wall gap since the last
    /// tick and the frames this tick ran, so a catch-up tick's gap reads
    /// against them.
    fn snapshots_begin(
        &mut self,
        moved: Vec<MoveSummary>,
        wall: (Option<f32>, u32),
    ) -> Cod1Snapshots {
        let live = self.build_world_frame();
        let svs_ms = self.svs_time_ms();
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.expire_broadcast_times(svs_ms);
        }
        let teams: Vec<i32> = (0..self.core.clients.len())
            .map(|slot| {
                self.game
                    .module
                    .script
                    .as_mut()
                    .map_or(script::TEAM_SPECTATOR, |rt| rt.client_team(slot))
            })
            .collect();

        // A follower's frame is the followed client's: its playerstate with
        // the follow flags patched in, its eye for the cull and its number
        // for the single-client scopes (`SpectatorClientEndFrame` 0x40896,
        // `SV_BuildClientSnapshot` 0x808f25f). A copy the end frame took out
        // of the archive is that frame's, its times shifted by the age
        // (0x808ef7c). Built up front, since the loop below holds each slot
        // mutably.
        let now = self.core.sv_time_ms;
        let follow_frames: Vec<Option<FollowFrame>> = (0..self.core.clients.len())
            .map(|slot| {
                let sim = self.core.clients[slot].as_ref()?.sim.as_ref()?;
                let t = sim.follow.target.filter(|_| sim.follow.on)?;
                let own = sim.to_wire(self.game.proto, slot as i32, 0);
                let ef = msg::PlayerState::field_index(self.game.proto, "eFlags").unwrap();
                let (mut ps, eye, replay) = match sim.follow.copied.and_then(|c| c.frame) {
                    Some(index) => {
                        let f = self.game.archive.frame(index)?;
                        let view = f.clients.get(t)?.as_ref()?;
                        let ps = crate::archive::replayed_ps(view, now - f.time, self.game.proto);
                        (ps, view.eye, true)
                    }
                    None => {
                        let tc = self.core.clients.get(t)?.as_ref()?;
                        let ts = tc.sim.as_ref()?;
                        let mut ps = ts.to_wire(self.game.proto, t as i32, tc.last_processed_st);
                        // The end-frame loop runs in slot order.
                        if let (true, Some(last)) = (slot < t, ts.end_frame_wire.as_ref()) {
                            follow::before_end_frame(&mut ps, last, self.game.proto);
                        }
                        // `P_DamageFeedback`'s `EV_PAIN` is that end frame's too.
                        if let (true, Some(ring)) = (slot < t, ts.ring_before_pain) {
                            ring.write(&mut |name, v| {
                                let i =
                                    msg::PlayerState::field_index(self.game.proto, name).unwrap();
                                ps.fields[i] = v;
                            });
                        }
                        (ps, ts.eye_origin(), false)
                    }
                };
                follow::patch_wire(&mut ps, self.game.proto, sim.follow.forced, own.fields[ef]);
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
            let Some(c) = self.core.clients[slot].as_mut() else {
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
                .then(|| sim.to_wire(self.game.proto, slot as i32, command_time));
            sim.ring_before_pain = None;
        }

        // `SV_BuildClientSnapshot` reads each client's `archivetime` again and
        // takes the entities and the roster from the frame it names, times
        // shifted by the age, whether or not the client follows anyone
        // (0x808f1ab..0x808f211); the trim is written back as it is there.
        let sources: Vec<Option<(i32, i32)>> = (0..self.core.clients.len())
            .map(|slot| {
                let rt = self.game.module.script.as_mut()?;
                let asked = rt.client_archive_ms(slot);
                let mut age = asked;
                let index = self.game.archive.lookup(&mut age);
                if age != asked {
                    rt.set_client_archive_ms(slot, age);
                }
                let f = self.game.archive.frame(index?)?;
                Some((index?, now - f.time))
            })
            .collect();

        Cod1Snapshots {
            live,
            teams,
            follow_frames,
            sources,
            open: Box::new(self.download_opener()),
            www: self.www_config(),
            max_rate: self.live_int_cvar("sv_maxRate"),
            moved,
            wall,
        }
    }

    /// Whether `slot` takes a turn in `SV_SendClientMessages` (0x809045c):
    /// a client with a socket, in the world or not, or a zombie, once its
    /// `nextSnapshotTime` came. A bot has no socket to write to; its slot
    /// still counts toward the roster and entity lists the real clients
    /// are sent.
    fn message_due(&self, slot: usize) -> bool {
        let svs_ms = self.svs_time_ms();
        match (&self.core.clients[slot], &self.game.zombies[slot]) {
            (Some(c), _) => !c.is_bot && svs_ms >= c.link.next_message_ms,
            (None, Some(z)) => svs_ms >= z.link.next_message_ms,
            (None, None) => false,
        }
    }

    /// A turn with fragments left sends the next one instead of a
    /// snapshot ([`Client::next_fragment`]). A zombie's message goes out
    /// whole on its turn.
    fn next_fragment(&mut self, slot: usize) -> Option<(SocketAddr, Vec<u8>)> {
        let svs_ms = self.svs_time_ms();
        let max_rate = self.live_int_cvar("sv_maxRate");
        let c = self.core.clients[slot].as_mut()?;
        let rate = c.send_rate(self.game.dedicated, max_rate);
        c.next_fragment(svs_ms, rate).map(|pkt| (c.link.addr, pkt))
    }

    /// `SV_SendClientSnapshot` (0x808f844), which serves every state: a
    /// zombie's message ([`Self::send_zombie`]), a client not in the
    /// world's ([`Self::send_unentered`]), or for a client in the world a
    /// delta against the frame it last acked when one is still in its
    /// ring, uncompressed otherwise, culled to what it can see, then its
    /// download's blocks.
    fn send_client_snapshot(&mut self, slot: usize, snaps: &Cod1Snapshots) {
        match &self.core.clients[slot] {
            None => return self.send_zombie(slot, snaps.max_rate),
            Some(c) if c.sim.is_none() => return self.send_unentered(slot, snaps),
            Some(_) => {}
        }
        let svs_ms = self.svs_time_ms();
        let collision_vis = self.game.world.as_ref().map(|w| &w.vis);
        let Cod1Snapshots {
            live,
            teams,
            follow_frames,
            sources,
            open,
            www,
            max_rate,
            moved,
            wall,
        } = snaps;
        let follow_frame = follow_frames[slot].as_ref();
        {
            let Some(c) = self.core.clients[slot].as_mut() else {
                return;
            };
            let rate = c.send_rate(self.game.dedicated, *max_rate);
            let Some(sim) = c.sim.as_ref() else {
                return;
            };
            // Exactly the serverTime of the last cmd the sim consumed, and
            // nothing else: the client replays everything past it, so a
            // commandTime we never simulated drops that slice of its input
            // and its prediction judders (docs/protocol-1.1.md).
            c.link.was_active = true;
            let command_time = c.last_processed_st;
            let message_num = c.link.netchan.outgoing_sequence;
            // The frame's `ps.clientNum`, which the single-client flags test
            // against and whose entity the frame leaves out.
            let client_num = follow_frame.map_or(slot, |f| f.target);

            // Retail sends a client only what its own position can see, so
            // the list is per client rather than one list cloned into every
            // frame (docs/protocol-1.1.md, "Which entities a client is sent").
            let eye = follow_frame.map_or_else(|| sim.eye_origin(), |f| f.eye);
            let (world, shift) = match sources[slot] {
                Some((index, shift)) => (&self.game.archive.frame(index).unwrap().world, shift),
                None => (live, 0),
            };
            let mut visible =
                world.entities_for(client_num, eye, collision_vis, shift, self.game.proto);
            if self.game.proto.et_events != vcod_common::net::events::ET_EVENTS {
                for es in visible.values_mut() {
                    crate::game::wire::entity_to_protocol(self.game.proto, es);
                }
            }
            let roster = (*world.roster).clone();

            let mut ps = match follow_frame {
                Some(f) => f.ps.clone(),
                None => sim.to_wire(self.game.proto, client_num as i32, command_time),
            };
            let pm = msg::PlayerState::field_index(self.game.proto, "pm_flags").unwrap();
            ps.fields[pm] |= sim.spectate_flags;
            // The script's HUD elements, filtered for this client the way
            // `HudElem_UpdateClient` filters them; retail rebuilds both
            // arrays into the playerstate once per client per frame, so
            // they are read here rather than carried on the sim. A follower
            // keeps the followed client's archived half and objectives, which
            // ride the copied playerstate, and gets its own unarchived half.
            let team_of = |s: usize| teams.get(s).copied().unwrap_or(script::TEAM_SPECTATOR);
            let team = team_of(slot);
            if let Some(rt) = self.game.module.script.as_mut() {
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
            crate::game::wire::ps_to_protocol(self.game.proto, &mut ps);
            // `ClientSpawn`'s memset ran after the frame's HUD update
            // (the retail killcam's end frame reads both arrays empty).
            if sim.hud_cleared {
                ps.arrays.hud_archived.clear();
                ps.arrays.hud_current.clear();
            }
            let frame = snapshot::Snapshot {
                server_time: self.core.sv_time_ms,
                message_num,
                delta_num: -1,
                snap_flags: self.game.snap_flag_server_bit | u32::from(c.link.rate_delayed),
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

            let mut w = MsgWriter::new(&self.core.huff);
            write_pending_commands(&mut w, &c.link.netchan, c.link.reliable_ack);
            c.link.reliable_sent = c.link.netchan.reliable_sequence as i32;
            self.game.proto.write_snapshot(
                &mut w,
                base.as_ref(),
                &frame,
                frame.head(base.as_ref()),
                &self.game.baselines,
            );
            c.record_frame(frame);
            write_download(c, &mut w, svs_ms, rate, &open, www.as_ref());

            let ops = w.into_ops();

            if self.game.cfg.trace {
                // The prediction contract: a client replays every usercmd
                // newer than ps.commandTime, so `lead` at or below zero means
                // we claim to have simulated past our own frame clock and the
                // client has nothing left to replay. Retail's captures run a
                // lead of 0..34 ms, never negative (examples/snapshot_timing).
                let lead = self.core.sv_time_ms - command_time;
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
                    "trace c{slot} msg {message_num} wall {} frames {} sv {} ct {} lead {} \
cmds {processed} span {span} queued {queued} ack {} behind {ack_behind} {base_desc} {} B{walk}",
                    wall.0.map_or("-".into(), |w| format!("{w:.1}")),
                    wall.1,
                    self.core.sv_time_ms,
                    command_time,
                    lead,
                    c.message_ack,
                    ops.len(),
                );
            }
            c.stamp_sent(message_num, svs_ms);
            let sent = c
                .link
                .netchan
                .transmit(c.link.last_client_command, &ops, &self.core.huff);
            let msec = c.snapshot_msec();
            c.pace(svs_ms, sent.iter().map(Vec::len).sum(), rate, msec);
            if let Some(pkt) = c.first_packet(sent) {
                self.core.outbox.push((c.link.addr, pkt));
            }
        }
    }

    /// After every client's turn: the HUD a spawn cleared is back, and the
    /// frame the snapshots were built from goes into the archive.
    fn snapshots_end(&mut self, snaps: Cod1Snapshots) {
        for c in self.core.clients.iter_mut().flatten() {
            if let Some(sim) = c.sim.as_mut() {
                sim.hud_cleared = false;
            }
        }
        self.archive_frame(snaps.live);
    }

    /// `SV_ArchiveSnapshot` (`cod_lnxded` 0x808fb84), after the frame's
    /// snapshots: the entities and roster they were built from, and every
    /// client's own view as `GetFollowPlayerState` would answer it, which a
    /// spectating or intermission client has none of.
    fn archive_frame(&mut self, world: crate::archive::WorldFrame) {
        if !self.game.archive.is_on() {
            return;
        }
        let clients = (0..self.core.clients.len())
            .map(|slot| {
                let c = self.core.clients[slot].as_ref()?;
                let sim = c.sim.as_ref().filter(|s| s.own_view)?;
                let mut ps = sim.to_wire(self.game.proto, slot as i32, c.last_processed_st);
                if let Some(rt) = self.game.module.script.as_mut() {
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
        self.game.archive.push(crate::archive::Frame {
            time: self.core.sv_time_ms,
            world,
            clients,
        });
    }
}

/// Sectors round a `bas` base the breach stands are spread over.
const BREACH_SECTORS: usize = 16;
/// How far off a breach stand may lie, and the distance it is picked
/// nearest to.
const BREACH_STAND_FAR: f32 = 1000.0;
const BREACH_STAND_IDEAL: f32 = 600.0;
/// Script models and vehicles this near a base are in its breach traces:
/// anywhere between it and a stand. A rocket from mp_italy's stand by the
/// allied spawn struck the Sherman parked 190 units on, 1290 from the
/// base, and killed its shooter.
const BREACH_MODEL_NEAR: f32 = crate::bots::BREACH_RANGE + 64.0;

/// Where a rocket from `muzzle` is aimed at a base whose script model
/// stands at `base`, from [`crate::bots::BREACH_MIN`] to
/// [`crate::bots::BREACH_RANGE`] off: a point on the model's mesh the
/// rocket strikes first (`G_MissileImpact`'s direct `G_Damage`), or one
/// whose burst on the world lands within [`crate::bots::BREACH_NEAR`] of
/// the origin (the splash). Tried up the bunker, then at its foot.
fn breach_aim(
    world: &vcod_common::collision::CollisionWorld,
    models: &[crate::game::combat::PlacedModel],
    muzzle: glam::Vec3,
    base: glam::Vec3,
) -> Option<[f32; 3]> {
    use crate::bots::{BREACH_CLEAR, BREACH_MIN, BREACH_NEAR, BREACH_RANGE};
    let d = muzzle.distance(base);
    if !(BREACH_MIN..=BREACH_RANGE).contains(&d) {
        return None;
    }
    [40.0, 72.0, 16.0, 0.0].into_iter().find_map(|up| {
        let spot = base + glam::Vec3::Z * up;
        let dir = (spot - muzzle).normalize_or_zero();
        let end = spot + dir * BREACH_NEAR;
        let tr = world.missile_trace(muzzle, end);
        let mut best = (tr.fraction, None);
        for m in models {
            if let Some((f, ..)) = m.clip(
                muzzle,
                end,
                crate::game::missile::MISSILE_MODEL_MASK,
                best.0,
            ) {
                best = (f, Some(m.origin));
            }
        }
        let burst = muzzle + (end - muzzle) * best.0;
        // The base's mesh reaches out toward the shooter: a strike on it
        // inside the rocket's own splash took the shooter too.
        let ok = burst.distance(muzzle) >= BREACH_CLEAR
            && match best.1 {
                Some(o) => o == base,
                None => best.0 < 1.0 && burst.distance(base) < BREACH_NEAR,
            }
            && cone_clear(
                world,
                models,
                muzzle,
                dir,
                BREACH_SPREAD_DEG,
                crate::bots::BREACH_CLEAR,
            );
        ok.then_some(spot.into())
    })
}

/// Per group, the index of its first item `pass` takes, as a search of
/// each group in order would find it, on up to [`BREACH_THREADS`] threads:
/// [`Server::breach_stands`]' sectors. The items go out by their place in
/// their group, every group's first, then every group's second, and one
/// behind a pass its group already found is skipped. One sector after
/// another the traces held a `bas` tick 25 to 30 ms (map-cycle doc 4.9).
fn first_passes<T: Sync>(groups: &[&[T]], pass: impl Fn(&T) -> bool + Sync) -> Vec<Option<usize>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut order: Vec<(usize, usize)> = groups
        .iter()
        .enumerate()
        .flat_map(|(g, items)| (0..items.len()).map(move |i| (i, g)))
        .collect();
    order.sort_unstable();
    let found: Vec<AtomicUsize> = groups
        .iter()
        .map(|_| AtomicUsize::new(usize::MAX))
        .collect();
    let next = AtomicUsize::new(0);
    let work = || loop {
        let Some(&(i, g)) = order.get(next.fetch_add(1, Ordering::Relaxed)) else {
            return;
        };
        if found[g].load(Ordering::Relaxed) > i && pass(&groups[g][i]) {
            found[g].fetch_min(i, Ordering::Relaxed);
        }
    };
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(BREACH_THREADS)
        .min(order.len());
    if threads <= 1 {
        work();
    } else {
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(work);
            }
        });
    }
    found
        .into_iter()
        .map(|f| Some(f.into_inner()).filter(|&i| i != usize::MAX))
        .collect()
}

/// The most threads [`first_passes`] runs.
const BREACH_THREADS: usize = 8;

/// The loaded rocket launcher a bot carries ([`crate::bots::BotView::rocket`]):
/// the one in `hand` if loaded, else the first loaded one in `loaded`
/// (the held weapons with a round left).
fn held_rocket(hand: u8, loaded: u64, launcher: impl Fn(u8) -> bool) -> Option<u8> {
    let ok = |w: u8| w < 64 && loaded >> w & 1 == 1 && launcher(w);
    Some(hand)
        .filter(|&w| ok(w))
        .or_else(|| (1u8..64).find(|&w| ok(w)))
}

/// The launchers' sighted spread standing (`adsSpread`: 4 for the
/// bazooka and the panzerschreck, 3.5 for the panzerfaust).
const BREACH_SPREAD_DEG: f32 = 4.0;

/// Whether rockets along `dir` and the edge of a `spread_deg` cone round
/// it meet nothing within `reach` of the muzzle. A breach stand checks the
/// sighted cone out to [`crate::bots::BREACH_CLEAR`]: the line's middle
/// alone left a bot now and then killed by a rocket the spread carried
/// into the cover by its stand. Only the near part of the cone counts;
/// where the rest bursts the base's checks already rule.
pub(crate) fn cone_clear(
    world: &vcod_common::collision::CollisionWorld,
    models: &[crate::game::combat::PlacedModel],
    muzzle: glam::Vec3,
    dir: glam::Vec3,
    spread_deg: f32,
    reach: f32,
) -> bool {
    let right = dir.cross(glam::Vec3::Z).normalize_or(glam::Vec3::X);
    let up = right.cross(dir);
    let r = spread_deg.to_radians().tan();
    (0..9).all(|k| {
        let a = k as f32 * std::f32::consts::FRAC_PI_4;
        let edge = if k == 8 {
            glam::Vec3::ZERO
        } else {
            (right * a.cos() + up * a.sin()) * r
        };
        let ray = (dir + edge).normalize();
        let end = muzzle + ray * reach;
        world.missile_trace(muzzle, end).fraction >= 1.0
            && models.iter().all(|m| {
                m.clip(muzzle, end, crate::game::missile::MISSILE_MODEL_MASK, 1.0)
                    .is_none()
            })
    })
}

#[cfg(test)]
mod tests {
    /// The threaded search finds each group's first pass, as one group
    /// after another searched in order would, and nothing for a group with
    /// none.
    #[test]
    fn first_passes_finds_what_a_serial_search_finds() {
        let groups: Vec<Vec<u32>> = (0..16u32)
            .map(|g| (0..(g * 7 % 40)).map(|i| (g * 131 + i * 17) % 97).collect())
            .collect();
        let pass = |v: &u32| v % 13 == 5;
        let views: Vec<&[u32]> = groups.iter().map(|g| &g[..]).collect();
        let serial: Vec<Option<usize>> = groups.iter().map(|g| g.iter().position(pass)).collect();
        assert!(serial.iter().any(Option::is_none) && serial.iter().any(Option::is_some));
        assert_eq!(first_passes(&views, pass), serial);
    }

    /// A loaded launcher counts whatever else is carried: an empty bazooka
    /// beside a loaded panzerschreck, the pistol in hand.
    #[test]
    fn a_loaded_launcher_beside_an_empty_one_is_the_rocket() {
        let launcher = |w: u8| w == 5 || w == 43;
        let loaded = (1 << 9) | (1 << 43);
        assert_eq!(held_rocket(9, loaded, launcher), Some(43));
        assert_eq!(held_rocket(5, loaded, launcher), Some(43));
        assert_eq!(held_rocket(43, loaded | (1 << 5), launcher), Some(43));
        assert_eq!(held_rocket(9, 1 << 9, launcher), None);
    }

    /// A launcher's widest hip cone (12 degrees) out to its 256 splash and
    /// the margin (`Server::armor_view`): a wall 200 ahead of the muzzle
    /// fails it, one 400 ahead does not, and neither does the floor under
    /// a standing body's level view.
    #[test]
    fn a_wall_inside_the_splash_by_the_muzzle_closes_the_cone() {
        let muzzle = glam::Vec3::new(0.0, 0.0, 64.0);
        let reach = 256.0 + crate::bots::ROCKET_CLEAR_MARGIN;
        let wall = |x: f32| {
            vcod_common::collision::test_world(&[(
                glam::Vec3::new(x, -512.0, 0.0),
                glam::Vec3::new(x + 16.0, 512.0, 512.0),
            )])
        };
        let clear = |w: &vcod_common::collision::CollisionWorld| {
            cone_clear(w, &[], muzzle, glam::Vec3::X, 12.0, reach)
        };
        assert!(!clear(&wall(200.0)));
        assert!(clear(&wall(400.0)));
        // A wall beside the line, inside the cone's edge.
        let beside = vcod_common::collision::test_world(&[(
            glam::Vec3::new(100.0, 40.0, 0.0),
            glam::Vec3::new(300.0, 56.0, 512.0),
        )]);
        assert!(!clear(&beside));
        assert!(cone_clear(&beside, &[], muzzle, glam::Vec3::X, 4.0, reach));
    }

    use super::*;
    use crate::world::World;
    use vcod_common::collision::test_world;
    use vcod_common::net::connectionless::{
        build_connect, build_oob, info_value_for_key, parse_oob,
    };
    use vcod_common::net::msg::write_delta_usercmd;
    use vcod_common::net::netchan::Netchan;
    use vcod_common::net::protocol::PROTOCOL_V6;
    use vcod_common::net::protocol::PROTOCOL_V22;
    use vcod_common::net::snapshot::{SVC_SNAPSHOT, SnapshotRing};

    const QPORT: u16 = 0x2001;

    /// Every landing of the two 2026-10-05 retail runs, parm and maxhealth
    /// to damage (player-clip doc 8.10): the float `0.01` keeps each whole
    /// share just under, and past 99 the fall does 110 percent.
    /// `Cmd_FlameDamageClient_f`'s reading (game.mp.uo 0x4ee2c): `atoi` of
    /// the first three characters, kept in `0..=maxclients`, one argument
    /// only.
    #[test]
    fn an_fdc_argument_reads_as_retail_reads_it() {
        assert_eq!(fdc_inflictor("3", 20), Some(3));
        assert_eq!(fdc_inflictor("20", 20), Some(20));
        assert_eq!(fdc_inflictor("21", 20), None);
        assert_eq!(fdc_inflictor("100", 20), None);
        assert_eq!(fdc_inflictor("1000", 64), None);
        assert_eq!(fdc_inflictor("5000", 64), None);
        assert_eq!(fdc_inflictor("12345", 64), None);
        assert_eq!(fdc_inflictor("1234", 200), Some(123));
        assert_eq!(fdc_inflictor("x", 20), Some(0));
        assert_eq!(fdc_inflictor("4x", 20), Some(4));
        assert_eq!(fdc_inflictor("-1", 20), None);
        assert_eq!(fdc_inflictor("", 20), None);
        assert_eq!(fdc_inflictor("1 2", 20), None);
    }

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
        let version = sv.game.proto.transport.version;
        sv.handle_packet(from, &connect_pkt(challenge, QPORT, version), now);
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

    /// The checksum feed rides the seed, not how long the caller took to
    /// build the server: two servers off one seed key their pure checks
    /// and cmds alike.
    #[test]
    fn a_seeded_server_draws_the_same_checksum_feed() {
        let now = Instant::now();
        let a = Server::new(cfg(), now);
        let b = Server::new(cfg(), now - Duration::from_secs(5));
        assert_eq!(a.core.checksum_feed, b.core.checksum_feed);
        let c = Server::with_seed(cfg(), now, 7);
        assert_ne!(a.core.checksum_feed, c.core.checksum_feed);
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

    /// `SVC_Info` adds `game` (`fs_game`) after `pure` when it is set.
    #[test]
    fn getinfo_names_the_mod() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("fs_game", "mymod");
        sv.handle_packet(addr(5), &oob("getinfo 1"), now);
        let (_, _, rest) = reply(&mut sv);
        let info = String::from_utf8_lossy(&rest);
        assert!(info.ends_with("\\pure\\0\\game\\mymod\\sv_allowAnonymous\\0\\pswrd\\0"));
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

    /// `Cmd_Argv(1)` of a query, against what 1.1d, 1.5b and UO 1.51b all
    /// echoed as `challenge` (cod11-server-handshake.md, "The challenge
    /// key"); the info string's own stripping (`;`, `\`) comes after.
    #[test]
    fn oob_argv_reads_the_line_as_retail_does() {
        let cases: [(&[u8], &str); 12] = [
            (b"getinfo a\xc3\xa9b", "a..b"),
            (b"getinfo a%b", "a.b"),
            (b"getinfo a\x7fb", "a\x7fb"),
            (b"getinfo a\x01b", "a"),
            (b"getinfo a\nb", "a"),
            (b"getinfo a\"b", "a"),
            (b"getinfo \"a b\" c", "a b"),
            (b"getinfo a//b", "a"),
            (b"getinfo a/*x*/b", "a"),
            (b"getinfo /*x*/b", "b"),
            (b"getinfo \"ab", "ab"),
            (b"getinfo", ""),
        ];
        for (line, arg) in cases {
            let pkt = [&[0xff; 4][..], line].concat();
            assert_eq!(oob_argv(&pkt, 1), arg, "{line:?}");
        }
    }

    /// No argument, no `challenge` key: retail's `Info_SetValueForKey`
    /// leaves an empty value out, on every protocol.
    #[test]
    fn a_bare_query_sends_no_challenge_key() {
        let now = Instant::now();
        for proto in [&PROTOCOL_V1, &PROTOCOL_V6, &PROTOCOL_V22] {
            let mut sv = Server::new(cfg(), now);
            sv.set_protocol(proto);
            for (n, (query, answer)) in
                [("getinfo", "infoResponse"), ("getstatus", "statusResponse")]
                    .into_iter()
                    .enumerate()
            {
                sv.handle_packet(addr(10 + n as u16), &oob(query), now);
                let (_, cmd, rest) = reply(&mut sv);
                assert_eq!(cmd, answer);
                let text = String::from_utf8_lossy(&rest);
                assert!(
                    !text.contains("challenge"),
                    "{} {text}",
                    proto.transport.version
                );
            }
        }
        let mut sv = Server::new(cfg(), now);
        sv.handle_packet(addr(5), &oob("getinfo"), now);
        let (_, _, rest) = reply(&mut sv);
        assert_eq!(
            String::from_utf8_lossy(&rest),
            "\\protocol\\1\\hostname\\vcod test\\mapname\\mp_carentan\\clients\\0\\sv_maxclients\\4\\gametype\\dm\\pure\\0\\sv_allowAnonymous\\0\\pswrd\\0"
        );
    }

    /// A challenge that takes the info string past 1024 bytes: 1.1d drops
    /// its level or crashes, so vcod sends nothing; 1.5b and UO skip each
    /// pair that no longer fits and answer with the rest.
    #[test]
    fn an_overlong_challenge_overflows_as_retail_does() {
        use vcod_common::net::connectionless::MAX_INFO_STRING;
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.handle_packet(addr(5), &oob("getinfo x"), now);
        let base = reply(&mut sv).2.len() - "\\challenge\\x".len();
        let fits = MAX_INFO_STRING - base - "\\challenge\\".len();
        sv.handle_packet(addr(6), &oob(&format!("getinfo {}", "a".repeat(fits))), now);
        assert_eq!(reply(&mut sv).2.len(), MAX_INFO_STRING);
        sv.handle_packet(
            addr(7),
            &oob(&format!("getinfo {}", "a".repeat(fits + 1))),
            now,
        );
        assert!(sv.take_outgoing().is_empty());

        let mut sv = v6_server(now);
        let long = "a".repeat(1000);
        sv.handle_packet(addr(5), &oob(&format!("getinfo {long}")), now);
        let (_, _, rest) = reply(&mut sv);
        assert_eq!(
            String::from_utf8_lossy(&rest),
            format!("\\challenge\\{long}\\protocol\\6")
        );
        sv.handle_packet(addr(6), &oob(&format!("getstatus {long}")), now);
        let (_, _, rest) = reply(&mut sv);
        let text = String::from_utf8_lossy(&rest);
        assert!(!text.contains("challenge"), "{text}");
        assert!(text.ends_with("\\pswrd\\0\\mod\\1\n"), "{text}");
    }

    /// `fs_restrict` puts `demo` in front of `sv_keywords` (empty here) and
    /// the key after `challenge`; `getinfo` carries no keywords.
    #[test]
    fn fs_restrict_adds_the_demo_keyword() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("fs_restrict", "1");
        sv.handle_packet(addr(5), &oob("getstatus 7"), now);
        let (_, _, rest) = reply(&mut sv);
        let text = String::from_utf8_lossy(&rest);
        assert!(
            text.ends_with("\\challenge\\7\\sv_keywords\\demo \\pswrd\\0\n"),
            "{text}"
        );
        sv.handle_packet(addr(7), &oob("getinfo 7"), now);
        let (_, _, rest) = reply(&mut sv);
        assert!(!String::from_utf8_lossy(&rest).contains("demo"));
    }

    fn v6_server(now: Instant) -> Server {
        let mut sv = Server::new(cfg(), now);
        sv.set_protocol(&PROTOCOL_V6);
        sv
    }

    /// 1.5b's `infoResponse` against the capture in
    /// docs/research/cod15-protocol-diff.md 1: the zero-valued keys left out
    /// and five more after them. `kc` is `scr_killcam`'s string.
    #[test]
    fn protocol_6_answers_getinfo_as_15b_does() {
        let now = Instant::now();
        let mut sv = v6_server(now);
        sv.set_cvar("scr_killcam", "1");
        sv.handle_packet(addr(5), &oob("getinfo xyz"), now);
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(cmd, "infoResponse");
        assert_eq!(
            String::from_utf8_lossy(&rest),
            "\\challenge\\xyz\\protocol\\6\\hostname\\vcod test\\mapname\\mp_carentan\\sv_maxclients\\4\\gametype\\dm\\kc\\1\\hw\\1\\pb\\0\\mod\\1"
        );
        // A password and a client: `clients` counts the private slots too,
        // and `sv_maxclients` gives back the ones in use.
        sv.set_cvar("g_password", "secret");
        sv.set_cvar("sv_privateClients", "2");
        sv.core.clients[0] = Some(Client::new(addr(9), 1, 0, "\\name\\x".into(), now));
        sv.handle_packet(addr(5), &oob("getinfo 1"), now);
        let (_, _, rest) = reply(&mut sv);
        let info = String::from_utf8_lossy(&rest);
        assert!(
            info.contains("\\clients\\1\\sv_maxclients\\3\\gametype\\dm\\pswrd\\1\\kc\\1"),
            "{info}"
        );
    }

    /// 1.5b refuses `+set sv_punkbuster 1` as read-only and answers
    /// `pb 0`; vcod runs no PunkBuster either.
    #[test]
    fn protocol_6_refuses_punkbuster_on_the_command_line() {
        let now = Instant::now();
        let mut sv = v6_server(now);
        sv.set_cvar("sv_punkbuster", "1");
        sv.handle_packet(addr(5), &oob("getinfo x"), now);
        let (_, _, rest) = reply(&mut sv);
        assert!(String::from_utf8_lossy(&rest).ends_with("\\pb\\0\\mod\\1"));
    }

    /// 1.5b's `statusResponse` ends `challenge`, `pswrd`, `mod`, and its
    /// serverinfo names protocol 6.
    #[test]
    fn protocol_6_answers_getstatus_as_15b_does() {
        let now = Instant::now();
        let mut sv = v6_server(now);
        sv.handle_packet(addr(5), &oob("getstatus 7"), now);
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(cmd, "statusResponse");
        let text = String::from_utf8_lossy(&rest);
        let (info, _) = text.split_once('\n').unwrap();
        assert_eq!(info_value_for_key(info, "protocol"), Some("6"));
        assert_eq!(info_value_for_key(info, "shortversion"), Some("1.5"));
        assert!(info.ends_with("\\challenge\\7\\pswrd\\0\\mod\\1"), "{info}");
    }

    /// A 1.1 client is turned away with 1.5's version, which a 1.1 client
    /// shows and a vcod client retries in (`net::NetClient`).
    #[test]
    fn protocol_6_refuses_a_11_connect_with_its_version() {
        let now = Instant::now();
        let mut sv = v6_server(now);
        let challenge = challenge_for(&mut sv, addr(5), now);
        sv.handle_packet(addr(5), &connect_pkt(challenge, QPORT, 1), now);
        assert_eq!(
            reply_text(&mut sv),
            (
                "error".to_string(),
                "EXE_SERVER_IS_DIFFERENT_VER\x151.5".to_string()
            )
        );
        sv.handle_packet(addr(5), &connect_pkt(challenge, QPORT, 6), now);
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
    }

    fn uo_server(now: Instant) -> Server {
        let mut sv = Server::new(cfg(), now);
        sv.set_protocol(&PROTOCOL_V22);
        sv
    }

    /// UO 1.51b's `infoResponse` (coduo-diff.md 2, live on mp_cassino):
    /// 1.5's keys with the vehicle switches `jps` and `tnk` after
    /// `timeoutsAllowed`, and its refusal of another version's connect.
    #[test]
    fn protocol_22_answers_getinfo_and_refuses_as_uo_does() {
        let now = Instant::now();
        let mut sv = uo_server(now);
        // What the level's table holds on retail's stock cfg; no level runs
        // here.
        for name in [
            "scr_friendlyfire",
            "scr_killcam",
            "scr_allow_jeeps",
            "scr_allow_tanks",
        ] {
            sv.set_cvar(name, "1");
        }
        sv.handle_packet(addr(5), &oob("getinfo xyz"), now);
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(cmd, "infoResponse");
        assert_eq!(
            String::from_utf8_lossy(&rest),
            "\\challenge\\xyz\\protocol\\22\\hostname\\vcod test\\mapname\\mp_carentan\\sv_maxclients\\4\\gametype\\dm\\ff\\1\\kc\\1\\jps\\1\\tnk\\1\\hw\\1\\pb\\0\\mod\\1"
        );
        sv.handle_packet(addr(5), &oob("getstatus 7"), now);
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(cmd, "statusResponse");
        let status = String::from_utf8_lossy(&rest);
        // The serverinfo half is the level's cvar table, measured byte for
        // byte against retail in coduo-diff.md 20; the trailer is 1.5's.
        assert!(
            status.contains("\\protocol\\22\\")
                && status.contains("\\shortversion\\1.51\\")
                && status.ends_with("\\challenge\\7\\pswrd\\0\\mod\\1\n"),
            "{status}"
        );
        let challenge = challenge_for(&mut sv, addr(6), now);
        for refused in [1, 6] {
            sv.handle_packet(addr(6), &connect_pkt(challenge, QPORT, refused), now);
            assert_eq!(
                reply_text(&mut sv),
                (
                    "error".to_string(),
                    "EXE_SERVER_IS_DIFFERENT_VER\x151.51".to_string()
                )
            );
        }
        sv.handle_packet(addr(6), &connect_pkt(challenge, QPORT, 22), now);
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
    }

    /// A UO server's entities and playerstate go out in UO's numbering: the
    /// `eFlags` bits from 0x100 one place down, the events renumbered, an
    /// event entity's `eType` from `ET_EVENTS` 16 (coduo-diff.md 8, 11).
    #[test]
    fn protocol_22_writes_uos_event_and_flag_numbers() {
        use vcod_common::net::event_ids::{EV_FIRE_WEAPON, EV_OBITUARY};
        use vcod_common::net::flags::EF_FIRING;
        let p = &PROTOCOL_V22;
        let mut es = msg::EntityState::null(p);
        let set = |es: &mut msg::EntityState, name: &str, v: i32| {
            let i = msg::EntityState::field_index(p, name).unwrap();
            es.fields[i] = v;
        };
        set(&mut es, "eFlags", EF_FIRING | 0x20);
        set(&mut es, "events[1]", EV_FIRE_WEAPON);
        set(
            &mut es,
            "eType",
            vcod_common::net::events::ET_EVENTS + EV_OBITUARY,
        );
        crate::game::wire::entity_to_protocol(p, &mut es);
        assert_eq!(es.field_i32(p, "eFlags"), 0x200 | 0x20);
        assert_eq!(es.field_i32(p, "events[1]"), 163);
        assert_eq!(es.field_i32(p, "eType"), 16 + 209);
        // 1.1 is left alone.
        let mut es = msg::EntityState::null(&PROTOCOL_V1);
        let i = msg::EntityState::field_index(&PROTOCOL_V1, "eFlags").unwrap();
        es.fields[i] = EF_FIRING;
        crate::game::wire::entity_to_protocol(&PROTOCOL_V1, &mut es);
        assert_eq!(es.fields[i], EF_FIRING);
    }

    /// The 1.5 `ui_*` rows ride configstring 1 in folded-name order, with
    /// no `timescale`, as the 1.5b gamestate's does.
    #[test]
    fn protocol_6_systeminfo_carries_the_15_rows() {
        let sv = v6_server(Instant::now());
        let rows = sv.systeminfo_v6().unwrap();
        let info = configstrings::systeminfo(
            0x10,
            FallHeights::default(),
            false,
            &Default::default(),
            Some(&rows),
        )
        .to_string();
        assert!(
            info.starts_with(
                "\\bg_fallDamageMaxHeight\\480\\bg_fallDamageMinHeight\\256\\g_synchronousClients\\0\\pmove_fixed\\0\\pmove_msec\\8\\sv_cheats\\0\\sv_console_lockout\\0\\sv_disableClientConsole\\0\\sv_pure\\0\\sv_serverid\\16\\ui_allowVote\\1\\ui_allowVoteClientKick\\0"
            ),
            "{info}"
        );
        assert!(
            info.ends_with("\\ui_killcam\\1\\ui_timeoutBank\\180000\\ui_timeoutLength\\90000\\ui_timeoutRecovery\\10000\\ui_timeoutsAllowed\\0"),
            "{info}"
        );
        assert!(!info.contains("timescale"));
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

    /// Every shape `client-probes/probe_menuresponse` sent retail 1.1d, and
    /// what its `menuresponse` carried (gsc language doc 6). The response
    /// comes back verbatim: the gametype compares it against `"allies"` and
    /// weapon names.
    #[test]
    fn mr_answers_each_shape_as_retail() {
        let idx = |i| Some((MenuRef::Index(i), "hello".to_string()));
        let bad = Some((MenuRef::Name(String::new()), "bad".to_string()));
        assert_eq!(parse_menu_response("mr 7 3 hello", 7), idx(3));
        assert_eq!(parse_menu_response("mr 7 0x1 hello", 7), idx(0));
        assert_eq!(parse_menu_response("mr 7 1abc hello", 7), idx(1));
        assert_eq!(parse_menu_response("mr 7 31 hello", 7), idx(31));
        assert_eq!(parse_menu_response("mr 6 3 hello", 7), None, "stale");
        for cmd in ["mr 0 noop", "mr 9 bad", "mr", "mr 7 0", "mr 7 0 a b"] {
            assert_eq!(parse_menu_response(cmd, 7), bad, "{cmd}");
        }
        assert_eq!(
            parse_menu_response("mr 7 40 past", 7),
            Some((MenuRef::Name("40".into()), "past".into()))
        );
        assert_eq!(
            parse_menu_response("mr 7 -1 neg", 7),
            Some((MenuRef::Name("-1".into()), "neg".into()))
        );
        assert_eq!(
            parse_menu_response("mr 7 0 \"quoted word\"", 7),
            Some((MenuRef::Index(0), "quoted word".into()))
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

    /// Every bot's cmds of a tick go in as one packet, so `replay_moves`
    /// poses the bodies their rounds meet under one DObj stamp, as retail's
    /// single bump ahead of `SV_Frame`'s bot pass does (movers doc 15).
    #[test]
    fn every_bots_cmds_of_a_tick_share_one_packet() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            return;
        };
        let bsp_path = fs.resolve_map("mp_carentan").expect("map in the paks");
        let bsp = crate::world::World::from_bsp(
            &vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap(),
            Some(&fs),
        );
        let mut now = Instant::now();
        let mut sv = Server::new(ServerConfig { bots: 3, ..cfg() }, now);
        sv.load_world(bsp);
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        for _ in 0..100 {
            now += std::time::Duration::from_millis(FRAME_MS as u64);
            sv.tick(now);
        }
        let before = sv.game.packet_seq;
        crate::nav::with_rules(sv.move_rules(), || sv.step_bots());
        let packets: Vec<u64> = sv
            .core
            .clients
            .iter()
            .flatten()
            .filter(|c| c.is_bot)
            .flat_map(|c| c.pending.iter().map(|q| q.packet))
            .collect();
        assert!(packets.len() >= 3, "every bot queued a cmd: {packets:?}");
        assert!(packets.iter().all(|&p| p == before + 1), "{packets:?}");
    }

    /// The `weaponClass` table the animation machine reads every frame, and
    /// the seam it is built across: `WEAPON_LIST` is 1-based on the wire, so
    /// `items::item_name` subtracts one to name it. Misaligned by one, every
    /// pistol player animates as a rifleman and nothing says so. Needs the
    /// paks -- the classes come from the weapon files.
    #[test]
    fn the_weapon_class_table_lines_up_with_the_wire_index() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
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
                sv.game.module.weapon_table.class(index),
                class,
                "{weapon} at wire index {index}"
            );
            // The same index the frame loop hands the animscript.
            assert_eq!(
                crate::items::item_name(&crate::defs::V1, index),
                Some(weapon)
            );
        }
        assert_eq!(
            sv.game.module.weapon_table.class(0),
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
        let before: Vec<String> = sv.core.configstrings.clone();
        // No paks, so the map script does not resolve and `load` fails at its
        // first step.
        let err = sv.load_scripts(Rc::new(vcod_common::pk3::Pk3Fs::empty()));
        assert!(err.is_err());
        assert_eq!(sv.core.configstrings, before);
        assert!(sv.configstring(7).contains("kar98k_mp"));
    }

    /// The script keeps allocating configstrings after map load, from any
    /// thread that has passed a `wait` (`G_SoundAliasIndex` has no
    /// `level.initializing` gate). The server re-reads the script's table
    /// each tick, so such an allocation reaches it.
    #[test]
    fn a_configstring_allocated_after_a_wait_reaches_the_server() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test(
                "main() { e = spawn(\"script_origin\", (0, 0, 0)); wait 0.5; e playsound(\"late_alias\"); }",
            ),
        );
        let slot = vcod_common::net::protocol::CS_SOUNDS + 1;
        // sv_time starts at 0 and each tick advances it one 50 ms frame, so
        // the thread is still suspended after the first.
        sv.tick(now);
        assert_eq!(sv.configstring(slot), "");
        for _ in 0..12 {
            sv.tick(now);
        }
        assert_eq!(sv.configstring(slot), "late_alias");
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

    /// A script `setCvar` of a serverinfo cvar reaches configstring 0 and
    /// `getinfo` by the next frame, as retail's `SV_Frame` flush makes it
    /// (`sv_privateClients 12` on 8 slots read `sv_maxclients -4` live; 4
    /// slots here).
    #[test]
    fn a_script_write_of_a_serverinfo_cvar_reaches_configstring_0() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test(
                "main() { wait 0.1; setCvar(\"sv_privateClients\", \"12\"); setCvar(\"sv_hostname\", \"scripted\"); }",
            ),
        );
        sv.tick(now);
        assert!(sv.configstring(0).contains("\\sv_privateClients\\0"));
        for _ in 0..4 {
            sv.tick(now);
        }
        let cs0 = sv.configstring(0).to_string();
        assert_eq!(info_value_for_key(&cs0, "sv_privateClients"), Some("12"));
        assert_eq!(info_value_for_key(&cs0, "sv_hostname"), Some("scripted"));
        sv.take_outgoing();
        sv.handle_packet(addr(20), &oob("getinfo x"), now);
        let (_, _, rest) = reply(&mut sv);
        let info = String::from_utf8_lossy(&rest).to_string();
        assert_eq!(info_value_for_key(&info, "sv_maxclients"), Some("-8"));
        assert_eq!(info_value_for_key(&info, "hostname"), Some("scripted"));
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
            sv.core.clients[0].as_ref().unwrap().state,
            ClientState::Connected
        );

        let seq = sv.core.clients[0]
            .as_ref()
            .unwrap()
            .link
            .netchan
            .reliable_sequence;
        sv.core.configstrings[524] = "MP_announcer_allies_win".to_string();
        sv.broadcast_configstring_changes();
        assert_eq!(
            sv.core.clients[0]
                .as_ref()
                .unwrap()
                .link
                .netchan
                .reliable_sequence,
            seq,
            "a client with no gamestate was sent a `d`"
        );

        // The same slot moving again once the gamestate has gone out does
        // reach it, so the skip above is the state and not the diff.
        sv.core.clients[0].as_mut().unwrap().state = ClientState::Primed;
        sv.core.configstrings[524] = "MP_announcer_axis_win".to_string();
        sv.broadcast_configstring_changes();
        let c = sv.core.clients[0].as_ref().unwrap();
        assert_eq!(c.link.netchan.reliable_sequence, seq + 1);
        assert_eq!(
            c.link.netchan.reliable
                [c.link.netchan.reliable_sequence as usize & (MAX_RELIABLE_COMMANDS - 1)],
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
        let Some(fs) = vcod_common::testing::game_fs_11() else {
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
        // first of the three, and a status icon is one precache retail
        // takes past the first wait.
        sv.overlay_script(
            "maps/mp/gametypes/settle",
            "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); \
             wait 0.05; precacheStatusIcon(\"gfx/hud/settle\"); }",
        );
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        sv.spawn_server("mp_brecourt").expect("the map load failed");
        // What the next tick reads back out of the script, which is what it
        // diffs the sent copy against.
        let rt = sv
            .game
            .module
            .script
            .as_ref()
            .expect("the spawn left no script");
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
            stale(&sv.core.sent_configstrings).is_empty(),
            "the spawn left slots {:?} for the diff to send",
            stale(&sv.core.sent_configstrings)
        );
        assert!(
            stale(&sv.core.configstrings).is_empty(),
            "the server's own copy is a frame behind the script at slots {:?}",
            stale(&sv.core.configstrings)
        );
    }

    /// A restart starts the level on the compile the outgoing level loaded;
    /// a map load and a new overlay compile again (map-cycle doc, 4.7).
    /// `games_mp.log` across a `map_restart`, a map load and `quit`, as
    /// the retail 1.1d run wrote it (cod11-map-cycle.md 9): the separator
    /// and `InitGame:` at each `G_InitGame`, `RestartGame:` closing the
    /// restarted level, `ShutdownGame:` the others, the script's
    /// `logPrint` between.
    #[test]
    fn games_mp_log_follows_the_levels() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let home = std::env::temp_dir().join(format!("vcod-cod1-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("fs_homepath", &home.display().to_string());
        sv.overlay_script(
            "maps/mp/gametypes/dm",
            "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); logPrint(\"hi\\n\"); }",
        );
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        sv.tick(now);
        sv.map_restart().expect("the restart failed");
        sv.tick(now);
        sv.spawn_server("mp_carentan").expect("the map load failed");
        sv.tick(now);
        sv.push_console("quit");
        sv.tick(now);
        let text = std::fs::read_to_string(home.join("main/games_mp.log")).unwrap();
        let sep = "-".repeat(60);
        let lines: Vec<String> = text
            .lines()
            .map(|l| match l[7..].strip_prefix("InitGame: \\") {
                Some(info) => {
                    assert!(info.contains("\\mapname\\mp_carentan\\"), "{l}");
                    "InitGame".to_string()
                }
                None => l[7..].to_string(),
            })
            .collect();
        let level = [sep.as_str(), "InitGame", "hi"];
        let mut want: Vec<&str> = level.to_vec();
        want.extend(["RestartGame:", &sep]);
        want.extend(level);
        want.extend(["ShutdownGame:", &sep]);
        want.extend(level);
        want.extend(["ShutdownGame:", &sep]);
        assert_eq!(lines, want, "{text}");
        assert!(text.lines().all(|l| l.as_bytes()[3] == b':'), "{text}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_restart_reuses_the_compiled_scripts_and_a_map_load_does_not() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let compiled = |sv: &Server| {
            sv.game
                .module
                .script
                .as_ref()
                .and_then(|rt| rt.compiled().cloned())
                .expect("a loaded level keeps its compile")
        };
        let first = compiled(&sv);
        sv.tick(now);
        sv.map_restart().expect("the restart failed");
        assert!(
            Rc::ptr_eq(&first, &compiled(&sv)),
            "the restart compiled again"
        );
        sv.tick(now);
        sv.spawn_server("mp_carentan").expect("the map load failed");
        let spawned = compiled(&sv);
        assert!(
            !Rc::ptr_eq(&first, &spawned),
            "the map load kept the old compile"
        );
        sv.tick(now);
        sv.overlay_script(
            "maps/mp/gametypes/dm",
            "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); }",
        );
        sv.map_restart().expect("the restart failed");
        assert!(
            !Rc::ptr_eq(&spawned, &compiled(&sv)),
            "the restart ran the old text after a new overlay"
        );
    }

    /// `scriptUsage` on retail 1.1d, mp_brecourt `tdm` (handshake doc,
    /// "Console commands over rcon"): the gametype's two waits, then the
    /// 32 minefield threads, in code address order, not by file name.
    #[test]
    fn script_usage_orders_files_as_retail() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(
            ServerConfig {
                map: "mp_brecourt".into(),
                gametype: "tdm".into(),
                ..cfg()
            },
            now,
        );
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        for _ in 0..10 {
            sv.tick(now);
        }
        let text = sv.game.module.script.as_mut().unwrap().script_usage();
        // The var usage numbers are vcod's own VM's; the counts and the
        // order are retail's.
        let heads: Vec<&str> = text
            .lines()
            .filter_map(|l| match l.starts_with("count") {
                true => l.split(',').next(),
                false => l.starts_with("(file").then_some(l),
            })
            .collect();
        assert_eq!(
            heads,
            [
                "count: 1",
                "(file 'maps\\mp\\gametypes\\tdm.gsc', line 992)",
                "count: 1",
                "(file 'maps\\mp\\gametypes\\tdm.gsc', line 1184)",
                "count: 32",
                "(file 'maps\\mp\\_minefields.gsc', line 19)",
            ],
            "{text}"
        );
    }

    /// `probe_startclock` on retail: the script clock starts on the load's
    /// `level.time`, the settle frames walk it 100 ms at a time, and a thread
    /// due at a frame's `level.time` runs the frame after
    /// (docs/research/cod11-gsc-language.md, "The script clock").
    #[test]
    fn the_script_clock_matches_probe_startclock() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(
            ServerConfig {
                gametype: "probe_startclock".into(),
                ..cfg()
            },
            now,
        );
        sv.overlay_script(
            "maps/mp/gametypes/probe_startclock",
            include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_startclock.gsc"),
        );
        // The first load runs the settle frames a map change does.
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        for _ in 0..36 {
            sv.tick(now);
        }
        let mut log: Vec<String> = sv
            .script_log()
            .iter()
            .map(|l| l.trim().to_string())
            .collect();
        // The `wait 0.05` loop's 6..10 rows add nothing past its 5th.
        log.retain(|l| {
            !matches!(
                l.as_str(),
                "PROBE tick 7 450" | "PROBE tick 8 500" | "PROBE tick 9 550" | "PROBE tick 10 600"
            )
        });
        assert_eq!(
            log,
            [
                "PROBE main 0",
                "PROBE start 0",
                "PROBE wait0 100",
                "PROBE tick 0 100",
                "PROBE tick 1 200",
                "PROBE wait01 200",
                "PROBE tick 2 200",
                "PROBE tick 3 300",
                "PROBE tick 4 300",
                "PROBE tick 5 350",
                "PROBE wait02 350",
                "PROBE tick 6 400",
                "PROBE tick 11 650",
                "PROBE wait1 1050",
                "PROBE wait1b 2050",
            ]
        );
    }

    /// The first map's baselines against the retail gamestate fixture
    /// (`mp_carentan` dm): every entity a snapshot can carry is baselined
    /// after the settle frames, as it stood then, so the first snapshot
    /// writes each as a delta against itself. The turrets' barrels are two
    /// slew steps down from 0. The trigger brush and the script origins are
    /// linked `SVF_NOCLIENT` and baselined all the same (map-cycle doc 3.3).
    #[test]
    fn the_first_maps_baselines_match_retail() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let p = &PROTOCOL_V1;
        let huff = Huffman::new();
        let gs_bytes = vcod_common::testing::fixture("net/gamestate.bin");
        let mut r = MsgReader::new(&gs_bytes[4..], &huff);
        let retail = vcod_common::net::gamestate::parse(&mut r, p)
            .unwrap()
            .baselines;

        let fs = Rc::new(fs);
        let mut sv = Server::new(cfg(), Instant::now());
        let path = fs.resolve_map("mp_carentan").expect("map in the paks");
        let bsp =
            vcod_common::bsp::parse(&fs.read(&path).expect("read the bsp")).expect("parse the bsp");
        sv.load_world(World::from_bsp(&bsp, Some(&fs)));
        sv.load_scripts(fs).expect("load the scripts");

        let mut ours: Vec<u32> = sv.game.baselines.keys().copied().collect();
        ours.sort_unstable();
        let mut want: Vec<u32> = retail.keys().copied().collect();
        want.sort_unstable();
        assert_eq!(ours, want);
        for n in ours {
            let (a, b) = (&sv.game.baselines[&n], &retail[&n]);
            for (f, (x, y)) in p.entity_fields.iter().zip(a.fields.iter().zip(&b.fields)) {
                assert_eq!(x, y, "entity {n} {}", f.name);
            }
        }
    }

    /// mp_cassino's `script_origin` "auto9" (entity 414) lists `origin`
    /// before `angles` "0 2.50448e-006 0": applied in lump order the yaw
    /// stays unsnapped and goes out as a sent 0, as the retail UO
    /// gamestate's baseline carries it; `angles` first, the `origin` link
    /// snapped it and the baseline left it out (object-model doc 7).
    #[test]
    fn uo_cassinos_auto9_baseline_matches_retail() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            eprintln!("COD_DIR is not a UO install: skipping");
            return;
        };
        let p = &PROTOCOL_V22;
        let huff = Huffman::new();
        let gs_bytes = vcod_common::testing::fixture("net/gamestate-uo.bin");
        let mut r = MsgReader::new(&gs_bytes[4..], &huff);
        let retail = vcod_common::net::gamestate::parse(&mut r, p)
            .unwrap()
            .baselines;
        let fs = Rc::new(fs);
        let mut sv = Server::new(
            ServerConfig {
                map: "mp_cassino".into(),
                max_clients: 8,
                ..cfg()
            },
            Instant::now(),
        );
        sv.set_protocol(p);
        let path = fs.resolve_map("mp_cassino").expect("map in the paks");
        let bsp = vcod_common::bsp::parse(&fs.read(&path).unwrap()).unwrap();
        sv.load_world(World::from_bsp(&bsp, Some(&fs)));
        sv.load_scripts(fs).expect("load the scripts");
        let null = msg::EntityState::null(p);
        let wire = |e: &msg::EntityState| {
            let mut w = MsgWriter::new(&huff);
            msg::write_delta_entity(&mut w, p, &null, Some(e));
            w.finish()
        };
        assert_eq!(wire(&sv.game.baselines[&414]), wire(&retail[&414]));
    }

    /// The level script's cvar table carries `mapname`, the probes' only way
    /// to tell which map they run on; it used to read "".
    #[test]
    fn a_level_script_reads_the_map_name_cvar() {
        let sv = Server::new(cfg(), Instant::now());
        let cvars = sv.cvars(&vcod_common::pk3::Pk3Fs::empty());
        assert_eq!(cvars.get("mapname"), "mp_carentan");
    }

    /// Doc 4.5: `Com_StartupVariable` runs from `Com_Init` only, so a
    /// `--set` goes into the first level's table and a script's later
    /// `setCvar` of it survives a restart and a map change. Each load of
    /// this gametype appends to the cvar; a replay of the `--set` at each
    /// load would keep resetting it to one `x`.
    #[test]
    fn a_set_applies_once_and_a_scripts_write_outlives_later_loads() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let mut now = Instant::now();
        let mut sv = Server::new(
            ServerConfig {
                gametype: "trail".into(),
                ..cfg()
            },
            now,
        );
        sv.overlay_script(
            "maps/mp/gametypes/trail",
            "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); \
             setCvar(\"vc_trail\", getCvar(\"vc_trail\") + \"x\"); }",
        );
        sv.set_cvar("vc_trail", "a");
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        assert_eq!(sv.script_cvar("vc_trail").as_deref(), Some("ax"));
        now += Duration::from_millis(50);
        sv.push_console("map_restart");
        sv.tick(now);
        assert_eq!(sv.script_cvar("vc_trail").as_deref(), Some("axx"));
        sv.spawn_server("mp_brecourt").expect("the map load failed");
        assert_eq!(sv.script_cvar("vc_trail").as_deref(), Some("axxx"));

        // A set while the level runs is `Cvar_Set` on the live table, and
        // the next load builds on it.
        sv.set_cvar("vc_trail", "b");
        assert_eq!(sv.script_cvar("vc_trail").as_deref(), Some("b"));
        sv.spawn_server("mp_carentan").expect("the map load failed");
        assert_eq!(sv.script_cvar("vc_trail").as_deref(), Some("bx"));
    }

    /// `sv_hostname` is a cvar like any other: the config's goes in at the
    /// first load and a script's `setCvar` of it outlives a restart and a
    /// map change (map-cycle doc 4.5).
    #[test]
    fn a_scripts_hostname_outlives_later_loads() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let mut now = Instant::now();
        let mut sv = Server::new(
            ServerConfig {
                gametype: "trail".into(),
                ..cfg()
            },
            now,
        );
        sv.overlay_script(
            "maps/mp/gametypes/trail",
            "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); \
             setCvar(\"sv_hostname\", getCvar(\"sv_hostname\") + \"x\"); }",
        );
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        assert_eq!(sv.script_cvar("sv_hostname").as_deref(), Some("vcod testx"));
        now += Duration::from_millis(50);
        sv.push_console("map_restart");
        sv.tick(now);
        assert_eq!(
            sv.script_cvar("sv_hostname").as_deref(),
            Some("vcod testxx")
        );
        sv.spawn_server("mp_brecourt").expect("the map load failed");
        assert_eq!(
            sv.script_cvar("sv_hostname").as_deref(),
            Some("vcod testxxx")
        );
        let cs0 = sv.configstring(0).to_string();
        assert_eq!(
            info_value_for_key(&cs0, "sv_hostname"),
            Some("vcod testxxx")
        );
    }

    /// Doc 4.5: a `+set` after `+map` runs again once the level is up
    /// (`Com_AddStartupCommands`), so it outranks the script's load-time
    /// write, as `scr_allow_vote 0` read back on 1.1d over `dm.gsc`'s
    /// `setCvar("scr_allow_vote", level.allowvote)`.
    #[test]
    fn a_set_after_the_map_outranks_the_scripts_load_time_write() {
        // 1.1's dm.gsc writes scr_allow_vote at load; UO's does not.
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR is not a 1.1 install: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("scr_allow_vote", "0");
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        assert_eq!(
            sv.script_cvar("scr_allow_vote").as_deref(),
            Some("1"),
            "dm.gsc no longer writes scr_allow_vote at load"
        );
        sv.push_set("scr_allow_vote", "0");
        sv.drain_console(now);
        assert_eq!(sv.script_cvar("scr_allow_vote").as_deref(), Some("0"));
    }

    /// `map_rotate`'s `gametype` token is a `Cvar_Set` (doc section 5.2), so
    /// it has to outrank an earlier `--set g_gametype`: the token picks which
    /// gametype script loads, and the cvar table is what that script's own
    /// `getCvar` answers from. Writing only `cfg.gametype` splits the two.
    #[test]
    fn a_rotation_gametype_token_outranks_an_earlier_set() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
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
        assert_eq!(sv.game.cfg.map, "mp_brecourt", "the rotation did not load");
        assert_eq!(sv.game.cfg.gametype, "tdm");
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

    /// Doc section 5.2: a script's `setCvar("sv_mapRotation", ...)` is the
    /// rotation the next `map_rotate` runs, and the queue it leaves is
    /// `sv_mapRotationCurrent` in the same table.
    #[test]
    fn a_rotation_a_script_set_drives_map_rotate() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR not at a 1.1 install: skipping");
            return;
        };
        let fs = Rc::new(fs);
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("g_gametype", "dm");
        let path = fs.resolve_map("mp_carentan").expect("map in the paks");
        let bsp =
            vcod_common::bsp::parse(&fs.read(&path).expect("read the bsp")).expect("parse the bsp");
        sv.load_world(World::from_bsp(&bsp, Some(&fs)));
        sv.load_scripts(fs).expect("load the scripts");
        sv.game.module.script.as_mut().unwrap().host.cvars.set(
            "sv_mapRotation",
            "gametype tdm map mp_brecourt map mp_carentan",
        );

        sv.push_console("map_rotate");
        sv.tick(now);
        assert_eq!(
            sv.game.cfg.map, "mp_brecourt",
            "the script's rotation did not load"
        );
        assert_eq!(sv.game.cfg.gametype, "tdm");
        assert_eq!(
            sv.script_cvar("sv_mapRotationCurrent").as_deref(),
            // Retail keeps the blank that ended `mp_brecourt` (doc 5.1).
            Some(" map mp_carentan"),
        );
    }

    /// Map-cycle doc 4.8: `G_InitGame` reseeds the script's generator at
    /// every load, a restart too, so a restarted level does not replay the
    /// last one's draws (UO hq's radio pick, coduo-diff.md 22).
    #[test]
    fn a_restart_reseeds_the_script_draws() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR not at 1.1 install: skipping");
            return;
        };
        let fs = Rc::new(fs);
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("g_gametype", "dm");
        let path = fs.resolve_map("mp_carentan").expect("map in paks");
        let bsp = vcod_common::bsp::parse(&fs.read(&path).expect("read bsp")).expect("parse bsp");
        sv.load_world(World::from_bsp(&bsp, Some(&fs)));
        sv.load_scripts(fs).expect("load scripts");
        let draws = |sv: &mut Server| -> Vec<i32> {
            let host = &mut sv.game.module.script.as_mut().unwrap().host;
            (0..4).map(|_| host.rand_int()).collect()
        };
        let first = draws(&mut sv);
        let id = sv.game.server_id;
        sv.push_console("map_restart");
        sv.tick(now);
        assert_ne!(sv.game.server_id, id, "the restart did not run");
        assert_ne!(draws(&mut sv), first, "the restart replayed the draws");
    }

    /// Doc section 4 step 3: a `g_gametype` that moved since the level
    /// loaded turns a restart into a full spawn of the same map, so the
    /// serverId's high nibble climbs where a restart would have moved the
    /// low one. A level that asked to keep its persistence is exempt.
    #[test]
    fn a_gametype_change_escalates_a_restart_to_a_spawn() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let fs = Rc::new(fs);
        let mut now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_scripts(fs).expect("load the scripts");
        let before = sv.game.server_id;

        // No change yet: the plain restart path.
        sv.push_console("map_restart");
        sv.tick(now);
        assert_eq!(sv.game.server_id, console::next_restart_id(before));

        // The rotation's `gametype` token, then a restart: the level is
        // still running dm, so this escalates.
        let id = sv.game.server_id;
        sv.set_cvar("g_gametype", "tdm");
        now += Duration::from_millis(50);
        sv.push_console("map_restart");
        sv.tick(now);
        assert_eq!(
            sv.game.server_id,
            console::next_map_id(id),
            "a gametype change did not escalate to a spawn"
        );
        assert_eq!(
            sv.game.cfg.map, "mp_carentan",
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
        let Some(fs) = vcod_common::testing::game_fs_11() else {
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
        assert_eq!(
            sv.game.cfg.map, "mp_carentan",
            "the failed map load moved off"
        );

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
        let Some(fs) = vcod_common::testing::game_fs_11() else {
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
            assert!(
                sv.core.clients[0].is_none(),
                "spawn {spawn}: the client stayed"
            );
            let z = sv.game.zombies[0].as_ref().expect("a zombie");
            let at = z.link.netchan.reliable_sequence as usize & (MAX_RELIABLE_COMMANDS - 1);
            assert_eq!(z.link.netchan.reliable[at], "w \"GAME_INVALIDPASSWORD\"");
        }
    }
    /// Doc section 4 step 11: `SV_ClientEnterWorld` runs only for a client
    /// that reads `CS_ACTIVE`. A `CS_PRIMED` one keeps its state through
    /// the restart and is promoted by its own next message instead (4.4),
    /// which is the branch the second half exercises.
    #[test]
    fn a_primed_client_is_not_re_entered_by_a_restart() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let mut nc = active(&mut sv, now);
        assert_eq!(
            sv.core.clients[0].as_ref().unwrap().state,
            ClientState::Primed
        );

        sv.map_restart().expect("the restart failed");
        let c = sv.core.clients[0].as_ref().unwrap();
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
        let c = sv.core.clients[0].as_ref().unwrap();
        assert_eq!(c.state, ClientState::Active);
        assert!(c.sim.is_some());
    }

    /// The burst ahead of the restart's `n` carries every slot the incoming
    /// level wrote to a new value: the outgoing level's team score, which
    /// `ExitLevel` zeroes in the game only, goes out as `d 6 0` when the next
    /// level sets it (coduo-diff.md 22, "Round and map ends").
    #[test]
    fn a_restart_sends_the_slots_the_incoming_level_changed() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let huff = Huffman::new();
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.overlay_script(
            "maps/mp/gametypes/dm",
            "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); \
             if (getcvar(\"probe_restarted\") == \"\") { setcvar(\"probe_restarted\", \"1\"); \
             setTeamScore(\"allies\", 1); } else setTeamScore(\"allies\", 0); }",
        );
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let mut nc = begun(&mut sv, now);
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..4 {
            sv.tick(now);
            for (_, pkt) in sv.take_outgoing() {
                server_commands(&mut nc, &pkt, &huff);
            }
        }
        assert_eq!(sv.configstring(6), "1");

        sv.map_restart().expect("the restart failed");
        for _ in 0..4 {
            sv.tick(now);
            for (_, pkt) in sv.take_outgoing() {
                seen.extend(server_commands(&mut nc, &pkt, &huff));
            }
        }
        let d = seen.iter().position(|c| c == "d 6 0");
        let n = seen.iter().position(|c| c == "n");
        assert!(
            matches!((d, n), (Some(d), Some(n)) if d < n),
            "no `d 6 0` ahead of the `n` in {seen:?}"
        );
    }

    /// The burst goes in the incoming level's write order, not slot order:
    /// `SV_SetConfigstring` queues each `d` as the game writes it (map-cycle
    /// doc 4.5). The allies' 6 is written ahead of the axis' 5 here.
    #[test]
    fn a_restart_s_burst_keeps_the_write_order() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let huff = Huffman::new();
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.overlay_script(
            "maps/mp/gametypes/dm",
            "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); \
             if (getcvar(\"probe_restarted\") == \"\") setcvar(\"probe_restarted\", \"1\"); \
             else { setTeamScore(\"allies\", 3); setTeamScore(\"axis\", 2); } }",
        );
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let mut nc = begun(&mut sv, now);
        for _ in 0..4 {
            sv.tick(now);
            for (_, pkt) in sv.take_outgoing() {
                server_commands(&mut nc, &pkt, &huff);
            }
        }
        sv.map_restart().expect("the restart failed");
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..4 {
            sv.tick(now);
            for (_, pkt) in sv.take_outgoing() {
                seen.extend(server_commands(&mut nc, &pkt, &huff));
            }
        }
        let at = |s: &str| seen.iter().position(|c| c == s);
        assert!(
            matches!((at("d 6 3"), at("d 5 2")), (Some(a), Some(b)) if a < b),
            "the allies' `d 6` not ahead of the axis' `d 5` in {seen:?}"
        );
    }

    /// The other half of step 3: a `+set` that names one of the two cvars is
    /// not a change. `cvars` replays the override list after stamping the
    /// config's own value, so `--max-clients 4 --set sv_maxclients=8` loads
    /// the level with 8; comparing that against the config's 4 would escalate
    /// every restart for the whole run.
    #[test]
    fn a_set_override_of_sv_maxclients_is_not_a_gametype_change() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        assert_eq!(sv.game.cfg.max_clients, 4);
        sv.set_cvar("sv_maxclients", "8");
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        assert_eq!(sv.script_cvar("sv_maxclients").as_deref(), Some("8"));

        let before = sv.game.server_id;
        sv.push_console("map_restart");
        sv.tick(now);
        assert_eq!(
            sv.game.server_id,
            console::next_restart_id(before),
            "the override escalated a restart to a spawn"
        );
    }

    /// Doc section 4 step 1: a second restart inside one frame is a no-op,
    /// and so is one straight after a spawn.
    #[test]
    fn a_second_restart_in_one_frame_is_a_no_op() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let before = sv.game.server_id;
        sv.push_console("map_restart");
        sv.push_console("map_restart");
        sv.tick(now);
        assert_eq!(
            sv.game.server_id,
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
            &connect_pkt(challenge, QPORT, PROTOCOL_V1.transport.version),
            now,
        );
        let (to, cmd, _) = reply(&mut sv);
        assert_eq!(to, addr(5));
        assert_eq!(cmd, "connectResponse");
        assert_eq!(sv.client_count(), 1);
    }

    /// A reconnect overwrites a slot the server may still believe is live
    /// (`sv_timeout` is 240 s, `RECONNECT_LIMIT` 3). 1.5's and UO's
    /// `SV_FreeClient` runs the disconnect callback inside the connect; 1.1d
    /// runs none and starts the held player over, its threads and script
    /// fields kept and `pers` fresh (`client-probes/probe_reconnect`).
    /// 1.1's reconnect zeroes the held player's entity without an unlink:
    /// the area tree keeps the entry where it was filed, and a walk reads
    /// it with no contents.
    #[test]
    fn a_reconnect_on_1_1_leaves_the_area_entry() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        let nc = connected(&mut sv, addr(5), now);
        sv.tick(now);
        sv.take_outgoing();
        fn area(sv: &mut Server) -> &mut crate::area::AreaTree {
            &mut sv.game.module.script.as_mut().unwrap().host.area
        }
        let link = crate::area::Link::boxed([0.0; 3], [-15.0; 3], [15.0; 3], 0x2000000);
        area(&mut sv).link(0, &link);
        let filed = area(&mut sv).filed(0);
        assert!(filed.is_some());
        let t = now + RECONNECT_LIMIT + Duration::from_millis(100);
        sv.handle_packet(addr(5), &connect_pkt(nc.challenge, QPORT, 1), t);
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
        sv.tick(t);
        assert_eq!(area(&mut sv).filed(0), filed, "still filed where it was");
        assert_eq!(
            area(&mut sv).contents(0),
            0,
            "the walk reads zeroed contents"
        );
    }

    #[test]
    fn a_reconnect_into_a_held_slot_follows_the_protocol() {
        use crate::game::script::{CALLBACK_SETUP, ScriptRuntime};
        for (proto, callbacks) in [(&PROTOCOL_V1, 0), (&PROTOCOL_V6, 1)] {
            let now = Instant::now();
            let mut sv = Server::new(cfg(), now);
            sv.set_protocol(proto);
            install_script(
                &mut sv,
                ScriptRuntime::for_test_at(
                    CALLBACK_SETUP,
                    "main() { level.gone = 0; level.gens = 0; level.ticks = 0; }\n\
                     CodeCallback_PlayerConnect() { level.field = isdefined(self.mark); \
                       level.pers = isdefined(self.pers[\"mark\"]); self.mark = 1; \
                       self.pers[\"mark\"] = 1; level.gens++; self thread tick(level.gens); }\n\
                     tick(gen) { for(;;) { wait 0.05; if (gen == 1) level.ticks++; } }\n\
                     CodeCallback_PlayerDisconnect() { level.gone++; }\n",
                ),
            );
            let nc = connected(&mut sv, addr(5), now);
            sv.tick(now);
            sv.take_outgoing();
            let field =
                |sv: &mut Server, f: &str| sv.game.module.script.as_mut().unwrap().level_field(f);

            // Past sv_reconnectlimit, the same peer's connect takes the slot back.
            let t = now + RECONNECT_LIMIT + Duration::from_millis(100);
            sv.handle_packet(
                addr(5),
                &connect_pkt(nc.challenge, QPORT, proto.transport.version),
                t,
            );
            assert_eq!(reply_text(&mut sv).0, "connectResponse");
            sv.tick(t);
            let ticks = field(&mut sv, "ticks");
            sv.tick(t + Duration::from_millis(100));
            let v = proto.transport.version;
            assert_eq!(
                field(&mut sv, "gone"),
                vcod_gsc::Value::Int(callbacks),
                "{v}"
            );
            assert_eq!(field(&mut sv, "gens"), vcod_gsc::Value::Int(2), "{v}");
            assert_eq!(field(&mut sv, "pers"), vcod_gsc::Value::Int(0), "{v}");
            let held = callbacks == 0;
            assert_eq!(
                field(&mut sv, "field"),
                vcod_gsc::Value::Int(held as i32),
                "{v}"
            );
            let ticking = field(&mut sv, "ticks") != ticks;
            assert_eq!(ticking, held, "{v}: the old player's thread");
            assert_eq!(sv.client_count(), 1);
        }
    }

    /// The disconnect callback's commands to the leaver alone go into its
    /// ring, and the zombie's `w` squashes the unsent prints: retail's
    /// zombie gets the `setClientCvar` and not the `iPrintLn` on all three
    /// (`client-probes/probe_reconnect`).
    #[test]
    fn a_leavers_zombie_keeps_the_callbacks_reliable_commands_only() {
        use crate::game::script::{CALLBACK_SETUP, ScriptRuntime};
        for proto in [&PROTOCOL_V1, &PROTOCOL_V6] {
            let now = Instant::now();
            let mut sv = Server::new(cfg(), now);
            sv.set_protocol(proto);
            install_script(
                &mut sv,
                ScriptRuntime::for_test_at(
                    CALLBACK_SETUP,
                    "main() {}\n\
                     CodeCallback_PlayerDisconnect() { self iprintln(\"self\"); \
                       self setclientcvar(\"probe_self\", 1); }\n",
                ),
            );
            begun(&mut sv, now);
            sv.tick(now);
            sv.take_outgoing();
            sv.send_server_command(0, "e \"unsent print\"");
            sv.drop_client(0, "EXE_DISCONNECTED");
            let z = sv.game.zombies[0].as_ref().expect("a zombie");
            let ring: Vec<&str> = (z.link.reliable_sent + 1
                ..=z.link.netchan.reliable_sequence as i32)
                .map(|i| z.link.netchan.reliable[i as usize & (MAX_RELIABLE_COMMANDS - 1)].as_str())
                .collect();
            assert_eq!(
                ring,
                ["v probe_self \"1\"", "w \"EXE_DISCONNECTED\""],
                "{}",
                proto.transport.version
            );
        }
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
            &connect_pkt(
                challenge.wrapping_add(1),
                QPORT,
                PROTOCOL_V1.transport.version,
            ),
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
            &connect_pkt(challenge, QPORT + 1, PROTOCOL_V1.transport.version),
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
            PROTOCOL_V1.transport.version
        );
        sv.handle_packet(from, &build_connect(&ui), now);
        reply_text(sv)
    }

    /// `sv_minPing` / `sv_maxPing` against the challenge ping, off the LAN
    /// only, and the two `getinfo` keys that appear while either is set
    /// (handshake doc, "Ping limits").
    #[test]
    fn ping_limits_refuse_a_connect_off_the_lan() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("sv_minPing", "100");
        sv.set_cvar("sv_maxPing", "250");
        let wan = SocketAddr::from(([203, 0, 113, 7], 27960));
        // The ping is `svs.time` at the connect less at the answer: the
        // server's clock, not the packets' arrival.
        let connect_after = |sv: &mut Server, from, ms| {
            let challenge = challenge_for(sv, from, now);
            sv.core.sv_time_ms += ms as i32;
            let ui = format!(
                "\\name\\vcod\\protocol\\{}\\qport\\{QPORT}\\challenge\\{challenge}",
                PROTOCOL_V1.transport.version
            );
            sv.handle_packet(from, &build_connect(&ui), now + Duration::from_millis(ms));
            reply_text(sv)
        };
        assert_eq!(
            connect_after(&mut sv, wan, 50),
            ("error".into(), "EXE_ERR_HIGH_PING_ONLY".into())
        );
        assert_eq!(
            connect_after(&mut sv, addr(9), 50).0,
            "connectResponse",
            "a LAN client skips both tests"
        );
        sv.handle_packet(addr(20), &oob("getinfo x"), now);
        let (_, _, rest) = reply(&mut sv);
        let info = String::from_utf8_lossy(&rest).to_string();
        assert_eq!(info_value_for_key(&info, "minPing"), Some("100"));
        assert_eq!(info_value_for_key(&info, "maxPing"), Some("250"));
        let wan2 = SocketAddr::from(([203, 0, 113, 8], 27960));
        assert_eq!(
            connect_after(&mut sv, wan2, 400),
            ("error".into(), "EXE_ERR_LOW_PING_ONLY".into())
        );
        let wan3 = SocketAddr::from(([203, 0, 113, 9], 27960));
        assert_eq!(connect_after(&mut sv, wan3, 150).0, "connectResponse");
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
            sv.core
                .clients
                .iter()
                .position(|c| c.as_ref().is_some_and(|c| c.link.addr == from))
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
            .build_out(i32::from(sv.game.server_id), 0, 0, &w.into_ops(), &huff)
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
        sv.game.paks.pure = true;
        sv.game.pure_check = Some(PureCheck {
            cgame: Some(11),
            ui: Some(22),
            loaded: vec![5, 6],
            feed: sv.core.checksum_feed,
        });
        let mut nc = active(&mut sv, now);
        let cmds = cmds(sv.core.checksum_feed);
        if !cmds.is_empty() {
            let cmds: Vec<&str> = cmds.iter().map(String::as_str).collect();
            pipelined(&mut sv, &mut nc, &cmds, now);
        }
        if sv.client_count() == 1 {
            let huff = Huffman::new();
            let ack = nc.incoming_sequence as i32;
            let ops = move_ops(sv.core.checksum_feed, ack, NULL_USERCMD);
            let pkt = nc
                .build_out(i32::from(sv.game.server_id), ack, 0, &ops, &huff)
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

    /// `sv_pure` is retail's 1 on the binary's server
    /// (cod11-server-handshake.md, "Defaults and order"), 0 on the test
    /// constructor, and a set holds past the first level, as the carried
    /// table keeps it.
    #[test]
    fn sv_pure_defaults_to_retails_and_a_set_outlives_the_level() {
        let fs = vcod_common::pk3::Pk3Fs::empty();
        let now = Instant::now();
        assert_eq!(
            Server::with_seed(cfg(), now, 7).cvars(&fs).get("sv_pure"),
            "1"
        );
        let mut sv = Server::new(cfg(), now);
        assert_eq!(sv.cvars(&fs).get("sv_pure"), "0");
        sv.set_cvar("sv_pure", "1");
        let first = sv.cvars(&fs);
        assert_eq!(first.get("sv_pure"), "1");
        sv.game.carried_cvars = Some(first);
        assert_eq!(sv.cvars(&fs).get("sv_pure"), "1");
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
                i32::from(sv.game.server_id),
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
        let before = sv.core.clients[0]
            .as_ref()
            .unwrap()
            .link
            .netchan
            .reliable_sequence;
        pipelined(&mut sv, &mut nc, &["score"; 65], now);
        let c = sv.core.clients[0].as_ref().unwrap();
        assert_eq!(c.link.netchan.reliable_sequence, before + 1);
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
            let c = sv.core.clients[0].as_ref().unwrap();
            MAX_RELIABLE_COMMANDS as i32
                - (c.link.netchan.reliable_sequence as i32 - c.link.reliable_ack)
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
        let cs = sv.game.module.script.as_ref().unwrap().configstrings();
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
        assert_eq!(sv.game.limiter.addrs.len(), MAX_BUCKETS);
        // The least recently seen address went first.
        assert!(
            !sv.game
                .limiter
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
            let c = sv.core.clients[0].as_ref().unwrap();
            (c.link.netchan.incoming_sequence, c.link.addr, c.last_packet)
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
            .build_out(i32::from(sv.game.server_id) ^ 0x01, 0, 0, &eof, &huff)
            .unwrap();
        sv.handle_packet(spoofer, &pkt, later);
        assert_eq!(snapshot(&sv), before, "a stale serverId committed state");

        // Right header, ops that end inside a command.
        forged.outgoing_sequence = 0x7fff_fffe;
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_CLIENT_COMMAND, 2);
        w.write_long(1);
        let pkt = forged
            .build_out(i32::from(sv.game.server_id), 0, 0, &w.into_ops(), &huff)
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
        // A loaded level's clock: at 0 the client's slot, written at 0,
        // ties the unwritten ones and goes first to the next challenge.
        sv.start_clock_at(300);
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
            &connect_pkt(foreign, QPORT, PROTOCOL_V1.transport.version),
            t2,
        );
        assert!(sv.take_outgoing().is_empty(), "the takeover got a reply");
        let c = sv.core.clients[0].as_ref().unwrap();
        assert_eq!((c.link.addr, c.link.netchan.challenge), (addr(5), own));

        // The client's own connect retry, same challenge, still resets its slot.
        sv.handle_packet(
            addr(5),
            &connect_pkt(own, QPORT, PROTOCOL_V1.transport.version),
            t2,
        );
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
        assert_eq!(sv.client_count(), 1);

        // Once the slot has been silent for sv_reconnectlimit, a crashed client
        // coming back with a new challenge may reclaim it.
        let t3 = t2 + RECONNECT_LIMIT + Duration::from_millis(100);
        sv.handle_packet(
            addr(7),
            &connect_pkt(foreign, QPORT, PROTOCOL_V1.transport.version),
            t3,
        );
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
        assert_eq!(sv.client_count(), 1);
        let c = sv.core.clients[0].as_ref().unwrap();
        assert_eq!((c.link.addr, c.link.netchan.challenge), (addr(7), foreign));
    }

    #[test]
    fn a_challenge_never_expires_and_binds_the_port() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let old = challenge_for(&mut sv, addr(5), now);
        // An hour on, asking again gives the same slot (retail has no TTL).
        let later = now + Duration::from_secs(3600);
        assert_eq!(challenge_for(&mut sv, addr(5), later), old);
        // Another port of the same host is another address (0x8080dec).
        sv.handle_packet(
            addr(6),
            &connect_pkt(old, QPORT, PROTOCOL_V1.transport.version),
            later,
        );
        assert_eq!(
            reply_text(&mut sv),
            ("error".to_string(), "EXE_BAD_CHALLENGE".to_string())
        );
        sv.handle_packet(
            addr(5),
            &connect_pkt(old, QPORT, PROTOCOL_V1.transport.version),
            later,
        );
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
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
        active_at(sv, addr(5), now)
    }

    /// [`active`] from any address.
    fn active_at(sv: &mut Server, from: SocketAddr, now: Instant) -> Netchan {
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

    /// Off the LAN a primed client gets a bare message a second apart, and
    /// entering the world makes it due at once (0x80877d8 sets
    /// `nextSnapshotTime` to `svs.time`): live, retail answered the entering
    /// cmd 15 ms later (cod11-server-handshake.md, "Clients not in the
    /// world").
    #[test]
    fn entering_the_world_cuts_the_primed_keepalive_short() {
        let huff = Huffman::new();
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let wan = SocketAddr::from(([203, 0, 113, 9], 27960));
        let mut nc = connected(&mut sv, wan, now);
        let pkt = nc.build_out(0, 0, 0, &ack_ops(), &huff).unwrap();
        sv.handle_packet(wan, &pkt, now);
        sv.take_outgoing();
        sv.core.clients[0].as_mut().unwrap().link.unsent.clear();
        let mut bare = Vec::new();
        for i in 0..21 {
            sv.tick(now + Duration::from_millis(50 * i));
            bare.extend(sv.take_outgoing().into_iter().map(|_| i));
        }
        assert_eq!(bare, [19], "one keepalive, a second after the gamestate");
        let t = now + Duration::from_millis(1100);
        let ack = nc.incoming_sequence as i32;
        let ops = move_ops(sv.core.checksum_feed, ack, NULL_USERCMD);
        let pkt = nc
            .build_out(i32::from(sv.game.server_id), ack, 0, &ops, &huff)
            .unwrap();
        sv.handle_packet(wan, &pkt, t);
        assert_eq!(
            sv.core.clients[0].as_ref().unwrap().state,
            ClientState::Active
        );
        sv.tick(t);
        assert_eq!(
            sv.take_outgoing().len(),
            1,
            "the first snapshot goes at once"
        );
    }

    /// `SV_SendClientGameState` (0x8085eec): fragments left of an earlier
    /// message go first, all at once; the gamestate's first fragment follows
    /// and the rest go one per frame, as retail sent a loopback probe
    /// (cod11-server-handshake.md, "The gamestate goes out fragmented").
    #[test]
    fn the_gamestate_goes_out_one_fragment_per_frame() {
        let huff = Huffman::new();
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        // Enough configstrings to pass one fragment.
        for i in 0..40 {
            sv.core.configstrings[vcod_common::net::protocol::CS_SOUNDS + 1 + i] =
                format!("sound/misc/{i:03}_{}", "x".repeat(40));
        }
        let mut nc = connected(&mut sv, addr(5), now);
        let stale = vec![vec![1u8; 8], vec![2u8; 8]];
        sv.core.clients[0].as_mut().unwrap().link.unsent = stale.clone().into();
        let pkt = nc.build_out(0, 0, 0, &ack_ops(), &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        let out: Vec<Vec<u8>> = sv.take_outgoing().into_iter().map(|(_, p)| p).collect();
        assert_eq!(
            out.len(),
            3,
            "the stale fragments, then one of the gamestate"
        );
        assert_eq!(out[..2], stale[..]);
        let fragment = |p: &[u8]| u32::from_le_bytes(p[..4].try_into().unwrap()) & (1 << 31) != 0;
        assert!(fragment(&out[2]));
        let mut whole = nc.process_in(&out[2], &huff).unwrap();
        let mut frames = 0;
        while whole.is_none() {
            sv.tick(now + Duration::from_millis(50 * (frames + 1)));
            let out = sv.take_outgoing();
            assert_eq!(out.len(), 1, "one fragment a frame");
            whole = nc.process_in(&out[0].1, &huff).unwrap();
            frames += 1;
        }
        assert!(frames >= 1);
        assert!(sv.core.clients[0].as_ref().unwrap().link.unsent.is_empty());
    }

    /// `active` plus the first usercmd, which is what puts a client in the
    /// world, so slot 0 has a sim and is sent snapshots. A real client sends
    /// no command to get here.
    fn begun(sv: &mut Server, now: Instant) -> Netchan {
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
    fn latest_snapshot(
        sv: &mut Server,
        nc: &mut Netchan,
        ring: &mut SnapshotRing,
        now: Instant,
    ) -> vcod_common::net::snapshot::Snapshot {
        let p = sv.game.proto;
        latest_snapshot_as(sv, nc, ring, now, p)
    }

    /// [`latest_snapshot`] decoded through `p`.
    fn latest_snapshot_as(
        sv: &mut Server,
        nc: &mut Netchan,
        ring: &mut SnapshotRing,
        now: Instant,
        p: &Protocol,
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
            rubble: Vec::new(),
            nav_cover: Vec::new(),
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
                        sv.core.checksum_feed,
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
            rubble: Vec::new(),
            nav_cover: Vec::new(),
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
                    sv.core.checksum_feed,
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
        let ring = sv.core.clients[0]
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
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        install_script(&mut sv, rt);
        sv.game.module.weapon_table =
            Rc::new(crate::weapons::WeaponTable::load(&fs, &crate::defs::V1));
        let _nc = begun(&mut sv, now);
        sv.tick(now);
        sv.place_client(0, [0.0, 0.0, 64.0], 0.0);
        let frag = crate::configstrings::weapon_index("fraggrenade_mp").unwrap();
        let def = sv.game.module.weapon_table.get(frag).unwrap().clone();
        let mut held = crate::weapons::PlayerWeapons::default();
        held.give(frag, sv.game.module.weapon_table.slot(frag));
        held.current = frag as u8;
        let rt = sv.game.module.script.as_mut().unwrap();
        rt.host.weapons = sv.game.module.weapon_table.clone();
        rt.host.precache_items_for_test(&["fraggrenade_mp"]);
        rt.host.client_weapons[0] = held;
        let c = sv.core.clients[0].as_mut().unwrap();
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
            sv.core.clients[0]
                .as_mut()
                .unwrap()
                .pending
                .push(cmd.into());
            // The frame's clock keeps up with the cmds, which `replay_moves`
            // clamps to 200 ms past `level.time`.
            sv.core.sv_time_ms = cmd.server_time;
            sv.replay_moves();
            let rt = sv.game.module.script.as_ref().unwrap();
            let thrown = rt.missiles().entities(sv.game.proto).next().is_some();
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
            sv.core.clients[0]
                .as_mut()
                .unwrap()
                .pending
                .push(cmd.into());
            sv.tick(now);
        }
        let stood = sv.core.clients[0]
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
        let c = sv.core.clients[0].as_mut().unwrap();
        c.pending.push(release.into());
        c.pending.push(run.into());
        sv.tick(now);
        let ran = sv.core.clients[0]
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
        let rt = sv.game.module.script.as_ref().unwrap();
        let (_, e) = rt
            .missiles()
            .entities(sv.game.proto)
            .next()
            .expect("the release threw nothing");
        let traj = vcod_common::net::trajectory::Trajectory::read(&e, sv.game.proto, "pos");
        assert_eq!(traj.tr_time, sv.core.sv_time_ms - FRAME_MS);
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
            sv.core.clients[0].as_ref().unwrap().sim.as_ref().unwrap()
        }
        let mut i = 0;
        while sim(&sv).ps.grenade_time_left_ms == 0 {
            assert!(i < 60, "the pin never came out");
            sv.core.clients[0]
                .as_mut()
                .unwrap()
                .pending
                .push(cook(i).into());
            sv.replay_moves();
            i += 1;
        }
        let stood = sim(&sv).origin();
        assert!(
            stood[0].fract() != 0.0 && stood[1].fract() != 0.0,
            "the cook settled on the unit grid at {stood:?}"
        );
        let rt = sv.game.module.script.as_mut().unwrap();
        let hurt = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
        rt.register_hurt_for_test(
            hurt,
            crate::game::trigger::TriggerShape::boxed([-64.0, -64.0, -8.0], [64.0, 64.0, 64.0]),
            1000,
        );
        sv.core.clients[0]
            .as_mut()
            .unwrap()
            .pending
            .push(cook(i).into());
        sv.replay_moves();

        let snapped = glam::Vec3::from(stood).trunc();
        assert_eq!(snapped.truncate(), glam::Vec2::new(10.0, -20.0));
        let rt = sv.game.module.script.as_mut().unwrap();
        let (_, e) = rt
            .missiles()
            .entities(sv.game.proto)
            .next()
            .expect("the death dropped no grenade");
        let traj = vcod_common::net::trajectory::Trajectory::read(&e, sv.game.proto, "pos");
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
        let at = body.origin(sv.game.proto);
        assert_eq!([at[0], at[1]], [snapped.x, snapped.y]);
    }

    /// A spawn and its loadout whose weapon ops something applies ahead of
    /// the frame's spawn pass (here the move pass, after the damage
    /// callback; on UO a vehicle round's callbacks) keep the loadout: the
    /// spawn's emptying of the ammo is queued with them, in call order, and
    /// the spawn pass leaves the arrays alone (cod11-gsc-object-model.md 20,
    /// "The spawn's memset and the weapon op queue").
    #[test]
    fn a_loadout_applied_before_the_spawn_pass_survives_it() {
        let rt = crate::game::script::ScriptRuntime::for_test_at(
            crate::game::script::CALLBACK_SETUP,
            "main() {}\n\
             CodeCallback_PlayerDamage(inflictor, attacker, damage, flags, mod, weapon, point, \
             dir, hitloc) { self spawn((400, 0, 64), (0, 0, 0)); \
             self giveWeapon(\"fraggrenade_mp\"); }\n",
        );
        let Some((mut sv, st, frag)) = last_frag_in_hand_under(rt) else {
            return;
        };
        let def = sv.game.module.weapon_table.get(frag).unwrap().clone();
        let rt = sv.game.module.script.as_mut().unwrap();
        let hurt = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
        rt.register_hurt_for_test(
            hurt,
            crate::game::trigger::TriggerShape::boxed([-64.0, -64.0, -8.0], [64.0, 64.0, 64.0]),
            10,
        );
        let cmd = UserCmd {
            server_time: st + 50,
            weapon: frag as u8,
            ..NULL_USERCMD
        };
        sv.core.clients[0]
            .as_mut()
            .unwrap()
            .pending
            .push(cmd.into());
        sv.replay_moves();
        fn sim(sv: &Server) -> &ClientSim {
            sv.core.clients[0].as_ref().unwrap().sim.as_ref().unwrap()
        }
        let full = def.clip_size as i16;
        assert_eq!(sim(&sv).ps.ammoclip[def.clip_index], full, "the give");
        assert_eq!(
            sv.game
                .module
                .script
                .as_ref()
                .unwrap()
                .host
                .client_spawns
                .len(),
            1
        );
        sv.tick(Instant::now() + Duration::from_millis(50));
        assert!(
            sv.game
                .module
                .script
                .as_ref()
                .unwrap()
                .host
                .client_spawns
                .is_empty()
        );
        let at = sim(&sv).origin();
        assert!(at[0] > 300.0, "no spawn ran: {at:?}");
        assert_eq!(sim(&sv).ps.ammoclip[def.clip_index], full, "emptied");
    }

    /// The take happens once, at the cmd's touch: a frag the script gives
    /// back in the frame it was thrown stays held.
    #[test]
    fn a_frag_given_back_in_the_frame_it_was_thrown_stays_held() {
        // The clients never spawn, so they are still G_InitGentity's "noclass".
        let script = "main() { \
               precacheitem(\"fraggrenade_mp\"); \
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
            sv.core.clients[0]
                .as_mut()
                .unwrap()
                .pending
                .push(cmd.into());
            sv.tick(now);
            if sv.script_log().iter().any(|l| l.trim() == "regive") {
                assert!(
                    sv.game.module.script.as_ref().unwrap().host.client_weapons[0].holds(frag),
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
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        sv.game.module.weapon_table =
            Rc::new(crate::weapons::WeaponTable::load(&fs, &crate::defs::V1));
        let _nc = begun(&mut sv, now);
        sv.tick(now);
        sv.place_client(0, [0.0, 0.0, 1.0], 0.0);
        let index = |n| crate::configstrings::weapon_index(n).unwrap();
        let (carbine, thompson, colt) = (
            index("m1carbine_mp"),
            index("thompson_mp"),
            index("colt_mp"),
        );
        let table = sv.game.module.weapon_table.clone();
        let mut held = crate::weapons::PlayerWeapons::default();
        for (w, slot) in [(carbine, 1), (thompson, 2), (colt, 3)] {
            held.give(w, slot);
        }
        held.current = colt as u8;
        let rt = sv.game.module.script.as_mut().unwrap();
        rt.host.weapons = table.clone();
        rt.host
            .precache_items_for_test(&["m1carbine_mp", "thompson_mp", "colt_mp"]);
        rt.host.client_weapons[0] = held;
        rt.host.client_vitals[0] = crate::game::host::Vitals {
            health: 100,
            max_health: 100,
            dead: false,
            takedamage: true,
        };
        let pf = rt.place_item("mpweapon_panzerfaust", [40.0, 0.0, 50.0], 0);
        let c = sv.core.clients[0].as_mut().unwrap();
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
        sv.core.sv_time_ms = cmd.server_time;
        sv.replay_moves();
        let sim = sv.core.clients[0].as_ref().unwrap().sim.as_ref().unwrap();
        assert_eq!(sim.ps.weapon, thompson as u8, "the switch did not land");
        let rt = sv.game.module.script.as_mut().unwrap();
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
    /// `ClientEndFrame` stores `g_speed` in a 1.1 player's `ps.speed`
    /// (game.mp 0x41100): the body runs at it and the wire carries it.
    #[test]
    fn a_player_runs_at_g_speed() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 1.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        let _nc = begun(&mut sv, now);
        sv.tick(now);
        sv.place_client(0, [0.0, 0.0, 1.0], 0.0);
        let rt = sv.game.module.script.as_mut().unwrap();
        rt.set_client_state_for_test(0, "playing");
        rt.host.cvars.set("g_speed", "250.9");
        sv.tick(now + Duration::from_millis(50));
        let c = sv.core.clients[0].as_mut().unwrap();
        assert_eq!(c.sim.as_ref().unwrap().ps.speed, 250.0);
        let st = c.last_processed_st;
        for i in 1..=20 {
            let cmd = UserCmd {
                server_time: st + 50 * i,
                forward: 127,
                ..NULL_USERCMD
            };
            c.pending.push(cmd.into());
        }
        sv.replay_moves();
        let sim = sv.core.clients[0].as_ref().unwrap().sim.as_ref().unwrap();
        let h = sim.ps.velocity.truncate().length();
        assert!((h - 250.0).abs() < 1.0, "{h}");
        let w = sim.to_wire(&PROTOCOL_V1, 0, 0);
        assert_eq!(w.field_i32(&PROTOCOL_V1, "speed"), 250);
    }

    /// `ClientEndFrame` stores `(int)g_gravity` in a player's `ps.gravity`
    /// (game.mp 0x410aa): a jump climbs as high and stays up `sqrt 2` times
    /// as long at 400, and the wire carries it (cod15-movement.md,
    /// "ps.gravity").
    #[test]
    fn a_player_jumps_at_g_gravity() {
        let airtime = |gravity: &str| {
            let now = Instant::now();
            let mut sv = Server::new(cfg(), now);
            sv.load_world(World {
                collision: test_world(&[]),
                vis: vcod_common::bsp::Visibility::none(),
                spawn: ([0.0, 0.0, 1.0], 0.0),
                spawn_points: Vec::new(),
                hazards: Vec::new(),
                rubble: Vec::new(),
                nav_cover: Vec::new(),
            });
            install_script(
                &mut sv,
                crate::game::script::ScriptRuntime::for_test("main() {}"),
            );
            let _nc = begun(&mut sv, now);
            sv.tick(now);
            sv.place_client(0, [0.0, 0.0, 0.125], 0.0);
            let rt = sv.game.module.script.as_mut().unwrap();
            rt.set_client_state_for_test(0, "playing");
            rt.host.cvars.set("g_gravity", gravity);
            sv.tick(now + Duration::from_millis(50));
            let st = sv.core.clients[0].as_ref().unwrap().last_processed_st;
            let mut top = 0.0f32;
            for i in 1..=60 {
                sv.core.clients[0].as_mut().unwrap().pending.push(
                    UserCmd {
                        server_time: st + 50 * i,
                        up: if i == 1 { 127 } else { 0 },
                        ..NULL_USERCMD
                    }
                    .into(),
                );
                sv.tick(now + Duration::from_millis(50 + 50 * i as u64));
                let sim = sv.core.clients[0].as_ref().unwrap().sim.as_ref().unwrap();
                top = top.max(sim.ps.origin.z);
                if i > 1 && sim.ps.on_ground {
                    let w = sim.to_wire(&PROTOCOL_V1, 0, 0);
                    return (w.field_i32(&PROTOCOL_V1, "gravity"), top, 50 * i);
                }
            }
            panic!("no landing at g_gravity {gravity}");
        };
        let (stock, light) = (airtime("800"), airtime("400.7"));
        assert_eq!((stock.0, light.0), (800, 400));
        assert!((light.1 - stock.1).abs() < 1.5, "{light:?} vs {stock:?}");
        let ratio = light.2 as f32 / stock.2 as f32;
        assert!((ratio - 2f32.sqrt()).abs() < 0.1, "{light:?} vs {stock:?}");
    }

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
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        sv.game.module.weapon_table =
            Rc::new(crate::weapons::WeaponTable::load(&fs, &crate::defs::V1));
        let _nc = begun(&mut sv, now);
        sv.tick(now);
        sv.place_client(0, [0.0, 0.0, 1.0], 0.0);
        let carbine = crate::configstrings::weapon_index("m1carbine_mp").unwrap();
        let def = sv.game.module.weapon_table.get(carbine).unwrap().clone();
        let mut held = crate::weapons::PlayerWeapons::default();
        held.give(carbine, sv.game.module.weapon_table.slot(carbine));
        held.current = carbine as u8;
        let rt = sv.game.module.script.as_mut().unwrap();
        rt.host.weapons = sv.game.module.weapon_table.clone();
        rt.host.precache_items_for_test(&["fraggrenade_mp"]);
        rt.host.client_weapons[0] = held;
        rt.host.client_vitals[0] = crate::game::host::Vitals {
            health: 100,
            max_health: 100,
            dead: false,
            takedamage: true,
        };
        rt.place_item("mpweapon_m1carbine", [0.0, 0.0, 1.0], 20);
        let c = sv.core.clients[0].as_mut().unwrap();
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
        let sim = sv.core.clients[0].as_ref().unwrap().sim.as_ref().unwrap();
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
                i32::from(sv.game.server_id),
                ack,
                0,
                &move_ops(sv.core.checksum_feed, ack, cmd),
                &Huffman::new(),
            )
            .unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        sv.tick(now + Duration::from_millis(50));
        let rt = sv.game.module.script.as_mut().unwrap();
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
            rubble: Vec::new(),
            nav_cover: Vec::new(),
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
                &move_ops(sv.core.checksum_feed, ack, entering),
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
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let p = &PROTOCOL_V1;
        latest_snapshot(&mut sv, &mut nc, &mut ring, now);

        // Two messages, so the base has to chain across them the way the
        // client's does; `begun` entered on a null cmd, which is where a
        // fresh chain starts.
        let mut chain = MoveChain::new(sv.core.checksum_feed);
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
            rubble: Vec::new(),
            nav_cover: Vec::new(),
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
                &move_ops_cmds(sv.core.checksum_feed, ack, &cmds),
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
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let mut chain = MoveChain::new(sv.core.checksum_feed);
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
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let mut chain = MoveChain::new(sv.core.checksum_feed);
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
            sv.core.clients[0].as_ref().unwrap().pending.len() <= 2,
            "flood left {} cmds queued",
            sv.core.clients[0].as_ref().unwrap().pending.len()
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
            rubble: Vec::new(),
            nav_cover: Vec::new(),
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
                    sv.core.checksum_feed,
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
                &move_ops_omit_angles(sv.core.checksum_feed, ack, 520, 20),
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

    /// A protocol 6 server moves its client by 1.5's rules and says so on
    /// the wire: a jump arms the penalty, `pm_flags` 0x2000 with its
    /// `pm_time` (cod15-movement.md, "The jump penalty"). A 1.1 server's
    /// jump carries neither.
    #[test]
    fn protocol_22_sprints_and_tires_on_the_wire() {
        use vcod_common::net::flags::PMF_SPRINT_V22;
        use vcod_common::net::msg::BUTTON_SPRINT_V22;
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_protocol(&PROTOCOL_V22);
        sv.load_world(World {
            collision: test_world(&[(
                glam::Vec3::new(-4000.0, -4000.0, -64.0),
                glam::Vec3::new(4000.0, 4000.0, 0.0),
            )]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 0.125], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        let mut nc = begun(&mut sv, now);
        let sim = sv.core.clients[0].as_mut().unwrap().sim.as_mut().unwrap();
        sim.become_player([0.0, 0.0, 0.125], 0.0, [0; 3]);
        let mut ring = SnapshotRing::new();
        let mut chain = MoveChain::new(sv.core.checksum_feed);
        let mut t = now;
        let mut seen = Vec::new();
        for i in 0..40 {
            let st = 100 + i * 50;
            let ack = nc.incoming_sequence as i32;
            let cmd = UserCmd {
                server_time: st,
                forward: 127,
                buttons: BUTTON_SPRINT_V22,
                ..Default::default()
            };
            let ops = chain.ops(ack, &[cmd]);
            let pkt = nc.build_out(0x10, ack, 0, &ops, &Huffman::new()).unwrap();
            sv.handle_packet(addr(5), &pkt, t);
            t += std::time::Duration::from_millis(50);
            sv.core.sv_time_ms = st;
            let s = latest_snapshot_as(&mut sv, &mut nc, &mut ring, t, &PROTOCOL_V22);
            let ps = &s.ps;
            seen.push((
                ps.field_i32(&PROTOCOL_V22, "pm_flags") & PMF_SPRINT_V22 != 0,
                ps.field_f32(&PROTOCOL_V22, "fatigueScale"),
                ps.field_i32(&PROTOCOL_V22, "lastSprintTime"),
                ps.field_f32(&PROTOCOL_V22, "sprintSpeedScale"),
            ));
        }
        let last = seen.last().unwrap();
        assert!(seen.iter().any(|s| s.0), "never sprinting: {seen:?}");
        assert!(last.1 < 1.0 && last.2 > 0, "{last:?}");
        assert_eq!(last.3, 1.6);
    }

    #[test]
    fn protocol_6_jumps_carry_the_15_penalty() {
        for (p, decode) in [(&PROTOCOL_V6, &RAW_V6), (&PROTOCOL_V1, &PROTOCOL_V1)] {
            let now = Instant::now();
            let mut sv = Server::new(cfg(), now);
            sv.set_protocol(p);
            sv.load_world(World {
                collision: test_world(&[(
                    glam::Vec3::new(-2000.0, -2000.0, -64.0),
                    glam::Vec3::new(2000.0, 2000.0, 0.0),
                )]),
                vis: vcod_common::bsp::Visibility::none(),
                spawn: ([0.0, 0.0, 0.125], 0.0),
                spawn_points: Vec::new(),
                hazards: Vec::new(),
                rubble: Vec::new(),
                nav_cover: Vec::new(),
            });
            let mut nc = begun(&mut sv, now);
            let sim = sv.core.clients[0].as_mut().unwrap().sim.as_mut().unwrap();
            sim.become_player([0.0, 0.0, 0.125], 0.0, [0; 3]);
            let mut ring = SnapshotRing::new();
            let mut chain = MoveChain::new(sv.core.checksum_feed);
            let mut t = now;
            let mut penalty = Vec::new();
            for i in 0..30 {
                let st = 100 + i * 50;
                let up = if i == 10 { 127 } else { 0 };
                let ack = nc.incoming_sequence as i32;
                let cmd = UserCmd {
                    server_time: st,
                    up,
                    ..Default::default()
                };
                let ops = chain.ops(ack, &[cmd]);
                let pkt = nc.build_out(0x10, ack, 0, &ops, &Huffman::new()).unwrap();
                sv.handle_packet(addr(5), &pkt, t);
                t += std::time::Duration::from_millis(50);
                sv.core.sv_time_ms = st;
                let s = latest_snapshot_as(&mut sv, &mut nc, &mut ring, t, decode);
                let flags = s.ps.field_i32(decode, "pm_flags");
                penalty.push((flags & 0x2000 != 0, s.ps.field_i32(decode, "pm_time")));
            }
            let armed = penalty.iter().filter(|(on, time)| *on && *time > 0).count();
            if p.transport.version == PROTOCOL_V6.transport.version {
                assert!(armed > 0, "{penalty:?}");
                assert!(!penalty[5].0, "no penalty before the jump");
            } else {
                assert_eq!(armed, 0, "{penalty:?}");
            }
        }
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
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let mut chain = MoveChain::new(sv.core.checksum_feed);
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
        // back to rest. Spectator friction drops speed * 5 * frametime, a
        // geometric decay with no stop-speed floor, over 32 frames (the wire
        // per-message cap).
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
        sv.core.sv_time_ms = 1900;
        let s2 = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            t1 + std::time::Duration::from_millis(50),
        );
        let vz = s2.ps.field_f32(p, "velocity[2]");
        // `PM_Friction` stops at speed 1 and zeroes only xy, so the climb
        // rests just under 1 u/s, as retail's idle spectator read 0.9565565
        // (docs/protocol-1.1.md, "Spectator").
        assert!(
            vz > 0.0 && vz < 1.0,
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
            rubble: Vec::new(),
            nav_cover: Vec::new(),
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
                    sv.core.checksum_feed,
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
                &move_ops_omit_angles(sv.core.checksum_feed, ack, 520, 20),
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
    fn a_client_before_its_gamestate_gets_bare_messages() {
        // `SV_SendClientMessages` (0x809045c): a `CS_CONNECTED` client gets
        // the header and `svc_EOF`, 9 bytes, every frame on a LAN and a
        // second apart off it (0x808f817).
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let wan = SocketAddr::from(([203, 0, 113, 9], 27960));
        let mut lan_nc = connected(&mut sv, addr(5), now);
        let mut wan_nc = connected(&mut sv, wan, now);
        let huff = Huffman::new();
        let (mut lan_n, mut wan_n) = (0, 0);
        for i in 0..22 {
            sv.tick(now + Duration::from_millis(50 * i));
            for (to, pkt) in sv.take_outgoing() {
                assert_eq!(pkt.len(), 9, "a bare message");
                let nc = if to == wan { &mut wan_nc } else { &mut lan_nc };
                let msg = nc.process_in(&pkt, &huff).unwrap().unwrap();
                let mut r = MsgReader::new(&msg[4..], &huff);
                assert_eq!(r.read_byte(), msg::SVC_EOF, "no commands, no snapshot");
                *if to == wan { &mut wan_n } else { &mut lan_n } += 1;
            }
        }
        assert_eq!(lan_n, 22);
        assert_eq!(wan_n, 2, "a keepalive at 0 and at 1000 ms");
    }

    /// `SV_SendClientMessages` (0x809045c) is one pass in slot order
    /// whatever each slot's state: a client not in the world at slot 0,
    /// or a zombie there, is sent its message before slot 1's snapshot
    /// (cod11-server-handshake.md, "The order of a frame's messages").
    #[test]
    fn a_frames_messages_go_out_in_slot_order() {
        let t = Instant::now();
        let low = addr(5);
        // Another host: the netchan finds a client by ip and qport.
        let high = SocketAddr::from(([127, 0, 0, 2], 5));
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "secret");
        connected(&mut sv, low, t);
        active_at(&mut sv, high, t);
        sv.take_outgoing();
        let order = |sv: &mut Server, at| {
            sv.tick(at);
            let out = sv.take_outgoing();
            out.iter().map(|(to, p)| (*to, p.len())).collect::<Vec<_>>()
        };
        let sent = order(&mut sv, t);
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[0], (low, 9), "slot 0's bare message first: {sent:?}");
        assert_eq!(sent[1].0, high);

        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "secret");
        active_at(&mut sv, low, t);
        active_at(&mut sv, high, t);
        sv.take_outgoing();
        rcon(&mut sv, "secret clientkick 0", t);
        let sent = order(&mut sv, t);
        assert!(sv.game.zombies[0].is_some());
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[0].0, low, "the zombie's message first: {sent:?}");
        assert_eq!(sent[1].0, high);
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
        /// What the rig's client decodes its frames with.
        decode: &'static Protocol,
    }

    /// Protocol 6 with `pm_flags` left in 1.5's layout, for asserting the
    /// wire bits.
    static RAW_V6: Protocol = Protocol {
        pm_flags_to_v1: |f| f,
        ..PROTOCOL_V6
    };

    const FOLLOW_P1: [f32; 3] = [200.0, 0.0, 0.0];
    const FOLLOW_P2: [f32; 3] = [-300.0, 100.0, 0.0];

    impl FollowRig {
        fn new() -> Self {
            Self::with_script(crate::game::script::ScriptRuntime::for_test("main() {}"))
        }

        fn with_script(rt: crate::game::script::ScriptRuntime) -> Self {
            Self::with_protocol(rt, &PROTOCOL_V1, &PROTOCOL_V1)
        }

        /// A server speaking `p`, its frames decoded through `decode`.
        fn with_protocol(
            rt: crate::game::script::ScriptRuntime,
            p: &'static Protocol,
            decode: &'static Protocol,
        ) -> Self {
            let now = Instant::now();
            let mut sv = Server::new(cfg(), now);
            sv.set_protocol(p);
            sv.load_world(World {
                collision: test_world(&[]),
                vis: vcod_common::bsp::Visibility::single_cluster(),
                spawn: ([0.0, 0.0, 64.0], 0.0),
                spawn_points: Vec::new(),
                hazards: Vec::new(),
                rubble: Vec::new(),
                nav_cover: Vec::new(),
            });
            install_script(&mut sv, rt);
            let nc = begun(&mut sv, now);
            let chain = MoveChain::new(sv.core.checksum_feed);
            let mut rig = FollowRig {
                sv,
                nc,
                ring: SnapshotRing::new(),
                chain,
                now,
                st: 0,
                decode,
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
            self.sv.game.module.script.as_mut().unwrap()
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
            self.sv.core.clients[slot] = Some(c);
            self.script().push_client_event(ClientEvent::Connect {
                slot,
                name: format!("p{slot}"),
            });
        }

        fn sim(&self, slot: usize) -> &ClientSim {
            self.sv.core.clients[slot]
                .as_ref()
                .unwrap()
                .sim
                .as_ref()
                .unwrap()
        }

        fn sim_mut(&mut self, slot: usize) -> &mut ClientSim {
            self.sv.core.clients[slot]
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
                .build_out(
                    i32::from(self.sv.game.server_id),
                    ack,
                    0,
                    &ops,
                    &Huffman::new(),
                )
                .unwrap();
            self.sv.handle_packet(addr(5), &pkt, self.now);
            latest_snapshot_as(
                &mut self.sv,
                &mut self.nc,
                &mut self.ring,
                self.now,
                self.decode,
            )
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

    fn v6_rig() -> FollowRig {
        FollowRig::with_protocol(
            crate::game::script::ScriptRuntime::for_test("main() {}"),
            &PROTOCOL_V6,
            &RAW_V6,
        )
    }

    /// 1.5's spectator bits on the wire: a free spectator may cycle
    /// (0x40000, as every spectator frame of the 1.5b capture reads), a free
    /// follow may cycle and fly (0x40000 | 0x80000), and a playing client's
    /// own view is 0x20000.
    #[test]
    fn protocol_6_sends_15s_spectator_bits() {
        let mut rig = v6_rig();
        let free = rig.step(0);
        assert_eq!(ps_i32(&free, "pm_flags") & 0x1f0000, 0x40000);
        let s = rig.press(msg::BUTTON_ATTACK);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(
            ps_i32(&s, "pm_flags") & 0x1f0000,
            0x10000 | 0x40000 | 0x80000
        );
        assert_eq!(ps_i32(&s, "speed"), 190);
    }

    /// Free flight refused (`allowSpectateTeam("freelook", false)`): a
    /// spectator following nobody is cycled onto a player, the sight edge
    /// no longer lets go, and the follow loses 0x80000.
    #[test]
    fn protocol_6_without_freelook_the_spectator_follows() {
        let mut rig = v6_rig();
        rig.script().host.client_no_spectate[0] = 0x10;
        let s = rig.step(0);
        let s = if ps_i32(&s, "clientNum") == 0 {
            rig.step(0)
        } else {
            s
        };
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x1f0000, 0x10000 | 0x40000);
        let s = rig.press(msg::BUTTON_ADS);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
    }

    /// A team the mask refuses is skipped by the cycle; with every team
    /// refused the spectator stays free and may not cycle.
    #[test]
    fn protocol_6_refused_teams_are_not_followed() {
        let mut rig = v6_rig();
        rig.script().set_client_team_for_test(1, "axis");
        rig.script().set_client_team_for_test(2, "allies");
        rig.script().host.client_no_spectate[0] = 0x2;
        let s = rig.press(msg::BUTTON_ATTACK);
        assert_eq!(ps_i32(&s, "clientNum"), 2, "slot 1 is axis");
        let s = rig.press(msg::BUTTON_ATTACK);
        assert_eq!(ps_i32(&s, "clientNum"), 2);
        rig.script().host.client_no_spectate[0] = 0x7;
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x1f0000, 0);
    }

    /// 1.5b's match timeout (cod15-protocol-diff.md, section 12): the
    /// game clock and every snapshot's `serverTime` stand from the frame
    /// after the call while snapshots keep coming, until the recovery runs
    /// out on `svs.time`; then the clock moves on from where it stood.
    #[test]
    fn protocol_6_match_timeout_holds_the_game_clock() {
        let mut rig = v6_rig();
        {
            let host = &mut rig.script().host;
            host.patch_1_5 = true;
            host.cvars.set("g_timeoutsAllowed", "1");
            host.cvars.set("g_timeoutLength", "300");
            host.cvars.set("g_timeoutRecovery", "200");
            host.timeout = crate::game::timeout::MatchTimeout::new(180_000);
        }
        rig.script().set_client_team_for_test(1, "axis");
        let called_at = rig.step(0).server_time;
        let seq = rig.sv.core.clients[1]
            .as_ref()
            .unwrap()
            .link
            .last_client_command
            + 1;
        assert!(rig.sv.client_command(1, seq, "matchtimeout".into()));
        let cmds = rig.commands();
        // The rig's own unacked `d 0` and `d 1` ride along; the timeout's
        // are the rest, in send order.
        let texts: Vec<&str> = cmds
            .iter()
            .map(|c| c.split_once(':').unwrap().1)
            .filter(|t| !t.starts_with("d 0 ") && !t.starts_with("d 1 "))
            .collect();
        assert_eq!(
            texts,
            [
                "d 1628 300",
                "d 1629 PATCH_1_5_TIMEOUT_CALLED\u{14}GAME_AXIS\u{15}",
                "e \"PATCH_1_5_TIMEOUT_CALLED_PLAYERNAME\u{15}p1\"",
            ]
        );
        // `commands` ran the first frozen frame; 300 ms of timeout and 200
        // of recovery later the clock moves again.
        let mut frozen = 1;
        loop {
            let s = rig.step(0);
            if s.server_time != called_at {
                assert_eq!(s.server_time, called_at + FRAME_MS);
                break;
            }
            frozen += 1;
            assert!(frozen < 40, "the clock never moved again");
        }
        assert_eq!(
            frozen, 11,
            "300 + 200 ms on svs.time, and the frame past it"
        );
        assert_eq!(rig.sv.configstring(1628), "");
        assert_eq!(rig.sv.game.paused_ms, frozen * FRAME_MS);
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
            quake: None,
            origin2: None,
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

    /// 1.5's and UO's `ClientDisconnect` stops a follower outright, with
    /// another client still there to cycle to (1.5b 0x4cf5f, UO 0x55433).
    #[test]
    fn a_dropped_client_stops_its_followers_on_1_5() {
        let rt = crate::game::script::ScriptRuntime::for_test("main() {}");
        let mut rig = FollowRig::with_protocol(rt, &PROTOCOL_V6, &PROTOCOL_V6);
        assert_eq!(ps_i32(&rig.press(msg::BUTTON_ATTACK), "clientNum"), 1);
        rig.sv.drop_client(1, "EXE_DISCONNECTED");
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
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
            .game
            .archive
            .frame(rig.sv.game.archive.lookup(&mut 1000).unwrap())
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
        // What a stock killed callback writes; the dead arm keys on it.
        rig.script().set_client_state_for_test(1, "dead");
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
                dead_yaw: 0,
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

    /// An rcon line is tokenized whole, comments dropped, rebuilt and
    /// tokenized again; the cvar reads are what retail 1.1d, 1.5b and UO
    /// 1.51b printed (cod11-server-handshake.md, "rcon").
    #[test]
    fn an_rcon_line_loses_its_comments_like_retail() {
        let mut t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        sv.set_cvar("rconPassword", "pw");
        let mut ask = |sv: &mut Server, line: &str| {
            t += Duration::from_millis(500);
            rcon(sv, &format!("pw {line}"), t).concat()
        };
        for (set, value) in [
            ("\"a//b\"", "a"),
            ("\"a b//c\"", "a b//c"),
            ("a/*/b", "a b"),
        ] {
            ask(&mut sv, &format!("set vc_t {set}"));
            assert_eq!(
                ask(&mut sv, "vc_t"),
                format!("print\n\"vc_t\" is:\"{value}^7\" default:\"a^7\"\n"),
                "{set}"
            );
        }
        ask(&mut sv, "sv_hostname a;b\\c");
        sv.handle_packet(addr(10), &oob("getstatus"), t);
        let (_, _, rest) = reply(&mut sv);
        let text = String::from_utf8_lossy(&rest);
        assert!(text.contains("\\sv_hostname\\abc\\"), "{text}");
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

    /// Two requests 510 ms apart read in one tick: retail handles each as it
    /// arrives, so both are answered; the window reads the handling moment,
    /// not the tick's (cod11-server-handshake.md, "rcon").
    #[test]
    fn rcon_s_window_reads_each_request_s_own_moment() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconPassword", "pw");
        let tick = t + Duration::from_millis(560);
        let mut answered = 0;
        for at in [t, t + Duration::from_millis(510)] {
            let pkt = oob("rcon pw bogus_cmd");
            let read = crate::engine::Arrival {
                now: tick,
                arrived: at,
                read: at,
            };
            sv.packet_read(addr(9), &pkt, read);
            answered += sv.take_outgoing().len();
        }
        assert_eq!(answered, 2);
    }

    /// `clientkick` drops through `SV_DropClient`: the slot is a zombie that
    /// resends the notice every frame until it is acked, shows as `ZMBI`,
    /// and is freed `sv_zombietime` after the kick, not at it.
    /// `SV_CheckTimeouts` (0x808cbc0): after the clock advance, a last
    /// packet more than `sv_timeout` seconds old counts a frame, a fresh one
    /// resets the count, and the sixth stale frame in a row drops the client
    /// with no zombie. A last-packet time ahead of the clock is pulled back.
    #[test]
    fn a_silent_client_is_dropped_on_its_sixth_stale_frame() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("sv_timeout", "1");
        let _nc = active(&mut sv, t);
        sv.take_outgoing();
        let last = sv.core.sv_time_ms;
        sv.core.clients[0].as_mut().unwrap().link.last_packet_ms = last + 400;
        sv.tick(t);
        assert_eq!(
            sv.core.clients[0].as_ref().unwrap().link.last_packet_ms,
            last + 50
        );
        let stale_at = |sv: &Server| {
            sv.core.sv_time_ms - sv.core.clients[0].as_ref().unwrap().link.last_packet_ms
        };
        while stale_at(&sv) <= 1000 {
            sv.tick(t);
        }
        assert_eq!(sv.core.clients[0].as_ref().unwrap().link.timeout_count, 1);
        // From a fresh count, the sixth stale frame drops.
        sv.core.clients[0].as_mut().unwrap().link.last_packet_ms = sv.core.sv_time_ms - 1001;
        sv.core.clients[0].as_mut().unwrap().link.timeout_count = 0;
        for _ in 0..5 {
            sv.tick(t);
            assert_eq!(sv.client_count(), 1);
        }
        sv.tick(t);
        assert_eq!(sv.client_count(), 0);
        assert!(sv.game.zombies[0].is_none());
    }

    /// `SV_DropClient` runs the game's `ClientDisconnect` inside the drop, so
    /// a timeout after the frame still puts the callback's line in that
    /// frame's message: after the drop notice on 1.1, ahead of it on 1.5 and
    /// UO (`client-probes/probe_dropclient`, handshake doc "When the game
    /// hears a drop").
    #[test]
    fn a_timed_out_clients_callback_lines_go_out_with_its_drop_notice() {
        use crate::game::script::{CALLBACK_SETUP, ScriptRuntime};
        for (proto, gone_first) in [(&PROTOCOL_V1, false), (&PROTOCOL_V6, true)] {
            let t = Instant::now();
            let mut sv = Server::new(cfg(), t);
            sv.set_protocol(proto);
            install_script(
                &mut sv,
                ScriptRuntime::for_test_at(
                    CALLBACK_SETUP,
                    "main() {}\n\
                     CodeCallback_PlayerDisconnect() { iprintln(\"gone\"); }\n",
                ),
            );
            sv.set_cvar("sv_timeout", "1");
            let mut watcher = begun(&mut sv, t);
            sv.core.clients[1] = Some(Client::new(addr(6), 0x3001, 0, "\\name\\quiet".into(), t));
            sv.game
                .module
                .script
                .as_mut()
                .unwrap()
                .push_client_event(ClientEvent::Connect {
                    slot: 1,
                    name: "quiet".into(),
                });
            sv.tick(t);
            sv.take_outgoing();
            let huff = Huffman::new();
            sv.core.clients[1].as_mut().unwrap().link.last_packet_ms = sv.core.sv_time_ms - 1001;
            sv.core.clients[1].as_mut().unwrap().link.timeout_count = 5;
            sv.core.clients[0].as_mut().unwrap().link.last_packet_ms = sv.core.sv_time_ms;
            sv.tick(t);
            assert_eq!(sv.client_count(), 1);
            let out = sv.take_outgoing();
            let (_, pkt) = out.iter().find(|(to, _)| *to == addr(5)).unwrap();
            let cmds = server_commands(&mut watcher, pkt, &huff);
            let notice = "e \"\u{15}quiet^7 \u{14}EXE_TIMEDOUT\"".to_string();
            let gone = "f \"gone\"".to_string();
            let want = if gone_first {
                [gone, notice]
            } else {
                [notice, gone]
            };
            assert!(
                cmds.ends_with(&want),
                "{}: {cmds:?}",
                proto.transport.version
            );
        }
    }

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
        let seq = sv.game.zombies[0]
            .as_ref()
            .unwrap()
            .link
            .netchan
            .reliable_sequence as i32;
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
        // Its packets do not keep it alive. The kick stamped the clock
        // between frames; the slot goes on the first frame more than
        // `sv_zombietime` past it, before that frame's message. Three
        // frames have run since the kick.
        for frame in 4..=40 {
            sv.tick(t1);
            assert!(sv.game.zombies[0].is_some(), "frame {frame}");
            sv.take_outgoing();
        }
        sv.tick(t1);
        assert!(sv.game.zombies[0].is_none());
        assert!(sv.take_outgoing().is_empty());
    }

    /// Off the LAN a zombie is paced as a client out of the world: a
    /// second between messages. Live, retail sent a disconnected probe's
    /// zombie a message 1000 ms apart where vcod had sent one a frame
    /// (cod11-server-handshake.md, "Zombie slots").
    #[test]
    fn an_off_lan_zombie_gets_a_message_a_second() {
        let huff = Huffman::new();
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "secret");
        let wan = SocketAddr::from(([203, 0, 113, 9], 27960));
        let mut nc = active_at(&mut sv, wan, t);
        let ack = nc.incoming_sequence as i32;
        let ops = move_ops(sv.core.checksum_feed, ack, NULL_USERCMD);
        let pkt = nc
            .build_out(i32::from(sv.game.server_id), ack, 0, &ops, &huff)
            .unwrap();
        sv.handle_packet(wan, &pkt, t);
        assert_eq!(
            sv.core.clients[0].as_ref().unwrap().state,
            ClientState::Active
        );
        sv.tick(t);
        sv.take_outgoing();
        let kick = t + Duration::from_millis(1000);
        rcon(&mut sv, "secret clientkick 0", kick);
        let mut sent = Vec::new();
        for i in 0..36 {
            sv.tick(kick + Duration::from_millis(50 * i));
            sent.extend(sv.take_outgoing().into_iter().map(|_| 50 * i));
        }
        assert_eq!(sent, [0, 1000], "a zombie's messages off the LAN");
    }

    /// A zombie's snapshot is the ring slot its sequence lands on, written
    /// whole: the playerstate of 32 messages back, no entities and no
    /// clients (handshake doc, "Zombie slots").
    #[test]
    fn a_zombie_gets_the_stale_ring_slot_uncompressed() {
        let p = &PROTOCOL_V1;
        let huff = Huffman::new();
        let mut c = Client::new(addr(5), 1, 7, String::new(), Instant::now());
        let commandtime = |n: i32| {
            let mut ps = msg::PlayerState::null(p);
            ps.fields[msg::PlayerState::field_index(p, "commandTime").unwrap()] = n;
            ps
        };
        for n in 1..=40u32 {
            let mut e = msg::EntityState::null(p);
            e.number = 100;
            c.record_frame(snapshot::Snapshot {
                server_time: 50 * n as i32,
                message_num: n,
                delta_num: -1,
                snap_flags: 0,
                ps: commandtime(n as i32),
                entities: [(100, e)].into(),
                clients: [(0, msg::ClientState::null(p))].into(),
                valid: true,
            });
        }
        c.link.netchan.outgoing_sequence = 41;
        let pkts = zombie_message(&mut c, 2050, 4, p, &huff);
        let mut nc = Netchan::new(1, 7);
        nc.incoming_sequence = 40;
        let msgbytes = nc.process_in(&pkts[0], &huff).unwrap().unwrap();
        let mut r = MsgReader::new(&msgbytes[4..], &huff);
        assert_eq!(r.read_byte(), SVC_SNAPSHOT);
        let mut ring = SnapshotRing::new();
        let z = ring.parse_into(&mut r, p, 41).unwrap();
        assert_eq!(z.delta_num, -1, "written whole");
        assert!(z.entities.is_empty() && z.clients.is_empty());
        assert_eq!(z.ps, commandtime(41 - 32));
    }

    /// `SNAPFLAG_NOT_ACTIVE` rides a zombie's snapshot only when the client
    /// was never sent one as an active client (0x808dfc3..0x808dfe3).
    #[test]
    fn a_zombie_never_active_carries_snapflag_not_active() {
        let p = &PROTOCOL_V1;
        let huff = Huffman::new();
        let zombie_flags = |entered: bool| {
            let t = Instant::now();
            let mut sv = Server::new(cfg(), t);
            let mut nc = if entered {
                begun(&mut sv, t)
            } else {
                active(&mut sv, t)
            };
            sv.tick(t);
            for (_, pkt) in sv.take_outgoing() {
                let _ = nc.process_in(&pkt, &huff);
            }
            sv.drop_client(0, "EXE_DISCONNECTED");
            sv.tick(t);
            let out = sv.take_outgoing();
            let msg = nc.process_in(&out[0].1, &huff).unwrap().unwrap();
            let mut r = MsgReader::new(&msg[4..], &huff);
            loop {
                match r.read_byte() {
                    msg::SVC_SERVER_COMMAND => {
                        r.read_long();
                        r.read_big_string();
                    }
                    SVC_SNAPSHOT => break,
                    b => panic!("unexpected {b}"),
                }
            }
            let mut ring = SnapshotRing::new();
            ring.parse_into(&mut r, p, nc.incoming_sequence)
                .unwrap()
                .snap_flags
        };
        assert_eq!(
            zombie_flags(false) & SNAPFLAG_NOT_ACTIVE,
            SNAPFLAG_NOT_ACTIVE
        );
        assert_eq!(zombie_flags(true) & SNAPFLAG_NOT_ACTIVE, 0);
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
        assert!(sv.core.clients[0].is_none() && sv.core.clients[1].is_some());
        let t2 = t + RECONNECT_LIMIT + Duration::from_millis(100);
        let _ = connected(&mut sv, addr(5), t2);
        assert!(sv.core.clients[0].is_some() && sv.game.zombies[0].is_none());
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
        assert!(sv.game.zombies[0].is_some());
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
        if let Some(rt) = sv.game.module.script.as_mut() {
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

    /// A stdin line reaches the game's cvars and `quit` ends the server,
    /// run by the console as an rcon line is (cod11-server-handshake.md,
    /// "The stdin console").
    #[test]
    fn a_stdin_line_runs_on_the_console() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        crate::stdin_console::StdinConsole::read_now(b"set scr_dm_timelimit 5\nquit\n")
            .queue(&mut sv);
        sv.drain_console(t);
        assert_eq!(sv.script_cvar("scr_dm_timelimit").as_deref(), Some("5"));
        assert!(sv.quit_requested());
    }

    /// The tty console's tab asks the frame loop for its names: the
    /// version's commands, then the cvars.
    #[test]
    fn a_tab_gets_the_commands_and_cvars() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        sv.set_cvar("scr_dm_timelimit", "5");
        let names = crate::stdin_console::StdinConsole::names_now(&mut sv);
        assert_eq!(names[0], "map");
        assert!(names.iter().any(|n| n == "killserver"));
        assert!(
            !names.iter().any(|n| n == "tempBanUser"),
            "1.1 has no tempBanUser"
        );
        assert!(names.iter().any(|n| n == "scr_dm_timelimit"));
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
        // A LAN client has no guid: nothing is kept and nothing refused.
        assert_eq!(sv.core.bans.unban("vcod"), None);
        sv.handle_packet(addr(6), &oob("getchallenge"), at);
        assert_eq!(reply(&mut sv).1, "challengeResponse");

        let wan = SocketAddr::from(([203, 0, 113, 7], 28960));
        sv.core
            .bans
            .append(guid_of(crate::engine::Lan::Cod1, wan), "");
        sv.handle_packet(wan, &oob("getchallenge"), at);
        let (to, cmd, rest) = reply(&mut sv);
        assert_eq!(
            (to, cmd.as_str(), rest.as_slice()),
            (wan, "error", &b"EXE_ERR_BAD_CDKEY"[..])
        );
    }

    /// 1.5's temporary ban (handshake doc, "Bans"): refused with retail's
    /// own error while the entry is no older than `sv_kickBanTime`, read at
    /// the check, so a longer time revives an expired ban.
    #[test]
    fn a_temporary_ban_lasts_sv_kickbantime_read_at_the_check() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        let ip = std::net::Ipv4Addr::new(203, 0, 113, 7);
        let wan = SocketAddr::from((ip, 28960));
        let now = sv.core.sv_time_ms;
        let guid = guid_of(crate::engine::Lan::Cod1, wan);
        sv.core.temp_bans.add(guid, now.wrapping_sub(300_000));
        sv.handle_packet(wan, &oob("getchallenge"), t);
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(
            (cmd.as_str(), rest.as_slice()),
            (
                "error",
                &b"\x15You are temporarily banned from this server"[..]
            )
        );
        sv.core.temp_bans = crate::engine::bans::TempBans::default();
        sv.core.temp_bans.add(guid, now.wrapping_sub(300_001));
        sv.handle_packet(wan, &oob("getchallenge"), t);
        assert_eq!(reply(&mut sv).1, "challengeResponse");
        sv.set_cvar("sv_kickBanTime", "400");
        sv.handle_packet(wan, &oob("getchallenge"), t);
        assert_eq!(reply(&mut sv).1, "error");
    }

    /// 1.5's `ban.txt` (handshake doc, "1.5 permanent bans"): a LAN client
    /// has no guid, so `banUser` answers an empty print and leaves it on,
    /// as the retail 1.5b server did; an off-LAN one is kicked and refused
    /// with retail's string, and `unbanUser` drops it by its cleaned name
    /// with retail's replies.
    #[test]
    fn a_15_ban_kicks_off_lan_refuses_and_unbans_by_name() {
        let t = Instant::now();
        let mut sv = v6_server(t);
        sv.set_cvar("rconpassword", "pw");
        let _lan = active(&mut sv, t);
        let ip = std::net::Ipv4Addr::new(203, 0, 113, 7);
        let wan = SocketAddr::from((ip, 28960));
        let _nc = active_at(&mut sv, wan, t);
        sv.take_outgoing();
        let at = std::cell::Cell::new(t);
        let ask = |sv: &mut Server, line: &str| {
            at.set(at.get() + Duration::from_millis(500));
            rcon(sv, &format!("pw {line}"), at.get()).concat()
        };
        assert_eq!(ask(&mut sv, "unbanUser vcod"), "print\n");
        assert_eq!(ask(&mut sv, "banClient 0"), "print\n");
        assert_eq!(sv.client_count(), 2);
        let guid = guid_of(crate::engine::Lan::Cod1, wan);
        assert!(!sv.core.bans.is_banned(guid));
        assert_eq!(
            ask(&mut sv, "banClient 1"),
            "print\n1:vcod EXE_PLAYERKICKED\n"
        );
        assert_eq!(sv.client_count(), 1);
        assert!(sv.core.bans.is_banned(guid));

        sv.handle_packet(wan, &oob("getchallenge"), at.get());
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(
            (cmd.as_str(), rest.as_slice()),
            (
                "error",
                &b"\x15You are permanently banned from this server"[..]
            )
        );
        for (line, want) in [
            ("unbanUser", "Usage: unban <client name>\n"),
            ("unbanUser a b", "Usage: unban <client name>\n"),
            ("unbanUser nobody", "no banned user has name nobody\n"),
            ("unbanUser ^1VCOD", "unbanned 1 user(s) named VCOD\n"),
        ] {
            assert_eq!(ask(&mut sv, line), format!("print\n{want}"), "{line}");
        }
        sv.handle_packet(wan, &oob("getchallenge"), at.get());
        assert_eq!(reply(&mut sv).1, "challengeResponse");
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

        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping the restart");
            return;
        };
        sv.game.fs = Some(Rc::new(fs));
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
        let Some(fs) = vcod_common::testing::game_fs_11() else {
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
        assert_eq!(sv.game.cfg.map, "mp_carentan");
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
        let Some(fs) = vcod_common::testing::game_fs_11() else {
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
            let ops = move_ops(sv.core.checksum_feed, ack, last);
            let pkt = nc
                .build_out(i32::from(sv.game.server_id), ack, 0, &ops, &huff)
                .unwrap();
            sv.handle_packet(addr(5), &pkt, t);
        }
        sv.tick(t);
        assert_eq!(sv.core.clients[0].as_ref().unwrap().link.ping, 50);
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
            let ops = move_ops(sv.core.checksum_feed, ack, last);
            let pkt = nc
                .build_out(i32::from(sv.game.server_id), ack, 0, &ops, &huff)
                .unwrap();
            let arrived = if i % 2 == 0 {
                ms(clock - 1)
            } else {
                ms(clock + 1)
            };
            let at = crate::engine::Arrival {
                now: t,
                arrived,
                read: arrived,
            };
            sv.packet_read(addr(5), &pkt, at);
        }
        sv.tick(t);
        assert_eq!(sv.core.clients[0].as_ref().unwrap().link.ping, 25);
    }

    /// `gameCompleteStatus` over rcon (handshake doc, "Console commands
    /// over rcon"): a master the heartbeat resolved first is named and sent
    /// nothing; one the command resolves itself gets the serverinfo with
    /// the argument as `challenge` and a line per player.
    /// A UO server heartbeats UO's master unless `sv_master1` says otherwise.
    #[test]
    fn sv_master1_defaults_to_the_protocols_master() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_protocol(&PROTOCOL_V22);
        assert_eq!(sv.core.masters.name(0), "master.cod.pm");
        sv.set_cvar("sv_master1", "127.0.0.1");
        sv.set_protocol(&PROTOCOL_V22);
        assert_eq!(sv.core.masters.name(0), "127.0.0.1");
    }

    #[test]
    fn game_complete_status_reaches_only_the_masters_it_resolved() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_master_resolver(local_master);
        sv.set_cvar("rconPassword", "pw");
        sv.set_cvar("dedicated", "2");
        let _ = connected(&mut sv, addr(5), t);
        sv.tick(t);
        sv.take_outgoing();
        let master = addr(crate::master::DEFAULT_MASTER.port);
        let ask = |sv: &mut Server, line: &str, at: Instant| {
            sv.handle_packet(addr(9), &oob(&format!("rcon pw {line}")), at);
            let out = sv.take_outgoing();
            let text = |to: SocketAddr| -> Vec<String> {
                out.iter()
                    .filter(|(a, _)| *a == to)
                    .map(|(_, p)| String::from_utf8_lossy(&p[4..]).into_owned())
                    .collect()
            };
            (text(addr(9)), text(master))
        };
        let (reply, sent) = ask(&mut sv, "gameCompleteStatus abc", t);
        assert_eq!(
            reply,
            ["print\nSending gameCompleteStatus to master.cod.pm\n"]
        );
        assert!(sent.is_empty(), "{sent:?}");
        sv.set_cvar("sv_master2", "127.0.0.1");
        // A second past the heartbeat, which went to the same host.
        let at = t + Duration::from_millis(1500);
        sv.tick_frames(at, 30);
        let (reply, sent) = ask(&mut sv, "gameCompleteStatus abc", at);
        assert_eq!(
            reply,
            ["print\nSending gameCompleteStatus to master.cod.pm\n\
              Resolving 127.0.0.1\n127.0.0.1 resolved to 127.0.0.1:20510\n\
              Sending gameCompleteStatus to 127.0.0.1\n"]
        );
        assert_eq!(sent.len(), 1);
        let (head, players) = sent[0]
            .split_once('\n')
            .unwrap()
            .1
            .split_once('\n')
            .unwrap();
        assert!(sent[0].starts_with("gameCompleteStatus\n\\"), "{sent:?}");
        assert!(head.ends_with("\\challenge\\abc"), "{head}");
        assert!(!head.contains("pswrd"), "{head}");
        assert_eq!(players.lines().count(), 1, "{players}");
        let at = t + Duration::from_millis(3000);
        sv.tick_frames(at, 30);
        let (_, sent) = ask(&mut sv, "gameCompleteStatus", at);
        assert!(!sent[0].contains("challenge"), "{sent:?}");
    }

    #[test]
    fn dedicated_2_heartbeats_and_flatlines() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_master_resolver(local_master);
        let beats = |sv: &mut Server| -> Vec<String> {
            sv.take_outgoing()
                .into_iter()
                .filter(|(to, _)| *to == addr(crate::master::DEFAULT_MASTER.port))
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
        // One packet a second to a master: the beat a new client forces
        // waits that out.
        let _ = connected(&mut sv, addr(5), t);
        sv.tick(t);
        assert!(beats(&mut sv).is_empty());
        sv.push_console("heartbeat");
        sv.tick_frames(t, 20);
        assert_eq!(beats(&mut sv), ["heartbeat COD-1\n"]);
        sv.tick_frames(t, 20);
        sv.push_console("quit");
        sv.tick(t);
        assert_eq!(beats(&mut sv), ["heartbeat flatline\n"]);
        assert!(sv.quit_requested());
    }
}
