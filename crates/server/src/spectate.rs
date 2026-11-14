//! Server-side client state: usercmds in, wire playerstate out.

use crate::game::host::SimOp;
use glam::Vec3;
use vcod_common::movetrace::{Body, CONTENTS_BODY, CONTENTS_CORPSE, MoveWorld};
use vcod_common::net::flags::{
    EF_CROUCH, EF_DEAD, EF_MOUNTED_DUCK, EF_MOUNTED_PRONE, EF_MOUNTED_STAND, EF_PRONE,
    PMF_BACKWARDS_RUN, PMF_DUCKED, PMF_JUMP_HELD, PMF_LADDER, PMF_OWN_VIEW, PMF_PRONE,
    PMF_PRONE_BLOCKED, PMF_PRONE_DIVE, PMF_RESPAWNED,
};
pub use vcod_common::net::flags::{
    EF_TELEPORT_BIT, PM_DEAD, PM_DEAD_LINKED, PM_INTERMISSION, PM_NORMAL_LINKED, PM_SPECTATOR,
};
use vcod_common::net::msg::{self, UserCmd};
use vcod_common::net::protocol::Protocol;
use vcod_common::net::trajectory;
use vcod_common::pmove::cmd::{self, ANGLE2SHORT, EventRing, view_angles};
use vcod_common::pmove::{self, PmEvent};
use vcod_common::weapon::WeaponDef;

/// `ET_PLAYER`, what another client sees a player as
/// (`crates/client/src/entities.rs` carries the table).
use vcod_common::net::flags::ET_PLAYER;

/// A player entity's `eFlags` and `pos.trDuration`, transcribed from the
/// retail two-probe capture rather than derived.
const PLAYER_EFLAGS: i32 = 16;
const PLAYER_TR_DURATION: i32 = 50;

/// How far behind the frame `ClientSpawn` puts `commandTime` before its own
/// think runs the client up to it (0x42a48).
const SPAWN_THINK_MS: f32 = 100.0;

/// `docs/research/cod11-events-and-fx.md`.
use vcod_common::net::event_ids::EV_PLAYER_TELEPORT_IN;
use vcod_common::net::event_ids::EV_PLAYER_TELEPORT_OUT;
use vcod_common::net::flags::{EF_FIRING, EF_TALK};
/// `pingPlayer`'s bit (docs/research/cod11-hud-protocol.md, "Compass
/// friendlies").
use vcod_common::net::flags::{EF_FRIEND_PING, EF_PING};

/// What `linkTo` left on a client: the parent it follows, the gap it stood
/// at when it linked and the velocity it had then. `Server` re-applies all
/// three every tick, which is `G_RunClient`'s own re-anchor; retail's
/// velocity holds its pre-link value under the link (object-model doc,
/// 23.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Link {
    pub parent: vcod_gsc::EntId,
    pub offset: [f32; 3],
    pub velocity: [f32; 3],
}

/// A rider's animscript conditions: `mounted` names the seat, `vehicle`
/// the kind and `vehicle_motion` the hull's (condition slots 2, 3 and 4 of
/// `BG_AnimUpdatePlayerStateConditions`, game.mp.uo 0x1c973).
fn seat_conditions(seat: Option<Seat>) -> (Option<String>, Option<String>, Option<String>) {
    let Some(s) = seat else {
        return (None, None, None);
    };
    let mounted = match s.pos {
        1 => "vehdriver".to_string(),
        2 => "vehgunner".to_string(),
        n => format!("vehpassenger{}", n - 2),
    };
    let vehicle = match s.kind {
        2 => "tank",
        5 => "flak88",
        _ => "jeep",
    };
    // Slot 4 takes `vehMotion` as it stands, and its value table opens on
    // `** UNUSED **` (game.mp.uo .data 0xa92a0): a first boarding's cmds
    // before the end frame sets the motion match no `vehicle_motion`
    // clause (coduo-vehicles.md 11.4).
    let motion = match s.motion {
        1 => Some("idle"),
        2 => Some("forward"),
        3 => Some("reversing"),
        _ => None,
    };
    (
        Some(mounted),
        Some(vehicle.to_string()),
        motion.map(str::to_string),
    )
}

/// A seated rider's playerstate half: the vehicle's number
/// (`viewlocked_entNum`), `vehPos`, `vehType`, `vehMotion`, the hull's
/// angles pmove clamps the view about, and the hull-view bit (UO
/// `eFlags` 0x200000).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seat {
    pub vehicle: u32,
    pub pos: i32,
    pub kind: i32,
    pub motion: i32,
    pub angles: [f32; 3],
    pub hull_view: bool,
    /// The gunner's MG is overheated (`pm_flags` 0x800000).
    pub overheated: bool,
    /// The offset `VEH_LinkPlayer` linked the player at, in the seat tag's
    /// axes: `BG_GetVehiclePosOffset` of the slot it asked for, which a use
    /// asks as 0 (coduo-vehicles.md 13, "The tank driver's link offset").
    pub link_offset: [f32; 3],
}
use vcod_common::net::event_ids::EV_DEATH;
/// `EV_PAIN` and `EV_DEATH` (`docs/research/cod11-events-and-fx.md`).
use vcod_common::net::event_ids::EV_PAIN;
/// One `EV_PAIN` per 700 ms (combat doc, section 6, step 9).
const PAIN_DEBOUNCE_MS: i32 = 700;
/// `finishPlayerDamage`'s knockback (combat doc, 4.5): the stance scales,
/// the cap, and `g_knockback / 250` at the cvar's stock 1000.
const KNOCKBACK_STAND: f32 = 0.3;
const KNOCKBACK_DUCKED: f32 = 0.15;
const KNOCKBACK_PRONE: f32 = 0.02;
const KNOCKBACK_MAX: i32 = 60;
const KNOCKBACK_UNITS: f32 = 1000.0 / 250.0;

/// `finishPlayerDamage`'s per-frame accumulator (combat doc, 4.5): what the
/// frame's hits added up to and where the last one came from, which
/// `P_DamageFeedback` turns into the four wire fields at end-frame.
#[derive(Clone, Copy, Default)]
struct DamageAccum {
    taken: i32,
    /// The normalised damage direction, or `None` for a hit that carried
    /// no direction, which the feedback marks with 255/255.
    from: Option<Vec3>,
}

/// `ps.damageEvent`, `damageCount`, `damageYaw`, `damagePitch`. They keep
/// their last values between hits: the client detects a hit by
/// `damageEvent` changing (combat doc, section 6).
#[derive(Clone, Copy, Default)]
struct DamageFeedback {
    event: i32,
    count: i32,
    yaw: i32,
    pitch: i32,
}

/// `serverCursorHintString`'s no-hint sentinel, which is retail's -1 in an
/// 8-bit netfield. A different field from `serverCursorHint`, whose own
/// no-hint value is 0. Object model doc, section 20.
const NO_CURSOR_HINT_STRING: i32 = 0xff;

/// `stats[3]`'s "no teammate": retail's -1 in six raw bits, so the wire
/// cannot tell it from client 63 (docs/protocol-1.1.md, "Block 1").
const NO_TEAMMATE: i32 = 63;

/// `ANGLE2SHORT(spawn_angle) - cmd.angles`, RTCW's `SetClientViewAngle`
/// (docs/protocol-1.1.md, "View angles"). `cmd_angles` is the
/// client's last-known angles at the moment of this spawn. Only a fresh
/// connect's are zero; a client spawned by the script has been sending cmds
/// since it entered the world, and without the subtraction its spawn would
/// force-turn the view to the spawn yaw plus whatever it was already
/// looking at.
fn spawn_delta_angles(yaw_deg: f32, cmd_angles: [i32; 3]) -> [i32; 3] {
    [
        -cmd_angles[0],
        (yaw_deg * ANGLE2SHORT) as i32 - cmd_angles[1],
        -cmd_angles[2],
    ]
}

/// The movement path a client is on, and with it the half of the wire
/// playerstate that a spectator and a player disagree about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PmType {
    /// `pm_type` 0 on the wire.
    Normal,
    /// `pm_type` 4 on the wire, the value every client carries before it
    /// answers the team menu.
    Spectator,
    /// `pm_type` 5, the camera a level's end parks every client at
    /// (docs/research/cod11-map-cycle.md section 6.2). Nothing moves it and
    /// nothing it presses is read.
    Intermission,
}

/// One client's simulated state. `pm_type` selects the movement path, the way
/// retail's own `playerState_t` does: a client is a spectator before the menu
/// and a player after, within one connection.
/// The end frame [`ClientSim::commit_pose`] runs in.
#[derive(Clone, Copy, Debug)]
pub struct EndFrame {
    pub frametime_ms: i32,
    /// The client's last processed cmd's serverTime.
    pub command_time: i32,
    /// `level.time`, which the DObj's clocks are kept in.
    pub now_ms: i32,
    pub client_num: i32,
}

/// What [`ClientSim::vehicle_overlap`] found: a body inside a vehicle's
/// hull, whether the vehicle sits out the cmd's pmove, the run-over a
/// moving one dealt, and whether the body lies prone, which takes
/// `VEH_PlayerDamage`'s 10 points every cmd (coduo-vehicles.md 11.6), all
/// at `at`, the body's origin ahead of the pmove.
#[derive(Clone, Copy, Debug)]
pub struct VehicleOverlap {
    pub entity: u32,
    pub unlink: bool,
    pub struck: Option<crate::game::vehicle::physics::PlayerPush>,
    pub prone: bool,
    pub at: Vec3,
}

/// A gunner's turret group and the one his legs left, each with its rows
/// and leaves (`ClientSim::gunner_leaves`, `gunner_left`).
pub type TurretLegs = (
    Option<(usize, Vec<vcod_common::turretpose::TurretRow>)>,
    Option<(usize, Vec<vcod_common::turretpose::TurretRow>)>,
);

pub struct ClientSim {
    pub ps: pmove::PlayerState,
    pub pm_type: PmType,
    /// `ps.eventSequence`, `ps.events` and `ps.eventParms`. Cleared at a
    /// respawn: retail's own respawn frame reads an empty ring at sequence 0
    /// (`docs/research/cod11-combat.md` 9.2).
    pub ring: EventRing,
    /// The model configstring index `setViewmodel` left on the client,
    /// mirrored from the script host every frame the way the weapons are.
    pub viewmodel_index: i32,
    /// `ps.shellshockIndex`, `shellshockTime`, `shellshockDuration`, from
    /// `GameHost::client_shellshock`.
    pub shellshock: [i32; 3],
    /// The body, head and helmet the character script dressed this client in,
    /// mirrored the same way: the locational trace poses the grafted rig they
    /// make (`crate::game::hitrig`).
    pub assembly: crate::game::hitrig::Assembly,
    /// The pose [`Self::commit_pose`] took at the last end frame, which every
    /// locational trace until the next one meets.
    pose: crate::game::combat::BodyPose,
    /// The client's per-axis view offset, added back onto each cmd's angle
    /// by both `step` and the connected client itself.
    /// docs/protocol-1.1.md, "View angles".
    delta_angles: [i32; 3],
    /// `ps.viewangles`, the sum of the two above, kept because the wire
    /// carries it for a player and nothing else in the sim stores the roll.
    /// Frozen for a dead or intermission client, both of which return before
    /// the view update the way retail's own `PM_UpdateViewAngles` is skipped.
    view_angles: [f32; 3],
    /// `serverTime` the running eye-height lerp started at, cleared when it
    /// settles. Retail stamps it and the client runs the lerp from it, so a
    /// zero here makes the client's prediction snap to the target and then be
    /// dragged back once a snapshot. Invisible to a settled capture, which is
    /// why the motion gate cannot pin it (docs/protocol-1.1.md, "The
    /// view-height lerp").
    view_lerp_start: Option<i32>,
    /// The animation channels, driven by `update_anims` after each frame's
    /// moves and read straight into the wire playerstate.
    anim: vcod_common::animscript::AnimState,
    /// Whether the sim was off the ground last frame, so `update_anims` can
    /// raise `jump` and `land` on the edges rather than every frame.
    was_airborne: bool,
    /// The animscript's `strafing` condition, which is state rather than a
    /// reading of this frame's cmd: retail updates it only when the cmd asks
    /// for movement and leaves it alone when both axes are zero
    /// (`game.mp.i386.so` 0x32504).
    strafing: Option<vcod_common::animscript::Side>,
    /// The movetype `update_anims` last selected by: pmove's write of the
    /// record's `movetype` condition, which the end frame's angle updater
    /// reads (combat doc 16.4).
    movetype: vcod_common::animscript::Movetype,
    /// The `movetype` condition as the last pmove's `PM_Footsteps` wrote it,
    /// cmd by cmd: what a weapon event raised in the next pmove selects by
    /// on 1.1 (player-model-anim-system.md, "The weapon event's movetype").
    pub footsteps_movetype: vcod_common::animscript::Movetype,
    /// The turret group the placement last set, with its rows and leaves
    /// for the gun this client mans, which its legs pose by while mounted.
    pub gunner_leaves: Option<(usize, Vec<vcod_common::turretpose::TurretRow>)>,
    /// The turret group the placement last set before the current one, with
    /// its leaves: what the legs blend out of on an aim-fire change.
    pub gunner_left: Option<(usize, Vec<vcod_common::turretpose::TurretRow>)>,
    /// A 1.5 or UO gunner's turret rows and leaves as the anim info carries
    /// them from frame to frame (cod11-turrets.md 13.9).
    gunner_tree: vcod_common::turretpose::GunnerTree,
    /// The takeoff anim a move raised since the last `update_anims`, and the
    /// hold-clock time its timer was set on: the cmd's serverTime, or its
    /// start when the move's `PM_DropTimers` ran it down
    /// ([`pmove::Takeoff`]), and the `movetype` the last pmove's
    /// `PM_Footsteps` left, which every arm that raises one runs ahead of.
    /// A tick that ran several moves keeps the last.
    jumped: Option<(pmove::Takeoff, i32, vcod_common::animscript::Movetype)>,
    /// What each cmd since the last `update_anims` did to the torso outside
    /// the weapon events, with the cmd's serverTime: a prone flip's clear
    /// and UO's `LMG_BREAKDOWN` continue ([`TorsoCmd`]).
    torso_cmds: Vec<(i32, TorsoCmd)>,
    /// `ps.jumpTime`: the serverTime of the step the last jump took, 0 since
    /// the spawn. The client's own prediction refuses a jump inside 500 ms
    /// of it (docs/research/cod11-mantle.md, "Jumps").
    jump_time: i32,
    /// The serverTime of the cmd whose landing since the last
    /// `update_anims` was fast enough for the land anim
    /// (`PlayerState::land_anim`).
    land_anim: Option<i32>,
    /// `ps.stats[0]` and `stats[2]`, mirrored from the host's vitals every
    /// frame; the host is where the script's `self.health` lands.
    pub health: i32,
    pub max_health: i32,
    /// Killed and not yet respawned: no entity, no feedback, no anims.
    pub dead: bool,
    /// `ps.pm_type > 5` as the last end frame wrote it off `dead`
    /// (`ClientEndFrame` 0x41079): the wire's `PM_DEAD` and what the cmds
    /// read, so the cmds a death lands ahead of still run the live move
    /// (`docs/research/cod11-combat.md` 9.2).
    pm_dead: bool,
    /// `player_die`'s 30-unit `r.maxs[2]`, which holds until the next cmd's
    /// think copies `pm.maxs` over it ([`ClientSim::box_height`]).
    died_box: bool,
    /// The script's `sessionstate` as the frame's end frame found it;
    /// `None` with no script client behind the sim, where `dead` stands
    /// in. `ClientEndFrame` keys its arms on the session, not the health,
    /// so a `suicide()` that leaves `playing` keeps `pm_type` 0 and stays
    /// on the wire (docs/research/cod11-combat.md 5.1, "A suicide that
    /// stays playing").
    pub session: Option<crate::follow::SessionState>,
    /// The last move ran the dead arm, whose `PM_CheckDuck` targets
    /// `deadViewHeight`.
    dead_eye: bool,
    /// `linkTo`'s record, `gentity_t+0x2e4`. Not `linked()`, which is about
    /// whether the other clients are sent an entity for this one.
    pub link_to: Option<Link>,
    /// United Offensive: the vehicle seat this client rides
    /// (docs/research/coduo-vehicles.md 11), `None` on foot.
    pub seat: Option<Seat>,
    /// The seat tag's world angles as the last end frame placed the body
    /// (`G_PlayerVehiclePositionAndBlend`'s `r.currentAngles`), `None` on
    /// foot or before the first placement.
    pub seat_angles: Option<[f32; 3]>,
    /// The vehicle's steer (`s.leanf`) as the last end frame read it, which
    /// a four-wheel's driver and passenger blend by.
    pub seat_steer: f32,
    /// The entity fields the controllers' hook reads, as the last
    /// `BG_PlayerStateToEntityState` wrote them ([`Self::write_entity`]).
    hook_es: crate::game::combat::HookEs,
    /// The record's seat angles (`+0x3f4`): the last placement's, kept past
    /// an exit.
    ci_seat_angles: [f32; 3],
    /// `ps.vehType` and `ps.vehMotion`, which an exit leaves as they were.
    pub veh_type: i32,
    pub veh_motion: i32,
    /// `pers.cmd`: the last cmd that ran, what a vehicle's think reads its
    /// driver's and gunner's input off (a cmd repeating the last one's
    /// time never runs, so never lands here).
    pub pers_cmd: UserCmd,
    /// The last link was a `setOrigin`'s or `TeleportPlayer`'s, at the
    /// unsnapped `ps.origin`; the next cmd's link clears it.
    linked_unsnapped: bool,
    /// Out of a seat since this client's last `G_RunClient` or end frame:
    /// `pm_type` stays 1 (7 dead) until one of them writes it again, so the
    /// cmds left in an exit's frame run linked and move nothing
    /// (docs/research/coduo-vehicles.md 11.2).
    unseated: bool,
    /// `ps.serverCursorHint`, written every frame by the end-of-frame pass
    /// (`ScriptRuntime::cursor_hint_pass`).
    pub cursor_hint: i32,
    /// `ps.serverCursorHintVal`, the same pass's: a mount's crouch flag.
    pub cursor_hint_val: i32,
    /// `ps.serverCursorHintString`, -1 for none: the same pass writes it,
    /// and leaves it alone for a dead player or a gunner (turrets doc 4.3).
    pub cursor_hint_string: i32,
    /// `iHeadIcon` and `iHeadIconTeam`, which script's `.headicon` and
    /// `.headiconteam` own; the end frame mirrors them here.
    pub head_icon: i32,
    pub head_icon_team: i32,
    damage: DamageAccum,
    feedback: DamageFeedback,
    /// The last hit another client dealt this life: his slot, the
    /// `sv_time` it landed and the damage direction (from him toward this
    /// body; zero when it carried none). Read by the bots
    /// (`bots::Hurt`), never sent.
    pub last_hit: Option<(usize, i32, [f32; 3])>,
    /// `ps.stats[1]`, the yaw toward the killer (combat doc, 5.1, item 11).
    dead_yaw: i32,
    /// When the next `EV_PAIN` may fire.
    pain_after_ms: i32,
    /// The aim block's state between cmds (`pmove::aim`, combat doc 15).
    aim_state: pmove::aim::AimState,
    /// `ps.viewangles` as `ClientSpawn`'s `SetClientViewAngle` leaves them
    /// for its think's aim block: the spawn's yaw as given, not quantized
    /// (180 stays 180 where a cmd's view reads -180).
    spawn_view: [f32; 3],
    /// The last damage's kick, which the aim block plays out
    /// (`P_DamageFeedback` step 6, combat doc 6).
    kick: pmove::aim::DamageKick,
    /// The gun-kick spring the aim block adds down the sight. Only 1.5's and
    /// UO's servers kick it, off each fire event (`BG_WeaponFireRecoil`,
    /// cod15-movement.md 6); on 1.1 it stays zero (combat doc 15.2).
    gun_kick: pmove::aim::GunKick,
    /// `client+0x220c/0x2210`: where this client's next shot, swing or
    /// throw goes, computed once per cmd ahead of its moves.
    aim: [f32; 2],
    /// [`Self::aim`] less the view it was built from, wrapped to ±180.
    gun_offset: [f32; 2],
    /// `EF_TELEPORT_BIT`'s current state, flipped by every spawn of any
    /// mode (docs/research/cod11-map-cycle.md, 8.2).
    teleport_bit: bool,
    /// `ps.stats[5]`, the spawn counter retail's `ClientSpawn` carries across
    /// its own memset (docs/protocol-1.1.md, "Block 1"). A byte on the wire,
    /// so it wraps; every spawn of either mode bumps it, which puts the lone
    /// spectator's capture at 1.
    spawn_count: u8,
    /// `ps.viewlocked` (0 free, 1 on a turret, 2 on a frame the turret
    /// fired), `ps.viewlocked_entNum` (the turret; 0 after a spawn,
    /// `ENTITYNUM_NONE` after a release, turrets doc 12.7) and `ps.gunfx`.
    pub viewlocked: u8,
    pub viewlocked_ent: u32,
    pub gunfx: u8,
    /// The turret this client mans: `s.otherEntityNum` and the gun's stance
    /// as `eFlags` 0xC000/0x8000/0x4000 (turrets doc, 4.4).
    pub mounted_on: Option<(u32, crate::game::turret::TurretStance)>,
    /// `ps.eFlags` 0x400, on a frame the gun this client mans fired (turrets
    /// doc 12.4).
    pub firing: bool,
    /// `ps.eFlags` 0x400 as `PmoveSingle` writes it every cmd: attack held
    /// with the weapon ready or firing and a loaded clip ([`attack_flag`];
    /// combat doc 16.5).
    pub attacking: bool,
    /// `ps.eFlags` 0x40000 as `PmoveSingle` writes it every cmd: the cmd's
    /// talk bit (1.1 0x33f6a, 1.5b 0x281d4; UO's 0x20000 at 0x2d8af).
    pub talking: bool,
    /// `eFlags` 0x80000, `pingPlayer`'s chat flash on teammates' compasses,
    /// held until the stamp the builtin left (`GameHost::client_ping_until`).
    pub ping: bool,
    /// `eFlags` 0x20000, this client's vote in on the running vote
    /// (`GameHost::client_voted`, docs/research/cod11-votes.md).
    pub voted: bool,
    /// `ps.iCompassFriendInfo`, the out-of-view teammate the end frame packs
    /// (`crate::compass`), 0 for none.
    pub compass_friend: i32,
    /// `client + 0x2264`: the slot of the last answer, where the next scan
    /// starts after.
    pub last_friend: u32,
    /// `ps.eFlags` 0x100000: that teammate's `pingPlayer` bit, which the
    /// compass flashes on.
    pub friend_ping: bool,
    /// The last cmd's angles, retail's `pers.cmd.angles`, which
    /// `set_view_angle` rewrites `delta_angles` against.
    last_cmd_angles: [i32; 3],
    /// `r.contents`: `CONTENTS_BODY` while alive and playing, `CONTENTS_CORPSE`
    /// after a death or a stuck push until the next end frame, else 0.
    pub contents: u32,
    /// The entity `solid`, packed at the last link from the box and contents
    /// then, never at end frame (docs/research/cod11-player-clip.md).
    pub linked_solid: i32,
    /// Who this spectator's view rides (`crate::follow`).
    pub follow: crate::follow::Follow,
    /// `pm_flags` 0x40000 as the last end frame left it: a playing or dead
    /// client, which is what a follow may copy. `ClientSpawn`'s own end
    /// frame sets it for a player and clears it for anyone else.
    pub own_view: bool,
    /// 1.5's spectator permission bits as the last end frame left them,
    /// `flags::PMF_MAY_CYCLE` and `PMF_MAY_FREE`, ORed into the frame's
    /// `pm_flags`.
    pub spectate_flags: i32,
    /// The buttons of the last cmd this client ran, `client+0x21e8`, which
    /// the next cmd's edges are taken against. The spawn's memset zeroes it.
    pub last_buttons: u8,
    /// This frame's `ClientEndFrame` spawned the client out of a follow's
    /// copy, whose memset leaves the frame's HUD arrays empty; the snapshot
    /// consumes it.
    pub hud_cleared: bool,
    /// `pm_flags` 0x800 ([`PMF_RESPAWNED`]).
    respawned: bool,
    /// UO's spawn built the DObj inside the frame's script threads, so the
    /// clips the spawn set advance in that frame's anim update, a frame ahead
    /// of 1.1's and 1.5's (cod11-turrets.md 13.7). The next end frame takes it.
    pub dobj_ahead: bool,
    /// `aimSpreadScale / 255` as the last `ClientEndFrame` stored it
    /// (1.1d `client+0x2240` 0x410f1, UO `client+0x4698`): what a shot's
    /// cone reads, so a hip shot's own `hipSpreadFireAdd` widens only the
    /// next one (combat doc 2.1).
    pub spread_frac: f32,
    /// 1.5's and UO's `ci+0x4a4` / `ci+0x4c0`: until when a crouch or
    /// prone change stretches every blend (`animclock::ChannelClock::update_windowed`).
    stance_window_ms: i32,
    /// What `ClientEndFrame` stores in a playing body's `ps.speed`: `g_speed`
    /// on 1.1, the client's `maxspeed` on 1.5 and UO, mirrored off the host
    /// every frame (cod15-movement.md, "ps.speed").
    pub max_speed: i32,
    /// What `ClientEndFrame` stores in a playing body's `ps.gravity`:
    /// `(int)g_gravity`, mirrored off the host every frame
    /// (cod15-movement.md, "ps.gravity").
    pub gravity: i32,
    /// The last frame a follow sent this spectator, the copy with its flags
    /// patched: the playerstate `StopFollowing` writes over.
    pub follow_wire: Option<msg::PlayerState>,
    /// What a stopped follow left in the playerstate, until the next spawn.
    residue: Option<Residue>,
    /// This client's own frame as its last `ClientEndFrame` left it, `None`
    /// since a spawn, whose own end frame is this frame's.
    pub end_frame_wire: Option<msg::PlayerState>,
    /// The ring as it stood before this frame's end frame put `EV_PAIN` on
    /// it, which a follower numbered below this client copies; `None` on a
    /// frame without one. Dropped after the frame's snapshots.
    pub ring_before_pain: Option<EventRing>,
    /// The entity state this client's own `ClientEndFrame` left, with the
    /// frame's time; `None` when it went out unlinked. A client a higher
    /// slot kills later in the frame is still sent it (combat doc 5.4).
    pub end_frame_entity: Option<(i32, msg::EntityState)>,
    /// The intermission camera's `ps.commandTime`: `ClientSpawn` puts it
    /// 100 ms behind the spawn's frame and nothing on that arm moves it
    /// (`docs/research/cod11-spectator-follow.md` 13).
    frozen_command_time: Option<i32>,
}

/// A stopped follow's copy, under the fields a spectator's own frame writes.
struct Residue {
    wire: msg::PlayerState,
    /// A cmd has run `SpectatorThink`'s free-flight arm since the stop, which
    /// writes `pm_type` and `speed` (0x3fb94, 0x3fb9b).
    moved: bool,
}

/// What a free spectator's frame writes over a stopped follow's copy:
/// `StopFollowing`'s stores (0x46a28), `SpectatorThink`'s flight and
/// `SpectatorClientEndFrame`'s (spectator-follow doc, 7 and 5).
const SPECTATOR_OWNED: &[&str] = &[
    "commandTime",
    "clientNum",
    "origin[0]",
    "origin[1]",
    "origin[2]",
    "velocity[0]",
    "velocity[1]",
    "velocity[2]",
    "viewangles[0]",
    "viewangles[1]",
    "viewangles[2]",
    "delta_angles[0]",
    "delta_angles[1]",
    "delta_angles[2]",
    "fWeaponPosFrac",
    "viewHeightTarget",
    "viewHeightCurrent",
    "viewmodelIndex",
    "viewlocked",
    "viewlocked_entNum",
    "gunfx",
    "shellshockIndex",
    "shellshockTime",
    "shellshockDuration",
];

/// Everything the animscript needs that the sim does not own: the script
/// itself, the name-to-index lookup, and the weapon the client holds.
pub struct AnimInputs<'a> {
    pub anims: &'a vcod_common::animtree::PlayerAnims,
    pub weapon: &'a str,
    pub weapon_class: &'a str,
}

impl AnimInputs<'_> {
    /// The `weapons` condition's name: weapon 0 (no name) is `none`, which
    /// the file's `weapons none` clauses select. VERIFIED, live (1.1d,
    /// 1.5b, UO 1.51b): an unarmed prone body plays `pb_prone_aim_pistol`
    /// and an unarmed crouch `pb_crouch_alert_unarmed`
    /// (player-model-anim-system.md, "Unarmed").
    fn condition_weapon(&self) -> String {
        if self.weapon.is_empty() {
            "none".to_string()
        } else {
            self.weapon.to_ascii_lowercase()
        }
    }
}

/// The `EVENTS` block a weapon event raises, or `None` for one the script has
/// nothing for. Retail's `PM_Weapon` raises `BG_AnimScriptEvent` where it
/// enters the state (docs/research/cod11-combat.md, sections 1.5, 1.7, 1.8
/// and 1.10); the block's clauses are what pick the anim.
fn weapon_anim_event(event: i32) -> Option<&'static str> {
    use vcod_common::net::event_ids::{EV_UO_BREAKDOWN_WEAPON, EV_UO_DEPLOY_WEAPON};
    use vcod_common::pmove::weapon::{
        EV_FIRE_WEAPON, EV_FIRE_WEAPON_LASTSHOT, EV_MELEE_SWIPE, EV_PUTAWAY_WEAPON,
        EV_RAISE_WEAPON, EV_RELOAD, EV_RELOAD_FROM_EMPTY, EV_RELOAD_START,
    };
    Some(match event {
        EV_FIRE_WEAPON | EV_FIRE_WEAPON_LASTSHOT => "fireweapon",
        EV_RELOAD | EV_RELOAD_FROM_EMPTY | EV_RELOAD_START => "reload",
        EV_PUTAWAY_WEAPON => "dropweapon",
        EV_RAISE_WEAPON => "raiseweapon",
        // The swing carries the anim; `EV_FIRE_MELEE`, the damage frame
        // 150 ms later, maps to nothing (combat doc, 1.10).
        EV_MELEE_SWIPE => "meleeattack",
        // UO's deploy and breakdown raise their own blocks beside the
        // event, forced (0x447aa, 0x4484c in Ghidra's numbering).
        EV_UO_DEPLOY_WEAPON => "lmg_deploy",
        EV_UO_BREAKDOWN_WEAPON => "lmg_breakdown",
        _ => return None,
    })
}

/// A weapon event one pmove raised: the event, its cmd's `serverTime` and
/// the `movetype` condition it selects by ([`ClientSim::note_footsteps`]).
pub type AnimEvent = (PmEvent, i32, vcod_common::animscript::Movetype);

/// A move's write to the torso that is not a weapon event, in the order a
/// cmd runs them: `PM_CheckDuck`'s, then `PM_Weapon`'s.
#[derive(Clone, Copy, Debug)]
enum TorsoCmd {
    /// 1.5's and UO's prone flip ([`pmove::PlayerState`]'s
    /// `prone_flip_clears_torso`): index 0 as a forced continue, no timer.
    ProneFlipClear,
    /// UO's `LMG_BREAKDOWN` continue (`breakdown_continue`), with the
    /// `movetype` its `PM_Footsteps` left, which UO runs ahead of
    /// `PM_Weapon`.
    BreakdownContinue(vcod_common::animscript::Movetype),
}

/// Horizontal speed below which the animscript sees an idle player, whatever
/// the input. A player pressed into geometry keeps a few units a second and
/// retail's capture animates it standing; the value sits between that
/// capture's 8 u/s blocked run and its 13 u/s prone crawl
/// (`crates/server/tests/playerstate_motion_ab.rs`).
pub const ANIM_IDLE_SPEED: f32 = 10.0;

/// Horizontal speed at or under which a crouched or prone body with no move
/// key held reselects its idle over the move clip (`PM_Footsteps` 0x3282a,
/// rodata 0x70c48; player-model-anim-system.md, "How retail picks the
/// movetype").
pub const ANIM_STOP_SPEED: f32 = 120.0;

impl ClientSim {
    /// A client entering the world: the spectator fly
    /// ([`pmove::spectator_move`]), angles straight off the latest cmd.
    /// `cmd_angles` are the angles of the usercmd that brought the client in
    /// -- `SV_ClientEnterWorld`'s `cmds[0]`, zero when there is none -- so
    /// the view it already had survives entry instead of snapping.
    pub fn spectator(origin: [f32; 3], yaw_deg: f32, cmd_angles: [i32; 3]) -> Self {
        let view = view_angles(cmd_angles, spawn_delta_angles(yaw_deg, cmd_angles));
        let mut ps = pmove::PlayerState::spawn(Vec3::from(origin), yaw_deg);
        ps.speed = pmove::SPEED_SPECTATOR;
        ClientSim {
            ps,
            pm_type: PmType::Spectator,
            ring: EventRing::default(),
            viewmodel_index: 0,
            shellshock: [0; 3],
            assembly: Default::default(),
            pose: Default::default(),
            delta_angles: spawn_delta_angles(yaw_deg, cmd_angles),
            view_angles: view,
            aim_state: Default::default(),
            spawn_view: [0.0; 3],
            kick: Default::default(),
            gun_kick: Default::default(),
            aim: [view[0], view[1]],
            gun_offset: [0.0; 2],
            view_lerp_start: None,
            anim: Default::default(),
            was_airborne: false,
            strafing: None,
            movetype: Default::default(),
            footsteps_movetype: Default::default(),
            gunner_leaves: None,
            gunner_left: None,
            gunner_tree: Default::default(),
            jumped: None,
            torso_cmds: Vec::new(),
            jump_time: 0,
            land_anim: None,
            link_to: None,
            seat: None,
            seat_angles: None,
            seat_steer: 0.0,
            hook_es: Default::default(),
            ci_seat_angles: [0.0; 3],
            veh_type: 0,
            veh_motion: 0,
            pers_cmd: msg::NULL_USERCMD,
            linked_unsnapped: false,
            unseated: false,
            cursor_hint: 0,
            cursor_hint_val: 0,
            cursor_hint_string: -1,
            head_icon: 0,
            head_icon_team: 0,
            health: 0,
            max_health: 0,
            dead: false,
            pm_dead: false,
            died_box: false,
            session: None,
            dead_eye: false,
            damage: DamageAccum::default(),
            feedback: DamageFeedback::default(),
            last_hit: None,
            dead_yaw: 0,
            pain_after_ms: 0,
            // Clear, the way `ClientConnect`'s memset leaves `ps.eFlags`:
            // the connect's own `spawnSpectator` is the flip that puts the
            // first spectator frame at 24 and the life after it back at 16.
            teleport_bit: false,
            // The constructor is the connect, before any spawn: the script's
            // own `spawnSpectator` is what takes it to the capture's 1.
            spawn_count: 0,
            viewlocked: 0,
            viewlocked_ent: 0,
            gunfx: 0,
            mounted_on: None,
            firing: false,
            attacking: false,
            talking: false,
            ping: false,
            voted: false,
            compass_friend: 0,
            last_friend: 0,
            friend_ping: false,
            last_cmd_angles: cmd_angles,
            contents: 0,
            linked_solid: 0,
            follow: Default::default(),
            own_view: false,
            spectate_flags: 0,
            last_buttons: 0,
            hud_cleared: false,
            respawned: false,
            dobj_ahead: false,
            spread_frac: 0.0,
            stance_window_ms: 0,
            max_speed: pmove::SPEED_RUN as i32,
            gravity: pmove::GRAVITY as i32,
            follow_wire: None,
            residue: None,
            end_frame_wire: None,
            ring_before_pain: None,
            end_frame_entity: None,
            frozen_command_time: None,
        }
    }

    /// The mode change `spawnPlayer()` makes: the same sim, restarted at the
    /// spawn point the script chose. `cmd_angles` is the client's last-known
    /// cmd angles going into the spawn -- a spectator can have turned freely
    /// before answering the weapon menu, unlike a fresh connect, so the
    /// caller must supply the real value rather than assume zero.
    pub fn become_player(&mut self, origin: [f32; 3], yaw_deg: f32, cmd_angles: [i32; 3]) {
        self.respawn(PmType::Normal, origin, yaw_deg, cmd_angles);
        self.spawn_think(None);
    }

    /// The other half of the same builtin: `spawnSpectator()` parks a client
    /// at an intermission point through the very same `self spawn(origin,
    /// angles)`, so a spectator moves for exactly the reasons a player does.
    pub fn become_spectator(&mut self, origin: [f32; 3], yaw_deg: f32, cmd_angles: [i32; 3]) {
        self.respawn(PmType::Spectator, origin, yaw_deg, cmd_angles);
        self.spawn_think(None);
        // As the intermission camera below: the spectator arm copies no
        // health either, so a player parked as a spectator after a death
        // reads 0 where its entity still holds 100 (the retail round-restart
        // target reads `pm_type=4 health=0` on every such frame).
        self.health = 0;
        self.max_health = 0;
    }

    /// `ClientEndFrame`'s spawn arm (0x40f82): a playing or dead client whose
    /// playerstate is still a follow's copy is spawned at the copy's feet and
    /// yaw. The copy's `eFlags` are what the spawn flips the teleport bit of,
    /// and a dead one's own end frame takes the dead arm, so it is a dead
    /// player with no contents (the stock killcam's return to `dead`), whose
    /// eye the spawn's own think starts dropping in `world`.
    pub fn spawn_from_copy(
        &mut self,
        copied: &crate::follow::Copied,
        playing: bool,
        cmd_angles: [i32; 3],
        world: Option<MoveWorld<'_>>,
    ) {
        self.teleport_bit = copied.teleport_bit;
        self.respawn(PmType::Normal, copied.origin, copied.angles[1], cmd_angles);
        self.hud_cleared = true;
        if !playing {
            self.dead = true;
            self.pm_dead = true;
            self.contents = 0;
            self.relink();
        }
        self.spawn_think(world);
    }

    /// `eFlags` 0x8 as the wire carries it.
    pub fn teleport_bit(&self) -> bool {
        self.teleport_bit
    }

    /// The third mode, through the same `self spawn(origin, angles)`:
    /// `spawnIntermission()` parks the client at the map's intermission
    /// point for the level's last ten seconds (map-cycle doc, section 6).
    /// `now_ms` is the spawn's frame.
    pub fn become_intermission(
        &mut self,
        origin: [f32; 3],
        yaw_deg: f32,
        cmd_angles: [i32; 3],
        now_ms: i32,
    ) {
        self.respawn(PmType::Intermission, origin, yaw_deg, cmd_angles);
        self.frozen_command_time = Some(now_ms.wrapping_sub(SPAWN_THINK_MS as i32));
        self.spawn_think(None);
        // `ClientSpawn` zeroes the whole `gclient_t` and `ClientEndFrame`'s
        // intermission arm never copies `ent->health` back into the
        // playerstate, so the capture's `pm_type=5` traces read health 0
        // where the same client read 100 a frame earlier. `Server`'s vitals
        // mirror leaves an intermission sim alone for the same reason.
        self.health = 0;
        self.max_health = 0;
    }

    /// Whether the sim is something the other clients are sent an entity
    /// for. Retail links neither spectator nor intermission client and sets
    /// `SVF_NOCLIENT` on a dead one every frame (combat doc, 5.4;
    /// map-cycle doc, 6.2).
    pub fn linked(&self) -> bool {
        self.pm_type == PmType::Normal && !self.hidden()
    }

    /// Off the wire with no contents: every `ClientEndFrame` arm but the
    /// playing one sets `SVF_NOCLIENT` (dead 0x41057, intermission 0x40ef0,
    /// spectator `SpectatorClientEndFrame` 0x40760).
    fn hidden(&self) -> bool {
        use crate::follow::SessionState;
        self.session
            .map_or(self.dead, |s| s != SessionState::Playing)
    }

    /// `pm_type` 6: the dead arm (0x41079); a sim whose session is neither
    /// playing nor dead keeps the health rule.
    fn dead_arm(&self) -> bool {
        use crate::follow::SessionState;
        match self.session {
            Some(SessionState::Playing) => false,
            Some(SessionState::Dead) => true,
            _ => self.dead,
        }
    }

    /// The height of `r.maxs`, which the touch pass and `isTouching` read:
    /// `player_die` writes 30 (1.1d 0x49d6d, 1.5b 0x470d5, UO 0x4fd0b) until
    /// the next cmd's think, a dead think copies `PM_CheckDuck`'s dead arm,
    /// `ps.maxs`, the standing 70 (1.1d 0x317f6, 1.5b 0x2352c, UO 0x2807e),
    /// and a live one the stance box (cod11-gsc-object-model.md 22.3).
    pub fn box_height(&self) -> f32 {
        if self.died_box {
            crate::game::host::PLAYER_DIE_HEIGHT
        } else if self.pm_dead {
            pmove::HEIGHT_STAND
        } else {
            self.ps.box_height()
        }
    }

    /// `ps.groundEntityNum != ENTITYNUM_NONE`, which is what
    /// `PlayerCmd_isOnGround` answers off. The linked arm writes
    /// `ENTITYNUM_NONE`, so `isOnGround` is false under a link from the first
    /// frame whose cmds ran linked (docs/research/cod11-gsc-object-model.md,
    /// 23.2 and 23.5).
    pub fn on_ground(&self) -> bool {
        self.ps.on_ground
    }

    /// `ps.pm_type` as the wire carries it. The touch pass gates on it, so it
    /// is read outside `to_wire` too.
    pub fn wire_pm_type(&self) -> i32 {
        match (self.pm_type, self.pm_dead) {
            (PmType::Normal, true)
                if self.link_to.is_some() || self.seat.is_some() || self.unseated =>
            {
                PM_DEAD_LINKED
            }
            (PmType::Normal, false)
                if self.link_to.is_some() || self.seat.is_some() || self.unseated =>
            {
                PM_NORMAL_LINKED
            }
            (PmType::Normal, true) => PM_DEAD,
            (PmType::Normal, false) => 0,
            (PmType::Intermission, _) => PM_INTERMISSION,
            (PmType::Spectator, _) => PM_SPECTATOR,
        }
    }

    fn respawn(&mut self, mode: PmType, origin: [f32; 3], yaw_deg: f32, cmd_angles: [i32; 3]) {
        let (rules, bg) = (self.ps.rules, self.ps.bg);
        self.ps = pmove::PlayerState::spawn(Vec3::from(origin), yaw_deg);
        self.ps.rules = rules;
        self.ps.bg = bg;
        // The turret rows live in the DObj's anim info, which outlives a
        // dismount; a spawn's `G_DObjUpdate` builds a fresh one.
        self.gunner_tree = Default::default();
        // The spawn's `ClientEndFrame` stores a player's speed ahead of its
        // think (0x42a75); a spectator's think is `SpectatorThink`'s 400.
        self.ps.speed = match mode {
            PmType::Spectator => pmove::SPEED_SPECTATOR,
            _ => self.max_speed as f32,
        };
        self.ps.gravity = self.gravity as f32;
        // `ClientSpawn`'s memset leaves `groundEntityNum` 0, not 0x3ff, and
        // `PM_CheckDuck` runs ahead of the ground trace (1.5 0x28814,
        // 0x28819): the think's first step takes 1.5's prone press, which
        // it drops for a body in the air (0x237d6). 1.1's think has no keys.
        self.ps.on_ground = rules.v15();
        self.pm_type = mode;
        self.delta_angles = spawn_delta_angles(yaw_deg, cmd_angles);
        self.view_angles = view_angles(cmd_angles, self.delta_angles);
        // `ClientSpawn`'s memset clears the aim block's state with the rest
        // of the client.
        self.aim_state = Default::default();
        self.spread_frac = 0.0;
        self.spawn_view = [0.0, yaw_deg, 0.0];
        self.kick = Default::default();
        self.gun_kick = Default::default();
        self.aim = [self.view_angles[0], self.view_angles[1]];
        self.gun_offset = [0.0; 2];
        // `ClientSpawn` calls `G_EntUnlink` on the spawning client
        // (object-model doc, 23.2).
        self.link_to = None;
        self.unseated = false;
        // The memset takes the seat's `vehType` and `vehMotion`, which an
        // exit or a death in the seat left (coduo-vehicles.md 11.7).
        self.veh_type = 0;
        self.veh_motion = 0;
        // `ClientSpawn`'s memset. Only a playing client's end frame writes
        // the hint again, so a spectator and the intermission camera keep 0.
        self.set_cursor_hint(0, 0);
        self.cursor_hint_string = -1;
        // A respawned player does not resume the anim it died in.
        self.anim = Default::default();
        self.was_airborne = false;
        self.strafing = None;
        self.jumped = None;
        self.torso_cmds.clear();
        self.jump_time = 0;
        self.land_anim = None;
        // `ClientSpawn`'s memset: the damage fields read 0 again after a
        // respawn (combat doc, 8.4), and so does the dead yaw.
        self.dead = false;
        self.pm_dead = false;
        self.died_box = false;
        self.dead_eye = false;
        self.damage = DamageAccum::default();
        self.feedback = DamageFeedback::default();
        self.last_hit = None;
        self.dead_yaw = 0;
        self.pain_after_ms = 0;
        self.spawn_count = self.spawn_count.wrapping_add(1);
        // The memset again: the capture's first trace, after a spawn, reads
        // `viewlocked_entNum` 0 (turrets doc, 12.7). `ps.mounted` went with
        // the fresh `ps` above.
        self.viewlocked = 0;
        self.viewlocked_ent = 0;
        self.gunfx = 0;
        self.mounted_on = None;
        self.firing = false;
        self.attacking = false;
        self.talking = false;
        // The memset again; nothing after it in `ClientSpawn` rewrites them.
        self.compass_friend = 0;
        self.last_friend = 0;
        self.friend_ping = false;
        self.last_cmd_angles = cmd_angles;
        // Retail's respawn frame reads an empty ring at sequence 0
        // (combat doc, 9.2).
        self.ring.clear();
        // The memset again, which keeps only `sess` and writes -1 to the
        // follow target after it (0x4282c).
        self.follow = Default::default();
        self.last_buttons = 0;
        // Every spawn consumes a flip, a spectator's and the intermission
        // camera's included: retail's capture reads 16 on a respawn's
        // spectator frame and 24 on the next one (map-cycle doc, 8.2).
        self.teleport_bit = !self.teleport_bit;
        self.respawned = true;
        self.follow_wire = None;
        self.residue = None;
        self.end_frame_wire = None;
        self.frozen_command_time = None;
        // `G_SetClientContents`, then the spawn's link.
        self.contents = if mode == PmType::Normal {
            CONTENTS_BODY
        } else {
            0
        };
        self.relink();
    }

    /// `ClientSpawn`'s closing `ClientThink_real` (0x42a82): a cmd with no
    /// buttons 100 ms past the `commandTime` the spawn set (0x42a6f), which
    /// the caller moves up to the frame's clock. The intermission arm runs no
    /// pmove and a dead one keeps the flag, so only a live or spectating spawn
    /// loses `PMF_RESPAWNED` here; a dead one's eye drops those 100 ms. The
    /// live arm's 100 ms of pmove is [`Self::spawn_move`].
    fn spawn_think(&mut self, world: Option<MoveWorld<'_>>) {
        // The spawn's own `ClientEndFrame` (0x42a75) ahead of the think: its
        // playing and dead arm gives the client its own view at once.
        self.own_view = self.pm_type == PmType::Normal;
        match (self.pm_type, self.dead) {
            (PmType::Intermission, _) => {}
            (PmType::Normal, true) => {
                if let Some(w) = world {
                    for dt in [pmove::MAX_FRAME_MS, SPAWN_THINK_MS - pmove::MAX_FRAME_MS] {
                        for e in pmove::dead_move(&mut self.ps, &w, dt / 1000.0) {
                            self.ring.add(e.event, e.parm);
                        }
                    }
                    self.dead_eye = true;
                }
            }
            _ => self.respawned = false,
        }
    }

    /// United Offensive's `ClientThink_real` ahead of `Pmove` (game.mp.uo
    /// 0x415e4..0x419c3, coduo-vehicles.md 11.6): a client on foot whose box
    /// starts inside a vehicle (`eType` 12) gets out of it. The vehicle is
    /// unlinked for this cmd's pmove while the client is slower than 1 or
    /// moves away from it (the flat dot of the two directions past 0.8);
    /// one slower than 1 and not prone is also pushed off it, by a moving
    /// vehicle's `VEH_PlayerCollision` (its run-over damage and sound are
    /// the caller's, [`VehicleOverlap::struck`]) or at 200 along the flat
    /// direction from a resting one's origin, with a 200 ms damage timer.
    /// `vehicle` gives a vehicle entity's `r.currentOrigin` and world
    /// velocity.
    pub fn vehicle_overlap(
        &mut self,
        world: MoveWorld<'_>,
        vehicle: impl Fn(u32) -> Option<(Vec3, Vec3)>,
    ) -> Option<VehicleOverlap> {
        if self.seat.is_some() || self.pm_type == PmType::Intermission {
            return None;
        }
        let mask = if self.pm_dead {
            vcod_common::movetrace::MASK_DEADSOLID
        } else {
            vcod_common::collision::MASK_PLAYERSOLID
        };
        let mut maxs = self.ps.maxs();
        maxs.z = match self.ps.stance {
            pmove::Stance::Prone => pmove::HEIGHT_PRONE,
            pmove::Stance::Crouch => pmove::HEIGHT_CROUCH,
            _ => maxs.z,
        };
        let at = self.ps.origin;
        let t = world.box_trace(at, at, self.ps.mins(), maxs, mask);
        if !t.startsolid && !t.allsolid {
            return None;
        }
        let ent = world.entity_num(&t);
        let (origin, velocity) = vehicle(ent)?;
        // Both `VectorNormalize`d whole, then their z dropped unrenormalized.
        let unit = |v: Vec3| {
            let l = (f64::from(v.x).powi(2) + f64::from(v.y).powi(2) + f64::from(v.z).powi(2))
                .sqrt() as f32;
            (if l != 0.0 { v * (1.0 / l) } else { v }, l)
        };
        let (mut dir, _) = unit(at - origin);
        let (mut along, speed) = unit(self.ps.velocity);
        dir.z = 0.0;
        along.z = 0.0;
        let dot = f64::from(dir.x) * f64::from(along.x) + f64::from(dir.y) * f64::from(along.y);
        let prone = self.ps.stance == pmove::Stance::Prone;
        let mut out = VehicleOverlap {
            entity: ent,
            unlink: speed < 1.0 || dot > f64::from(0.8f32),
            struck: None,
            prone,
            at,
        };
        if !out.unlink {
            return Some(out);
        }
        if !prone && speed < 1.0 {
            if unit(velocity).1 > 1.0 {
                out.struck = crate::game::vehicle::physics::player_push(
                    origin,
                    velocity,
                    at,
                    self.ps.velocity,
                );
                if let Some(push) = out.struck {
                    self.ps.velocity = push.velocity;
                }
                let (v, l) = unit(self.ps.velocity);
                if l < 200.0 {
                    self.ps.velocity = v * 200.0;
                }
            } else {
                self.ps.velocity = dir * 200.0;
            }
            self.ps.knockback_ms = 200.0;
            self.ps.knockback_flags |= pmove::PMF_TIME_DAMAGE;
        }
        Some(out)
    }

    /// The live and spectating arms of the spawn's own think, which
    /// `spawn_think` leaves to a caller with a world: 100 ms of pmove up to
    /// `now_ms` on [`Self::spawn_cmd`]. It is what puts the standing idle on
    /// a player's spawn frame (combat doc, 9.2). The client's own angles stay
    /// what `set_view_angle` rebases on. Returns what the move touched, for
    /// the think's `ClientImpacts`.
    pub fn spawn_move(&mut self, world: MoveWorld<'_>, cmd: &UserCmd) -> Vec<u32> {
        if self.pm_type == PmType::Intermission || self.pm_dead {
            return Vec::new();
        }
        let own = self.last_cmd_angles;
        let start = cmd.server_time.wrapping_sub(SPAWN_THINK_MS as i32);
        let pm = vcod_common::movetrace::Pm::default();
        let world = world.in_pmove(&pm);
        cmd::chop(start, cmd, |step, dt| {
            self.step(step, dt, Some(world), &[]);
            self.ps.jump_latched
        });
        self.last_cmd_angles = own;
        if self.pm_type == PmType::Normal {
            pm.touched()
        } else {
            Vec::new()
        }
    }

    /// The spawn's think on [`Self::spawn_cmd`] of `last`, in `world` when
    /// there is one; returns the cmd it ran and what its move touched. Both
    /// the `spawn` builtin's fresh sim and the frame's spawn pass run it.
    pub fn spawn_think_on(
        &mut self,
        last: &UserCmd,
        now_ms: i32,
        world: Option<MoveWorld<'_>>,
    ) -> (UserCmd, Vec<u32>) {
        let think = self.spawn_cmd(last, now_ms);
        // The think's aim block runs ahead of its `Pmove`, on the spawn's
        // own view and the memset's weapon 0, so the sway's last view is
        // the spawn's yaw when the loadout's first cmd turns off it (combat
        // doc 15.3, "The spawn's view").
        self.aim_on(self.spawn_view, SPAWN_THINK_MS as i32, now_ms, &[]);
        let touched = world.map_or_else(Vec::new, |w| self.spawn_move(w, &think));
        (think, touched)
    }

    /// The cmd the spawn's think runs, stamped `now_ms`. 1.1's is zeroed,
    /// with angles the negated `delta_angles` (`ClientSpawn` 0x42a2f..0x42a69),
    /// so the spawn frame's `viewangles` read 0. 1.5's and UO's is the
    /// client's last cmd (`trap_GetUsercmd`, 1.5b 0x3cce6, UO 0x45162), keys
    /// and stance included, at the spawn's view: a client holding prone lies
    /// down along the spawn's yaw (cod15-movement.md 4, "The spawn's
    /// lie-down").
    pub fn spawn_cmd(&self, last: &UserCmd, now_ms: i32) -> UserCmd {
        if self.ps.rules.v15() {
            UserCmd {
                server_time: now_ms,
                ..*last
            }
        } else {
            UserCmd {
                server_time: now_ms,
                angles: self.delta_angles.map(|a| a.wrapping_neg() & 0xffff),
                ..msg::NULL_USERCMD
            }
        }
    }

    /// `ClientEndFrame`'s contents write, once per frame before `end_frame`.
    pub fn update_contents(&mut self) {
        self.contents = if self.pm_type == PmType::Normal && !self.hidden() {
            CONTENTS_BODY
        } else {
            0
        };
    }

    /// The death edge. `player_die` writes `CONTENTS_CORPSE`, outside every
    /// mover's mask, so the body stops blocking before the next end frame
    /// zeroes it.
    pub fn die(&mut self) {
        if !self.dead {
            self.dead = true;
            self.contents = CONTENTS_CORPSE;
            // `ps.viewangles` takes the entity's angles (combat doc 5.1 step
            // 12), which every cmd leaves (0, view yaw, 0): the corpse's
            // view loses its pitch, a seated rider's too.
            self.view_angles = [0.0, self.view_angles[1], 0.0];
            self.ps.pitch = 0.0;
            self.died_box = true;
            // `player_die`'s relink (combat doc 5.1 step 13): `solid` 0
            // until the next link, even for a body whose session stays
            // `playing`.
            self.relink();
        }
    }

    /// `SV_LinkEntity`'s `solid`, off the box and contents at the link.
    fn relink(&mut self) {
        self.linked_unsnapped = false;
        self.linked_solid = if self.contents & (CONTENTS_BODY | 1) != 0 {
            Body::pack_solid(self.ps.mins(), self.ps.maxs())
        } else {
            0
        };
    }

    /// What the other movers clip against, `None` while the contents are 0.
    pub fn body(&self, slot: u32) -> Option<Body> {
        (self.contents != 0).then(|| Body {
            entity: slot,
            origin: self.ps.origin,
            mins: self.ps.mins(),
            maxs: self.ps.maxs(),
            contents: self.contents,
        })
    }

    /// The wire word for any mode: the base, the per-spawn teleport bit, the
    /// mounted-gun bits and the gun's firing bit. The stance bits ride on a live player's
    /// playerstate copy only, which is where the motion capture measured
    /// them.
    fn eflags(&self) -> i32 {
        use crate::game::turret::TurretStance;
        let mounted = match self.mounted_on {
            None => 0,
            Some((_, TurretStance::Stand)) => EF_MOUNTED_STAND,
            Some((_, TurretStance::Duck)) => EF_MOUNTED_DUCK,
            Some((_, TurretStance::Prone)) => EF_MOUNTED_PRONE,
        };
        PLAYER_EFLAGS
            | mounted
            | if self.teleport_bit {
                EF_TELEPORT_BIT
            } else {
                0
            }
            | if self.firing || self.attacking {
                EF_FIRING
            } else {
                0
            }
            | if self.talking { EF_TALK } else { 0 }
            | if self.ping { EF_PING } else { 0 }
            | if self.voted {
                crate::game::vote::EF_VOTED
            } else {
                0
            }
            | if self.friend_ping { EF_FRIEND_PING } else { 0 }
            | match self.seat {
                Some(s) if s.hull_view => {
                    vcod_common::net::flags::EF_IN_VEHICLE_V22
                        | vcod_common::net::flags::EF_HULL_VIEW_V22
                }
                Some(_) => vcod_common::net::flags::EF_IN_VEHICLE_V22,
                None => 0,
            }
    }

    /// `VEH_LinkPlayer`'s player half (game.mp.uo 0x8998d): `SetClientOrigin`
    /// at the seat tag (+1 z, the teleport bit), `SetClientViewAngle` at the
    /// view tag, the seat bits on, the view locked on the vehicle, the
    /// sight flag off.
    pub fn enter_seat(&mut self, seat: Seat, origin: [f32; 3], view: [f32; 3]) {
        self.ps.origin = Vec3::from(origin) + Vec3::Z;
        self.teleport_bit = !self.teleport_bit;
        // `SetClientOrigin` copies the entity ahead of the seat bits.
        self.write_entity();
        self.linked_unsnapped = true;
        self.set_view_angle(view);
        self.seat = Some(seat);
        self.unseated = false;
        self.veh_type = seat.kind;
        self.veh_motion = seat.motion;
        self.viewlocked_ent = seat.vehicle;
        self.ps.ads_active = false;
    }

    /// `G_RunClient`'s step down (game.mp.uo 0x41f39): an unlinked
    /// client's `pm_type` 1 or 7 drops to 0 or 6 on its entity-pass turn.
    pub fn run_client(&mut self) {
        self.unseated = false;
    }

    /// `VEH_UnlinkPlayer`'s (0x89fb2): the seat bits off, the view free,
    /// the sight dropped; a voluntary exit lands at `origin` (+1 z) and may
    /// pop up at `velocity`.
    pub fn leave_seat(&mut self, origin: Option<[f32; 3]>, velocity: Option<[f32; 3]>) {
        self.seat = None;
        self.seat_angles = None;
        self.ps.seat = None;
        self.unseated = true;
        self.viewlocked_ent = vcod_common::net::protocol::ENTITYNUM_NONE;
        self.ps.ads_active = false;
        self.ps.weapon_pos_frac = 0.0;
        if let Some(o) = origin {
            self.ps.origin = Vec3::from(o) + Vec3::Z;
            self.teleport_bit = !self.teleport_bit;
            // `SetClientOrigin`, past the seat bits' clear.
            self.write_entity();
            self.linked_unsnapped = true;
        }
        if let Some(v) = velocity {
            self.ps.velocity = Vec3::from(v);
        }
    }

    /// `ps.eFlags`: [`Self::eflags`] and, on a live player's word only, the
    /// stance bits; a spectator and the intermission camera carry the base
    /// and the teleport bit alone.
    fn ps_eflags(&self) -> i32 {
        if self.pm_type != PmType::Normal {
            return self.eflags();
        }
        // 1.5's follow the stepped eye target (cod15-movement.md 3).
        self.eflags()
            | match self.ps.eflags_stance() {
                pmove::Stance::Stand => 0,
                pmove::Stance::Crouch => EF_CROUCH,
                pmove::Stance::Prone => EF_PRONE,
            }
    }

    /// The entity's `fTorsoHeight`, `fTorsoPitch` and `fWaistPitch`, off the
    /// same eye-leg stamp the playerstate carries.
    fn entity_prone_body(&self, command_time: i32) -> pmove::ProneBody {
        let lerp_time = if self.dead_eye {
            0
        } else {
            self.view_lerp_start.unwrap_or(0)
        };
        self.ps.entity_prone_body(lerp_time, command_time)
    }

    /// The entity's `eFlags`: `BG_PlayerStateToEntityState` copies
    /// `ps.eFlags` whole (0x2cd8b), then sets 0x1 on `pm_type > 5`
    /// (0x2cda6) and 0x200 on the sight flag, `pm_flags` 0x20 (0x2cdb6).
    /// That copy also wipes `ClientEndFrame`'s quiet-client 0x1000, so a
    /// lagging client never carries it (handshake doc, "A quiet client").
    fn entity_eflags(&self) -> i32 {
        let dead = if self.wire_pm_type() > PM_INTERMISSION {
            EF_DEAD
        } else {
            0
        };
        let ads = if self.ps.ads_active {
            vcod_common::playerpose::EF_ADS
        } else {
            0
        };
        self.ps_eflags() | dead | ads
    }

    /// `SpectatorThink`'s button half (`game.mp.i386.so` 0x3fab8) for one
    /// cmd whose buttons are `buttons`, after the previous cmd's `prev`:
    /// either edge of the sight bit ends a free follow, an attack press cycles
    /// forward and otherwise a melee press backward. `forced` is the script's
    /// `spectatorclient`; a forced follow neither ends nor cycles here.
    /// `no_spectate` is 1.5's `allowSpectateTeam` mask: without free flight
    /// the sight edge keeps the follow and the spectator does not fly
    /// (1.5 game.mp 0x389eb).
    #[allow(clippy::too_many_arguments)]
    pub fn spectator_think(
        &mut self,
        forced: i32,
        no_spectate: u8,
        prev: u8,
        buttons: u8,
        max_clients: usize,
        followable: impl Fn(usize) -> bool,
        collision: Option<&vcod_common::collision::CollisionWorld>,
    ) {
        let freelook = crate::follow::can_spectate(no_spectate, crate::follow::FREELOOK);
        self.ps.speed = if freelook {
            pmove::SPEED_SPECTATOR
        } else {
            0.0
        };
        if forced < 0
            && freelook
            && self.follow.target.is_some()
            && (buttons ^ prev) & msg::BUTTON_ADS != 0
        {
            self.stop_following(collision);
        }
        let pressed = |bit: u8| buttons & bit != 0 && prev & bit == 0;
        let dir = if pressed(msg::BUTTON_ATTACK) {
            1
        } else if pressed(msg::BUTTON_MELEE) {
            -1
        } else {
            return;
        };
        if forced < 0
            && let Some(t) = crate::follow::cycle(self.follow.target, dir, max_clients, followable)
        {
            self.follow.target = Some(t);
        }
    }

    /// `StopFollowing` (0x46a28): the follow is dropped, and a spectator
    /// whose last frame was a copy is left behind and above the followed
    /// eye, looking where it looked pitched down 15 (`follow::stop_spot`),
    /// with the copy's velocity and the rest of the copy under its own
    /// fields ([`SPECTATOR_OWNED`]).
    pub fn stop_following(&mut self, collision: Option<&vcod_common::collision::CollisionWorld>) {
        if let (true, Some(c)) = (self.follow.on, self.follow.copied) {
            let (spot, angles) = crate::follow::stop_spot(collision, c.eye, c.angles);
            self.ps.origin = spot.into();
            self.ps.velocity = c.velocity.into();
            self.teleport_bit = c.teleport_bit;
            self.set_view_angle(angles);
            self.residue = self
                .follow_wire
                .take()
                .map(|wire| Residue { wire, moved: false });
        }
        self.follow = Default::default();
    }

    /// `setOrigin` on a player: the origin moves and the teleport bit flips,
    /// so a client snaps rather than smearing across the gap; velocity and
    /// the rest of the playerstate stay.
    pub fn teleport(&mut self, origin: [f32; 3]) {
        self.ps.origin = origin.into();
        self.teleport_bit = !self.teleport_bit;
        self.linked_unsnapped = true;
    }

    /// `G_TryPushingEntity`'s or `G_MoverTeam`'s write of `ps.origin`, which
    /// it links at unsnapped (docs/research/cod11-movers.md, section 12).
    pub fn pushed_to(&mut self, origin: Vec3) {
        self.ps.origin = origin;
        self.linked_unsnapped = true;
    }

    /// `G_SetFixedLink`'s write of a seated UO rider's `ps.origin` and
    /// `r.currentOrigin` in `G_RunClient`, which relinks there unsnapped
    /// (coduo-vehicles.md 13, "The tank driver's link offset").
    pub fn seat_linked_at(&mut self, origin: Vec3) {
        self.ps.origin = origin;
        self.linked_unsnapped = true;
    }

    /// `SetClientViewAngle` (0x41e30): the view becomes `angles` (degrees,
    /// wire convention), held to the prone cone first on a prone player
    /// (`pmove::prone_view_angle`), and `delta_angles` is rewritten so the
    /// last cmd's angles land on it.
    pub fn set_view_angle(&mut self, angles: [f32; 3]) {
        let angles = vcod_common::pmove::prone_view_angle(&self.ps, angles);
        for (i, a) in angles.iter().enumerate() {
            self.delta_angles[i] = vcod_common::pmove::angle_shorts(*a) - self.last_cmd_angles[i];
        }
        self.view_angles = angles;
        self.ps.yaw = angles[1].to_radians();
        self.ps.pitch = -angles[0].to_radians();
    }

    /// `TeleportPlayer` (0x51380, turrets doc 8 and 12.7). A playing client
    /// raises `EV_PLAYER_TELEPORT_OUT` at the old origin and `_IN` at the
    /// destination, returned for the caller to queue since the host owns the
    /// temp entities; `client_num` is this client's slot, which both carry.
    pub fn teleport_player(
        &mut self,
        client_num: usize,
        origin: [f32; 3],
        angles: [f32; 3],
    ) -> Vec<crate::game::temp_entity::TempEntity> {
        use crate::game::temp_entity::{Scope, TempEntity};
        let temp = |event: i32, at: [f32; 3]| TempEntity {
            event,
            parm: 0,
            surf_type: 0,
            other: 0,
            attacker: 0,
            weapon: 0,
            client_num: client_num as i32,
            scale: 0,
            origin: at,
            scope: Scope::Pvs,
            quake: None,
            origin2: None,
        };
        let temps = if self.pm_type == PmType::Normal && !self.dead {
            vec![
                temp(EV_PLAYER_TELEPORT_OUT, self.origin()),
                temp(EV_PLAYER_TELEPORT_IN, origin),
            ]
        } else {
            Vec::new()
        };
        self.ps.origin = Vec3::from(origin) + Vec3::Z;
        self.teleport_bit = !self.teleport_bit;
        self.linked_unsnapped = true;
        self.set_view_angle(angles);
        temps
    }

    pub fn add_event(&mut self, event: i32, parm: i32) {
        self.ring.add(event, parm);
    }

    /// `pain_debounce_time` set outright, as `ClientEvents` sets it for a
    /// fall: no `EV_PAIN` from an end frame at or before `until_ms`.
    pub fn debounce_pain(&mut self, until_ms: i32) {
        self.pain_after_ms = until_ms;
    }

    /// A cmd `ClientThink_real` drops as stale (its clock not past
    /// `commandTime`): `ClientThink` (game.mp.uo 0x51d13) has already
    /// rotated it into `pers.cmd` and the one before into `pers.oldcmd`,
    /// which the next cmd's turn spread reads (cod11-combat.md 2.1).
    pub fn stale_cmd(&mut self, cmd: &UserCmd) {
        self.last_cmd_angles = cmd.angles;
        self.ps.last_cmd_angles = [cmd.angles[0], cmd.angles[1]];
        self.ps.last_cmd_ads = cmd.buttons & vcod_common::net::msg::BUTTON_ADS != 0;
        self.ps.last_cmd_moves = [cmd.forward, cmd.right];
    }

    /// Advance one frame, returning the events the move raised, already in the
    /// ring. The axes arrive quantized to ±127/0 and dt comes off the cmd
    /// clocks. `weapons` is the map's weapon table, which only a player reads.
    pub fn step(
        &mut self,
        cmd: &UserCmd,
        dt: f32,
        world: Option<MoveWorld<'_>>,
        weapons: &[Option<WeaponDef>],
    ) -> Vec<PmEvent> {
        // `pers.cmd` takes every cmd, ahead of the dead and intermission returns.
        self.last_cmd_angles = cmd.angles;
        // `PmoveSingle` clears the bit on every cmd and sets it again ahead
        // of the move and the weapon, off the state the cmd starts from. Its
        // only `pm_type` test is against 5, so a dead body's first cmd, which
        // still holds the weapon, can set it.
        self.attacking = self.pm_type != PmType::Intermission
            && !self.respawned
            && attack_flag(&self.ps, cmd.buttons, weapons);
        self.talking = self.pm_type != PmType::Intermission && cmd.buttons & msg::BUTTON_TALK != 0;
        // A dead player's view is frozen and its body falls and slides;
        // nothing it presses reaches the mover or the weapon (combat doc,
        // 1.12 and 6, the `pm_type > 5` returns).
        if self.pm_dead {
            self.died_box = false;
            // `pm_type` 7 takes the linked arm (`cmd::dead_step`).
            self.ps.linked = self.link_to.is_some() || self.seat.is_some() || self.unseated;
            if let Some(w) = world {
                // A corpse's landing event; nothing in it is `ClientEvents`'.
                cmd::dead_step(
                    &mut self.ps,
                    &mut self.ring,
                    cmd,
                    dt,
                    &w,
                    weapons,
                    self.respawned,
                );
                self.dead_eye = true;
            } else if !self.respawned {
                // `PM_Weapon`'s `pm_type > 5` arm (`cmd::dead_step`).
                self.ps.weapon = 0;
            }
            self.relink();
            return Vec::new();
        }
        // `ClientThink_real`'s `sessionstate` 3 arm jumps to the function's
        // exit past the view angles and the mover alike (map-cycle doc,
        // 6.2), so the camera holds the origin and the view the spawn gave
        // it however the client leans on its keyboard.
        if self.pm_type == PmType::Intermission {
            return Vec::new();
        }
        self.died_box = false;
        match (self.pm_type, world) {
            // Taken by the early return above.
            (PmType::Intermission, _) => {}
            // `(Normal, None)` is the two cases where a player has no world:
            // a unit test that mounts no map, and a server whose world failed
            // to load, which `Server::FALLBACK_SPAWN` keeps running. Both fly
            // as a spectator does, so a player on a failed load noclips.
            (PmType::Spectator, _) | (PmType::Normal, None) => {
                if let Some(r) = &mut self.residue {
                    r.moved = true;
                }
                let spectator = self.pm_type == PmType::Spectator;
                let w = world.filter(|_| spectator);
                self.view_angles = cmd::spectator_step(
                    &mut self.ps,
                    self.delta_angles,
                    &mut self.ring,
                    cmd,
                    dt,
                    w.as_ref(),
                    spectator,
                );
            }
            (PmType::Normal, Some(w)) => {
                self.dead_eye = false;
                // The cmds see the link the last frame's script left, so the
                // linking frame's run free and the unlinking frame's linked
                // (object-model doc, 23.2). A seat is `pm_type` 1 too, which
                // keeps its cone off the airborne rate (coduo-vehicles.md 3).
                self.ps.linked = self.link_to.is_some() || self.seat.is_some() || self.unseated;
                self.ps.seat = self.seat.map(|s| pmove::VehicleSeat {
                    kind: s.kind,
                    pos: s.pos,
                    angles: s.angles,
                });
                let out = cmd::player_step(
                    &mut self.ps,
                    &mut self.delta_angles,
                    &mut self.ring,
                    cmd,
                    dt,
                    &w,
                    weapons,
                );
                self.view_angles = out.view;
                self.view_lerp_start = (out.view_lerp_start != 0).then_some(out.view_lerp_start);
                if self.ps.jumped {
                    self.jump_time = cmd.server_time;
                }
                if let Some(t) = self.ps.takeoff {
                    let msec = (dt * 1000.0).round() as i32;
                    let at = cmd.server_time - if t.before_timers { msec } else { 0 };
                    self.jumped = Some((t, at, self.footsteps_movetype));
                }
                if self.ps.land_anim {
                    self.land_anim = Some(cmd.server_time);
                }
                self.relink();
                return out.events;
            }
        }
        self.relink();
        // A spectator's step event is in the ring already; `ClientEvents`
        // has nothing to do for one.
        Vec::new()
    }

    /// The movetype `PM_Footsteps` (1.1 0x322c8, 1.5 0x24ae8, UO 0x297e4)
    /// selects by off the state the moves left and the cmd behind them.
    ///
    /// Retail's shape, read out of the selection function at `game.mp.i386.so`
    /// 0x322c8: below 10 units/s of horizontal speed the player is idle
    /// whatever it asked for, above it the movetype is the stance crossed with
    /// the backpedal latch and the walk bit (docs/research/player-model-anim-system.md,
    /// "How retail picks the movetype").
    fn footsteps_movetype_of(&self, cmd: &UserCmd) -> vcod_common::animscript::Movetype {
        use vcod_common::animscript::Movetype;
        let linked_idle = self.ps.linked && self.ps.rules.v15();
        // Retail's compare is `xyspeed < 10.0`, so the constant itself is
        // moving.
        // What the footsteps read, before the clamp and the snap.
        let xyspeed = self.ps.footsteps_xyspeed;
        let moving = xyspeed >= ANIM_IDLE_SPEED;
        let stopping = cmd.forward == 0 && cmd.right == 0 && xyspeed <= ANIM_STOP_SPEED;
        let back = self.ps.backwards_run;
        // `pm_flags` 0x80, the ADS walk, read once at 0x32499 (1.5 adds a
        // lean, 0x24e6f; UO does not, 0x29ba4): the arms below pass the
        // `walk` family for it (player-model-anim-system.md, "Walk against
        // run").
        let walk =
            self.ps.walking || self.ps.rules == pmove::MoveRules::V1_5 && self.ps.lean != 0.0;
        // The stance crossed with the backpedal latch, every frame. CoD 1 has
        // no walk key, so retail's walk bit is never set and the `walk*`
        // blocks are unreachable; the prone arm never reads that bit at all
        // (@0x326f1), which is why prone moves through `walkprone`.
        // 1.5's and UO's `PM_Footsteps` take the stance off the stepped eye
        // target (`PM_GetEffectiveStance`, called at 1.5 0x24c46), so a
        // lie-down plays the crouch through its first leg. 1.1's (0x322c8)
        // picks the move clips off its inline effective stance
        // (0x32340..0x3239a) but the idles off `viewHeightLerpTarget` alone
        // (0x324c8, 0x3284b), so its lie-down idles crouched for the first
        // leg too (player-model-anim-system.md, "How retail picks the
        // movetype").
        let (stance, idle_stance) = match self.ps.rules {
            pmove::MoveRules::V1_1 => {
                let t = self.ps.view_lerp_target;
                let idle = if t == pmove::VIEW_PRONE {
                    pmove::Stance::Prone
                } else if t == pmove::VIEW_CROUCH {
                    pmove::Stance::Crouch
                } else {
                    pmove::Stance::Stand
                };
                (self.ps.effective_stance(), idle)
            }
            pmove::MoveRules::V1_5 | pmove::MoveRules::Uo => {
                let s = self.ps.effective_stance();
                (s, s)
            }
        };
        let idle = match idle_stance {
            pmove::Stance::Prone => Movetype::IdleProne,
            pmove::Stance::Crouch => Movetype::IdleCr,
            pmove::Stance::Stand => Movetype::Idle,
        };
        let movetype = match (stance, moving, back) {
            // Retail's 0x322c8 takes the stance straight off `eFlags & 0xC000`
            // for a mounted player, ahead of both the ladder flag and the
            // speed test: a stand gun always plays `idle` however the
            // gunner's residual velocity from before the mount reads
            // (turrets doc, section 10).
            // 1.1's mounted arm reads `pm_flags` 0x1 and 0x2 (0x3230a).
            _ if self.mounted_on.is_some() || linked_idle => match self.ps.rules {
                pmove::MoveRules::V1_1 => match self.ps.stance {
                    pmove::Stance::Prone => Movetype::IdleProne,
                    pmove::Stance::Crouch => Movetype::IdleCr,
                    pmove::Stance::Stand => Movetype::Idle,
                },
                _ => idle,
            },
            // A climber is off the ground and still selects: retail's ladder
            // flag bypasses the airborne early-out (@0x323af).
            _ if self.ps.on_ladder && self.ps.velocity.z >= 0.0 => Movetype::ClimbUp,
            _ if self.ps.on_ladder => Movetype::ClimbDown,
            (_, false, _) => idle,
            // Retail selects the move clip and then, with no move key and
            // the body slowing under 120, the idle over it; standing takes
            // that idle only when the move clip found no clause.
            _ if stopping && idle != Movetype::Idle => idle,
            (pmove::Stance::Prone, true, true) => Movetype::WalkProneBk,
            (pmove::Stance::Prone, true, false) => Movetype::WalkProne,
            (pmove::Stance::Crouch, true, true) if walk => Movetype::WalkCrBk,
            (pmove::Stance::Crouch, true, false) if walk => Movetype::WalkCr,
            (pmove::Stance::Crouch, true, true) => Movetype::RunCrBk,
            (pmove::Stance::Crouch, true, false) => Movetype::RunCr,
            (pmove::Stance::Stand, true, true) if walk => Movetype::WalkBk,
            (pmove::Stance::Stand, true, false) if walk => Movetype::Walk,
            (pmove::Stance::Stand, true, true) => Movetype::RunBk,
            (pmove::Stance::Stand, true, false) => Movetype::Run,
        };
        // A rider's legs come off its seat (`BG_AnimUpdatePlayerStateConditions`
        // game.mp.uo 0x1c973): it stands idle in it.
        if self.seat.is_some() {
            Movetype::Idle
        } else {
            movetype
        }
    }

    /// Whether the moves just run reach `PM_Footsteps`' selection, which
    /// writes the `movetype` condition: off the ground it returns first
    /// unless climbing, mounted or seated, and 1.1's linked arm never calls
    /// it (`update_anims`). 1.5's and UO's also go on for an airborne body
    /// whose eye target is not its flags' stance (1.5 0x24e51), so a
    /// lie-down pressed mid-fall selects on its first cmd.
    fn footsteps_select(&self) -> bool {
        let linked = self.ps.linked;
        let v15 = self.ps.rules.v15();
        linked && v15
            || !linked
                && (self.ps.on_ground
                    || self.ps.on_ladder
                    || self.mounted_on.is_some()
                    || self.seat.is_some()
                    || v15 && self.ps.effective_stance() != self.ps.stance)
    }

    /// After one pmove of `cmd`: the `movetype` condition its `PM_Footsteps`
    /// left. Returns the movetype a weapon event the same pmove raised
    /// selects by: 1.1's `PmoveSingle` calls `PM_Weapon` (0x34331) ahead of
    /// `PM_Footsteps` (0x34336), so its events read the last pmove's; 1.5's
    /// main arm calls the footsteps first (0x28889, then `PM_Weapon` at
    /// 0x2888e), and so does UO's (0x2e04e, 0x2e053).
    pub fn note_footsteps(&mut self, cmd: &UserCmd) -> vcod_common::animscript::Movetype {
        let before = self.footsteps_movetype;
        if self.pm_type == PmType::Normal && !self.dead && self.footsteps_select() {
            self.footsteps_movetype = self.footsteps_movetype_of(cmd);
        }
        if self.ps.prone_flip_clears_torso {
            self.torso_cmds
                .push((cmd.server_time, TorsoCmd::ProneFlipClear));
        }
        if self.ps.breakdown_continue {
            self.torso_cmds.push((
                cmd.server_time,
                TorsoCmd::BreakdownContinue(self.footsteps_movetype),
            ));
        }
        match self.ps.rules {
            pmove::MoveRules::V1_1 => before,
            _ => self.footsteps_movetype,
        }
    }

    /// Picks this frame's animation from the state the moves just produced.
    /// Called once per frame after the moves. Off the ground nothing is
    /// selected at all. The usercmd reaches the selection only through two
    /// latches, `pm_flags` 0x40 in pmove and the `strafing` condition here
    /// ([`Self::footsteps_movetype_of`]). Each weapon event carries the
    /// movetype its pmove selected it by ([`Self::note_footsteps`]).
    ///
    /// A spectator animates nothing; it is sent to nobody and its own
    /// playerstate carries zeros in the capture.
    /// `hold_ms` is the clock the anim timers run on, the newest cmd's
    /// `serverTime`, and each event carries the `serverTime` of the cmd
    /// that raised it: retail sets and runs down `torsoTimer` and
    /// `legsTimer` inside each cmd's pmove (`BG_PlayAnim`, 1.1d 0x3c338).
    pub fn update_anims(
        &mut self,
        inputs: &AnimInputs,
        cmd: &UserCmd,
        (now_ms, hold_ms): (i32, i32),
        events: &[AnimEvent],
        rng: &mut u64,
    ) {
        use vcod_common::animscript::{Conditions, Side};
        // A dead body keeps the death anim `take_damage` chose: retail's
        // selection returns on `pm_type > 5`, and only the corpse clone reads
        // the channels after that.
        if self.pm_type != PmType::Normal || self.dead {
            return;
        }
        // The linked arm never calls the selection (0x322c8), so a linked
        // player's legs, strafe and ground edges hold; its weapon events
        // still play (object-model doc, 23.2). 1.5 and UO select the
        // stance's idle for one, airborne or not (`PM_Footsteps` 1.5
        // 0x24ae8, UO 0x297e4: `pm_type == 1` skips the airborne return and
        // takes the idle arm), which is how a rider leaves a seat standing
        // (coduo-vehicles.md 11.2).
        let linked = self.ps.linked;
        // Retail's condition 8 (@0x32504): any forward component clears the
        // strafe, diagonals included; a cmd that is sideways only sets the
        // side; a cmd asking for neither leaves the condition as it was.
        if linked {
            // held
        } else if cmd.forward != 0 {
            self.strafing = None;
        } else if cmd.right != 0 {
            self.strafing = Some(if cmd.right < 0 {
                Side::Left
            } else {
                Side::Right
            });
        }
        let movetype = self.footsteps_movetype_of(cmd);
        self.movetype = movetype;
        let (seat_mounted, vehicle, vehicle_motion) = seat_conditions(self.seat);
        let conditions = Conditions {
            movetype,
            weapon: inputs.condition_weapon(),
            weapon_class: inputs.weapon_class.to_ascii_lowercase(),
            // The gated flag, not the raw bit: retail feeds
            // `BG_UpdateConditionValue` slot 7 `pm_flags & 0x20`
            // (combat doc, 1.13).
            ads: self.ps.ads_active,
            strafing: self.strafing,
            mounted: seat_mounted.or_else(|| self.mounted_on.map(|_| "mg42".to_string())),
            vehicle,
            vehicle_motion,
            // The turret never runs the weapon state machine, so a mounted
            // gunner's `firing` clause reads the cmd's raw attack bit instead
            // of `weaponstate` (turrets doc 10, condition slot 0x2a454's
            // neighbour).
            firing: if self.mounted_on.is_some() || self.seat.is_some() {
                cmd.buttons & msg::BUTTON_ATTACK != 0
            } else {
                self.ps.weaponstate == vcod_common::pmove::weapon::WEAPON_FIRING
            },
        };
        let resolve = |name: &str| inputs.anims.wire_of(name);
        let length = |name: &str| inputs.anims.length_ms(name);
        // The two ground edges, before the continuous state: an event anim
        // holds the channel, so raising it first is what keeps the restart
        // toggle flipping once per landing rather than twice.
        //
        // The takeoff is raised by pmove ([`pmove::Takeoff`]: the jump, the
        // kickoff off the ground, a step off a drop past 64) and not by
        // becoming airborne: retail's own mp_pavlov capture backs off a
        // ledge at `run_back` and reads the run loop while airborne, so a
        // short fall and a mounted ladder animate whatever they were doing.
        let jumped = std::mem::take(&mut self.jumped);
        let land_anim = std::mem::take(&mut self.land_anim);
        let script = &inputs.anims.script;
        match (self.ps.on_ground, self.was_airborne) {
            _ if linked => {}
            // `PM_CrashLand` raises the land anim only on a fast landing and
            // only with `legsTimer` 0 (cod11-sound-system.md, "Landing"): the
            // one-unit drop off a turret release plays none.
            (true, true)
                if let Some(at) = land_anim
                    && !self.anim.legs_held(at) =>
            {
                // The landing writes the legs alone, `both` clause or not
                // (combat doc, 1.14).
                let mut sel = script.select_event("land", &conditions);
                sel.torso = None;
                Self::play_event(&mut self.anim, &sel, (now_ms, at), resolve, length);
            }
            (_, _)
                if let Some((t, at, movetype)) = jumped
                    && (t.forced || !self.anim.legs_held(at)) =>
            {
                let event = if t.back { "jumpbk" } else { "jump" };
                let conditions = Conditions {
                    movetype,
                    ..conditions.clone()
                };
                let sel = script.select_event(event, &conditions);
                Self::play_event(&mut self.anim, &sel, (now_ms, at), resolve, length);
            }
            _ => {}
        }
        // The weapon channel: retail's `PM_Weapon` raises a script event on
        // entering a state, and the `EVENTS` block's own clauses pick the
        // torso index by stance, weapon and class
        // (docs/research/player-model-anim-system.md, "The weapon channel").
        // A cmd's prone flip clear and breakdown continue run ahead of its
        // weapon events, and every cmd's ahead of the next one's.
        let mut torso_cmds = std::mem::take(&mut self.torso_cmds).into_iter().peekable();
        for &(e, at, movetype) in events {
            while let Some((t, c)) = torso_cmds.next_if(|&(t, _)| t <= at) {
                self.torso_cmd(c, (now_ms, t), &conditions, inputs);
            }
            let Some(name) = weapon_anim_event(e.event) else {
                continue;
            };
            let conditions = Conditions {
                movetype,
                ..conditions.clone()
            };
            // `meleeattack` is the one weapon clause that lists several anims
            // per channel, and retail draws among them (animscript.rs).
            let sel = if name == "meleeattack" {
                script.select_event_random(name, &conditions, rng)
            } else {
                script.select_event(name, &conditions)
            };
            // The weapon change's two events are raised unforced (1.1d
            // game.mp 0x3c338's callers, UO's), every other one forced.
            if matches!(name, "dropweapon" | "raiseweapon") {
                self.anim.event_unforced(&sel, now_ms, at, resolve, length);
            } else {
                Self::play_event(&mut self.anim, &sel, (now_ms, at), resolve, length);
            }
        }
        for (t, c) in torso_cmds {
            self.torso_cmd(c, (now_ms, t), &conditions, inputs);
        }
        // Nothing is selected while off the ground -- retail returns before
        // the selection unless the ladder flag is set (@0x323a2), which is
        // what gives a climber its `climbup`/`climbdown` -- so a jump owns
        // the legs until the landing. A mounted player is also off the
        // ground (`ps.mounted` clears `on_ground` in pmove) but the 0x322c8
        // mounted arm runs ahead of that early-out (turrets doc, section 10).
        let mut continuous_torso = false;
        if self.footsteps_select() {
            let mut sel = script.select("combat", &conditions);
            // Retail leaves `torsoAnim` 0 in every settled pose of both
            // captures, although the clauses reached here are `both`. That 0
            // is a write: it is what a weapon event's anim gives the channel
            // back to when it runs out. A `torso` line is written as itself:
            // UO's jeep passenger reads `c_mp_torso_stand_alert` (coduo-vehicles.md 13).
            sel.torso = sel.torso.filter(|a| !a.both);
            continuous_torso = sel.torso.is_some();
            self.anim.set(&sel, now_ms, hold_ms, resolve);
        }
        // Outside the airborne early-out: a shot fired in the air would
        // otherwise hold its torso until the landing.
        if !continuous_torso {
            self.anim.clear_torso(now_ms, hold_ms);
        }
        if !linked {
            self.was_airborne = !self.ps.on_ground;
        }
    }

    /// One event clause on the two channels. A `both` clause is the whole
    /// body: retail puts the anim on the legs and restarts the torso on no
    /// anim at all, which is the same 0 every settled pose reads. The
    /// capture's grenade throws are the evidence -- `legsAnim` 575, index 63,
    /// with a bare toggle flip on the torso.
    ///
    /// The rule is general and the measurement is not: it also reaches
    /// `fireweapon`'s pistol-ADS clause and `jump`'s two run clauses, neither
    /// of which any capture covers (combat doc 1.14). It is kept general
    /// because it is the convention the continuous selection already follows.
    /// The landing is the one clause measured to break it -- its `weaponclass
    /// pistol AND grenade` arm is a `both` one and writes the legs alone --
    /// and its caller clears the torso of the selection before it gets here.
    /// One [`TorsoCmd`] at `at` on the hold clock. The breakdown continue
    /// restarts the torso only where its clause's anim differs from the one
    /// playing; on the same anim it sets the timer again when the clip
    /// loops (`BG_PlayAnim`'s flag 0x80, UO game.mp 0x1c122; no stock
    /// `LMG_BREAKDOWN` clause reaches it, player-model-anim-system.md,
    /// "The takeoff and the landing").
    fn torso_cmd(
        &mut self,
        c: TorsoCmd,
        (now_ms, at): (i32, i32),
        conditions: &vcod_common::animscript::Conditions,
        inputs: &AnimInputs,
    ) {
        let resolve = |name: &str| inputs.anims.wire_of(name);
        let length = |name: &str| inputs.anims.length_ms(name);
        match c {
            TorsoCmd::ProneFlipClear => self.anim.clear_torso_held(now_ms),
            TorsoCmd::BreakdownContinue(movetype) => {
                let conditions = vcod_common::animscript::Conditions {
                    movetype,
                    ..conditions.clone()
                };
                let sel = inputs
                    .anims
                    .script
                    .select_event("lmg_breakdown", &conditions);
                let playing = self.anim.torso() & !vcod_common::net::flags::ANIM_TOGGLEBIT;
                match sel.torso.as_ref().and_then(|a| resolve(&a.name)) {
                    Some(index) if index == playing => {
                        if inputs.anims.motion(index).is_some_and(|m| m.looping) {
                            self.anim.reset_torso_timer(&sel, at, length);
                        }
                    }
                    _ => Self::play_event(&mut self.anim, &sel, (now_ms, at), resolve, length),
                }
            }
        }
    }

    fn play_event(
        anim: &mut vcod_common::animscript::AnimState,
        sel: &vcod_common::animscript::Selection,
        (now_ms, hold_ms): (i32, i32),
        resolve: impl Fn(&str) -> Option<i32>,
        length: impl Fn(&str) -> Option<u32>,
    ) {
        if sel.legs.is_some() && sel.torso.is_some() {
            let legs_only = vcod_common::animscript::Selection {
                legs: sel.legs.clone(),
                torso: None,
            };
            anim.event(&legs_only, now_ms, hold_ms, resolve, length);
            anim.restart_torso_empty(now_ms);
            return;
        }
        anim.event(sel, now_ms, hold_ms, resolve, length);
    }

    /// The anim conditions of the moment, for an event raised outside the
    /// frame's own selection: the stance, and the weapon the inputs name.
    fn event_conditions(&self, inputs: &AnimInputs) -> vcod_common::animscript::Conditions {
        use vcod_common::animscript::{Conditions, Movetype};
        let moving = self.ps.velocity.truncate().length() >= ANIM_IDLE_SPEED;
        let movetype = match (self.ps.stance, moving, self.ps.backwards_run) {
            (pmove::Stance::Prone, _, _) => Movetype::IdleProne,
            (pmove::Stance::Crouch, false, _) => Movetype::IdleCr,
            (pmove::Stance::Crouch, true, _) => Movetype::RunCr,
            (pmove::Stance::Stand, false, _) => Movetype::Idle,
            (pmove::Stance::Stand, true, true) => Movetype::RunBk,
            (pmove::Stance::Stand, true, false) => Movetype::Run,
        };
        Conditions {
            movetype,
            weapon: inputs.condition_weapon(),
            weapon_class: inputs.weapon_class.to_ascii_lowercase(),
            ads: false,
            strafing: self.strafing,
            mounted: self.mounted_on.map(|_| "mg42".to_string()),
            firing: false,
            ..Conditions::default()
        }
    }

    /// The sim's half of `finishPlayerDamage` (combat doc, 4.5): the
    /// knockback, the frame's damage accumulated for `end_frame`, and on a
    /// killing hit what `player_die` does to the playerstate (5.1):
    /// `EV_DEATH` with parm 0, the dead yaw into `stats[1]`, the death anim.
    /// The health itself is the host's and arrives through the mirror.
    /// `rng` is the server's own draw state: the two anims here are the ones
    /// the script lists several of, and retail picks among them at random.
    /// `now_ms` is the frame clock the death clip's phase starts on,
    /// `cmd_ms` the client's last processed cmd's serverTime, which its
    /// timers count from (combat doc 16.4, "The dying body").
    pub fn take_damage(
        &mut self,
        op: &SimOp,
        anims: Option<&AnimInputs>,
        rng: &mut u64,
        (now_ms, cmd_ms): (i32, i32),
    ) {
        let SimOp::Damaged {
            damage,
            dir,
            knockback,
            dead_yaw,
            fatal,
            ..
        } = *op
        else {
            unreachable!("take_damage is the Damaged arm of the sim-op drain");
        };
        let dir = Vec3::from(dir);
        let has_dir = dir.length_squared() > 0.0;
        // Read before the knockback, which is this frame's impulse and not
        // movement: retail's conditions are the ones the last pmove left
        // (`BG_UpdateConditionValue`), so a standing player shot off his feet
        // still dies a standing death.
        let conditions = anims.map(|inputs| self.event_conditions(inputs));
        if knockback {
            let scale = if self.ps.stance == pmove::Stance::Prone {
                KNOCKBACK_PRONE
            } else if self.ps.ducked {
                KNOCKBACK_DUCKED
            } else {
                KNOCKBACK_STAND
            };
            let kb = ((damage as f32 * scale) as i32).min(KNOCKBACK_MAX);
            if kb > 0 && has_dir {
                let kvel = dir * (kb as f32 * KNOCKBACK_UNITS);
                let x = self.ps.velocity.x;
                self.ps.velocity += kvel;
                // 1.1d keeps x's product on the stack into its add
                // (0x439e7..0x439fb); 1.5b and UO store it (cod11-combat.md
                // 4.5).
                if self.ps.rules == pmove::MoveRules::V1_1 {
                    let s = f64::from(kb as f32 * KNOCKBACK_UNITS);
                    self.ps.velocity.x = (f64::from(x) + f64::from(dir.x) * s) as f32;
                }
            }
            // The slide it starts, when no timer runs yet (0x43a22-0x43a4a).
            if kb > 0 && self.ps.knockback_ms == 0.0 {
                self.ps.knockback_ms = (kb * 2).clamp(50, 200) as f32;
                self.ps.knockback_flags |= pmove::PMF_TIME_DAMAGE;
            }
        }
        self.damage.taken += damage;
        self.damage.from = has_dir.then_some(dir);
        if !fatal {
            // No `pain` animscript event: no server game module raises one
            // (combat doc 16.4), and the hit frame keeps `legsAnim` 634,
            // `torsoAnim` 0 and both timers in the captures (8.4, 3.4). The
            // stock clause would put `pb_crouch_pain_holdStomach` on a
            // standing player for 1.35 s, doubling it under the next shot.
            return;
        }
        self.die();
        // The cook went with the drop: retail's `fire_grenade` clears
        // `grenadeTimeLeft` on the thrower, and the retail death frame reads
        // 0 (combat doc, 11.1 and 5.1 step 5).
        self.ps.grenade_time_left_ms = 0;
        self.add_event(EV_DEATH, 0);
        // 5.1 item 11, which the host worked out off the entities.
        self.dead_yaw = dead_yaw;
        // The death anim on the legs, and the torso restarted on 0: both
        // retail deaths read `torsoAnim` 512 beside the death `legsAnim`
        // (combat doc, 8.1 and 8.4). Nothing selects for this sim again --
        // `update_anims` returns on a dead one -- so the drawn index is what
        // the corpse clone carries.
        if let (Some(inputs), Some(c)) = (anims, &conditions) {
            let mut sel = inputs.anims.script.select_event_random("death", c, rng);
            sel.torso = None;
            let resolve = |name: &str| inputs.anims.wire_of(name);
            let length = |name: &str| inputs.anims.length_ms(name);
            self.anim.event(&sel, now_ms, cmd_ms, resolve, length);
        }
        self.anim.restart_torso_empty(now_ms);
    }

    /// UO's `viewKick` (game.mp.uo 0x6db55): `damage_blood` set to the
    /// force scaled by the max health, rounded, and `damage_from` the body's
    /// origin less `from`, which the end frame's feedback reads as a hit's.
    pub fn view_kick(&mut self, force: i32, from: [f32; 3]) {
        self.damage.taken = (force * self.max_health + 50) / 100;
        let d = self.ps.origin - Vec3::from(from);
        self.damage.from = (d.length_squared() > 0.0).then(|| d.normalize());
    }

    /// UO's `Scr_Vehicle_Pain` (game.mp.uo 0x8d1ca) on the vehicle's
    /// driver: `damage_blood` up by the vehicle's damage and `damage_from`
    /// as given, not normalised (coduo-vehicles.md 13).
    pub fn vehicle_pain(&mut self, damage: i32, from: Option<[f32; 3]>) {
        self.damage.taken += damage;
        self.damage.from = from.map(Vec3::from);
    }

    /// `P_DamageFeedback` (combat doc, section 6), once per frame after the
    /// ops: the frame's damage becomes `damageCount`, its direction the two
    /// angle bytes, `damageEvent` counts up, `EV_PAIN` carries the health
    /// left. A dead player's feedback never runs, so the killing hit leaves
    /// all four fields as the last surviving hit left them (8.4).
    pub fn end_frame(&mut self, now_ms: i32) {
        // `ClientEndFrame` writes `pm_type` (0x41079) and `ps.speed`
        // (0x41100) ahead of its `P_DamageFeedback` call (0x41128). The
        // write reads the link alone (UO 0x52f59: 1 or 7 linked, 0 or 6
        // not), so a rider the vehicle think put out runs the next frame's
        // cmds unlinked (coduo-vehicles.md 11.2).
        self.pm_dead = self.dead_arm();
        self.unseated = false;
        if self.pm_type == PmType::Normal {
            self.ps.speed = self.max_speed as f32;
            self.ps.gravity = self.gravity as f32;
        }
        // Ahead of `P_DamageFeedback`, so a hit's spread add waits a frame.
        self.spread_frac = self.ps.aim_spread_scale / 255.0;
        if self.pm_dead || self.damage.taken <= 0 || self.max_health <= 0 {
            return;
        }
        let count = (self.damage.taken * 100 / self.max_health).min(127);
        self.ps.aim_spread_scale = (self.ps.aim_spread_scale + count as f32).min(255.0);
        // Step 6's kick, split along and across the view (steps 7 and 8);
        // the aim block plays it out from step 11's stamp.
        let kick = (self.ps.aim_spread_scale * 0.2).clamp(5.0, 90.0);
        match self.damage.from {
            None => {
                self.feedback.yaw = 255;
                self.feedback.pitch = 255;
                self.kick.side = 0.0;
                self.kick.pitch = -kick;
            }
            Some(d) => {
                // Both angles in [0, 360), the pitch down positive; the
                // scale runs on the x87 stack and truncates (combat doc 6).
                let [pitch, yaw, _] = crate::game::builtins::math::vectoangles(d.into());
                self.feedback.pitch = (f64::from(pitch) / 360.0 * 256.0) as i32;
                self.feedback.yaw = (f64::from(yaw) / 360.0 * 256.0) as i32;
                let axis = vcod_common::x87::angles_to_axis(self.view_angles.into());
                self.kick.side = -kick * d.dot(axis[1]);
                self.kick.pitch = kick * d.dot(axis[0]);
            }
        }
        self.kick.time_ms = now_ms.wrapping_sub(20);
        if now_ms.wrapping_sub(self.pain_after_ms) > 0 {
            self.ring_before_pain = Some(self.ring);
            let percent = (self.health as f32 * 100.0 / self.max_health as f32) as i32;
            self.add_event(EV_PAIN, percent.clamp(0, 100));
            self.pain_after_ms = now_ms.wrapping_add(PAIN_DEBOUNCE_MS);
        }
        self.feedback.event = self.feedback.event.wrapping_add(1);
        self.feedback.count = count;
        self.damage.taken = 0;
    }

    /// The wire playerstate. Fields the two modes disagree about are measured
    /// on both sides: the spectator's from the retail capture in
    /// `crates/common/tests/fixtures/net/snapshots.bin`, whose zeros
    /// `parses_captured_snapshot_run` pins, the player's from
    /// `crates/server/tests/fixtures/playerstate/*.txt`, which the
    /// `playerstate_ab` gate diffs against. `commandTime` mirrors the last
    /// processed cmd's server time.
    /// Where the player is: the sim owns it, the script mirrors it.
    pub fn origin(&self) -> [f32; 3] {
        self.ps.origin.into()
    }

    /// `r.currentOrigin` at the last link, which `r.absmin` and `r.absmax`
    /// are built off: `ClientThink_real` links at the snapped origin, and a
    /// linked client's `G_RunClient` relinks at the anchored one, and a
    /// `setOrigin` or `TeleportPlayer` with no cmd since links unsnapped
    /// (combat doc, 14.3).
    pub fn link_origin(&self) -> Vec3 {
        if self.link_to.is_some() || self.linked_unsnapped {
            self.ps.origin
        } else {
            self.ps.origin.trunc()
        }
    }

    /// The wire `legsAnim`, restart bit included.
    pub fn legs_anim(&self) -> i32 {
        self.anim.legs()
    }

    /// This client's body as a locational trace meets it, `None` unless it
    /// is alive and playing: the link where its last cmd left it, posed the
    /// way its last end frame did (`docs/research/cod11-combat.md` 16.1).
    pub fn hit_body(&self, slot: usize) -> Option<crate::game::combat::HitBody> {
        if self.dead {
            return None;
        }
        self.dobj(slot)
    }

    /// The posed model a tag lookup reads, which a dead player still has:
    /// [`Self::hit_body`] without the death gate.
    pub fn dobj(&self, slot: usize) -> Option<crate::game::combat::HitBody> {
        if self.pm_type != PmType::Normal {
            return None;
        }
        let mut pose = self.pose.clone();
        pose.es = self.hook_es;
        pose.ci_seat_angles = self.ci_seat_angles;
        Some(crate::game::combat::HitBody {
            slot,
            origin: self.ps.origin,
            mins: self.ps.mins(),
            maxs: self.ps.maxs(),
            pose,
        })
    }

    /// `BG_PlayerStateToEntityState`'s copy of what the controllers' hook
    /// reads off the entity: whether and where he rides, and his weapon.
    /// `ClientThink_real` runs it after each cmd's move (game.mp.uo
    /// 0x40c91), `SetClientOrigin` (0x44210) and `ClientEndFrame` too
    /// (coduo-vehicles.md 13, "The weapon-swap frame").
    pub fn write_entity(&mut self) {
        self.hook_es = crate::game::combat::HookEs {
            seat: self.seat.map(|s| (s.kind, s.pos)),
            weapon: self.ps.weapon.into(),
        };
    }

    /// `G_PlayerVehiclePositionAndBlend`'s seat tag angles: into the
    /// placement and into the record (`+0x3f4`), which keeps them past an
    /// exit.
    pub fn place_seat_angles(&mut self, angles: [f32; 3]) {
        self.seat_angles = Some(angles);
        self.ci_seat_angles = angles;
    }

    /// The turret placement's leaf blend at level time `now_ms`, `None`
    /// off a `turretanim` anim: the legs pose by its leaves, and its sync
    /// group steps at the mean frequency their weights give from the next
    /// frame on (cod11-turrets.md 13.9). On 1.5 and UO the rows are the
    /// carried weights (`gunner_tree`) the placement leaves, which the pose
    /// reads at once, until the next end frame's step.
    pub fn set_gunner_blend(
        &mut self,
        placed: Option<&vcod_common::turretpose::GunnerPlacement>,
        now_ms: i32,
    ) {
        let carried = self.ps.rules.v15();
        if let Some(g) = placed
            && self
                .gunner_leaves
                .as_ref()
                .is_some_and(|(n, _)| *n != g.node)
        {
            self.gunner_left = self.gunner_leaves.take();
        }
        match placed {
            Some(g) if carried => {
                self.gunner_tree.place(g, crate::server::FRAME_MS);
                self.gunner_leaves = Some((g.node, Vec::new()));
                self.gunner_rows_from_tree();
                self.pose.turret_leaves.clone_from(&self.gunner_leaves);
                self.pose.turret_left.clone_from(&self.gunner_left);
            }
            Some(g) => self.gunner_leaves = Some((g.node, g.rows.clone())),
            None => {
                self.gunner_leaves = None;
                self.gunner_left = None;
            }
        }
        if let Some(g) = placed {
            let legs = self.anim.legs();
            self.pose
                .legs_clock
                .set_sync_frequency(legs, now_ms, g.frequency, g.looping);
        }
    }

    /// The weight step of 1.1d's `G_DObjUpdateServerTime` (0x80bb130) and
    /// of 1.5's and UO's `G_XAnimUpdate` (UO game.mp 0x69983), after the
    /// frame's thread passes and before any end frame: the goals the last
    /// placement set land on the rows, so a lower slot's round meets them
    /// before this gunner's own end frame places him again
    /// (cod11-turrets.md 13.9, 13.10). Returns the turret group and the one
    /// left, as the pose now holds them, when he has either.
    pub fn land_gunner_goals(&mut self) -> Option<TurretLegs> {
        if self.ps.rules.v15() {
            let dt = (f64::from(0.001f32) * f64::from(crate::server::FRAME_MS)) as f32;
            self.gunner_tree.step(dt);
            self.gunner_rows_from_tree();
        }
        if self.gunner_leaves.is_none() && self.gunner_left.is_none() {
            return None;
        }
        self.pose.turret_leaves.clone_from(&self.gunner_leaves);
        self.pose.turret_left.clone_from(&self.gunner_left);
        Some((self.gunner_leaves.clone(), self.gunner_left.clone()))
    }

    /// `gunner_leaves` and `gunner_left` as `gunner_tree` holds them now.
    fn gunner_rows_from_tree(&mut self) {
        let tree = &self.gunner_tree;
        for (n, rows) in self.gunner_leaves.iter_mut().chain(&mut self.gunner_left) {
            *rows = tree.rows(*n);
        }
    }

    /// The controls the last skeleton build eased to, which the record
    /// keeps for the next build's ease (coduo-vehicles.md 13).
    pub fn keep_eased(&mut self, c: vcod_common::playerpose::Controllers) {
        if self.pose.eased.is_some() {
            self.pose.eased = Some(c);
        }
    }

    /// `ClientEndFrame`'s `BG_UpdatePlayerDObj` and `BG_PlayerAnimation`:
    /// the models, the two anim indices with the phase each started at, and
    /// the record the controllers read, its swings stepped over
    /// `frametime_ms` at `bg_swingSpeed` (`docs/research/cod11-combat.md`
    /// 16.3, 16.4). `anims`
    /// gives the legs anim's record; without it no anim is a strafe one.
    pub fn commit_pose(
        &mut self,
        frame: EndFrame,
        swing_speed: f32,
        anims: Option<&vcod_common::animtree::PlayerAnims>,
    ) {
        use vcod_common::animclock::{self, ChannelClock, Seen, Times};
        use vcod_common::playerpose::{BodyInput, BodySlope, SeatWeapon};
        let EndFrame {
            frametime_ms,
            command_time,
            now_ms,
            client_num,
        } = frame;
        let legs = self.anim.legs();
        let torso = self.anim.torso();
        // The DObj's clocks, fed the entity's snapped `pos.trBase` (combat
        // doc 14.3); without the anim records every clip plays at rate 1
        // from where the channel took it.
        let (mut legs_clock, mut torso_clock) = (self.pose.legs_clock, self.pose.torso_clock);
        // The legs nodes still weighted after the frame's step: the seat
        // leaves under one at 0 stand still (`SeatRamp::fade_held`).
        let weighted: Vec<i32> = match self.seat {
            Some(_) => legs_clock.layers(now_ms).iter().map(|l| l.index).collect(),
            None => Vec::new(),
        };
        let ahead = if std::mem::take(&mut self.dobj_ahead) {
            frametime_ms
        } else {
            0
        };
        match anims {
            Some(a) => {
                let seen = Seen {
                    client_num,
                    tr_base: self.ps.origin.trunc(),
                    eflags: self.entity_eflags(),
                };
                let at = Times {
                    clock_ms: now_ms.wrapping_sub(ahead),
                    snapshot_ms: now_ms,
                };
                let window = (self.ps.rules != vcod_common::pmove::MoveRules::V1_1)
                    .then_some(&mut self.stance_window_ms);
                let clocks = (&mut legs_clock, &mut torso_clock);
                animclock::update_body(clocks, a, (legs, torso), &seen, at, window);
            }
            None => {
                legs_clock = ChannelClock::started(legs, self.anim.legs_start_ms());
                torso_clock = ChannelClock::started(torso, self.anim.torso_start_ms());
            }
        }
        let body = self.entity_prone_body(command_time);
        let input = BodyInput {
            view: self.view_angles,
            movement_dir: self.ps.movement_dir as f32,
            eflags: self.entity_eflags(),
            legs: anims.map(|a| a.record(legs)).unwrap_or_default(),
            seat_weapon: self.seat.map_or(SeatWeapon::Barred, |s| {
                SeatWeapon::of(s.kind, s.pos, self.ps.weapon.into(), self.ci_seat_angles[1])
            }),
        };
        let mut angles = self.pose.angles;
        // Pmove writes the movetype condition each cmd, ahead of the end
        // frame's updater; the anim's own record lands after it.
        angles.movetype = self.movetype.bit();
        angles.step(&input, frametime_ms, swing_speed);
        angles.update_conditions(&input);
        let (eased, mut seat_ramp) = (self.pose.eased, self.pose.seat_ramp.clone());
        self.pose = crate::game::combat::BodyPose {
            assembly: self.assembly.clone(),
            legs,
            torso,
            legs_clock,
            torso_clock,
            input,
            angles,
            // The entity's copy, which the controllers read (combat doc 16.4).
            slope: BodySlope {
                lean: self.ps.lean,
                torso_height: body.torso_height,
                torso_pitch: body.torso_pitch,
                waist_pitch: body.waist_pitch,
            },
            turret_leaves: self.gunner_leaves.clone(),
            turret_left: self.gunner_left.clone(),
            seat: self.seat.zip(self.seat_angles).map(|(s, a)| {
                vcod_common::playerpose::BodySeat::new(a, s.kind, s.pos, self.seat_steer)
                    .armed(self.ps.weapon.into())
            }),
            rules: self.ps.rules,
            eased: None,
            seat_ramp: Default::default(),
            ease_ms: frametime_ms,
            // `dobj` fills both in as the entity stands when it is asked.
            es: Default::default(),
            ci_seat_angles: [0.0; 3],
        };
        self.write_entity();
        // The end frame only sets the targets: the hook eases toward them
        // on each skeleton build that follows (`combat::DobjStamps`;
        // docs/research/coduo-vehicles.md 14, "The controllers").
        if vcod_common::playerpose::Controllers::eased(self.ps.rules) {
            self.pose.eased = Some(eased.unwrap_or_default());
        }
        if let Some(a) = anims {
            let goal = self
                .pose
                .seat
                .as_ref()
                .and_then(|seat| vcod_common::playerpose::seat_inputs(a, legs, seat));
            seat_ramp.fade_held(|node| weighted.iter().any(|&i| a.index.node_id(i) == Some(node)));
            seat_ramp.set(goal, now_ms, |leaf| a.leaf_motion(leaf));
            self.pose.seat_ramp = seat_ramp;
        }
    }

    /// `ps.delta_angles`, what a bot's absolute cmd angles must subtract to
    /// aim right after a spawn.
    pub fn delta_angles(&self) -> [i32; 3] {
        self.delta_angles
    }

    /// A rotating pusher turns its riders' view with it: `G_TryPushingEntity`
    /// adds the yaw it moved, in short units, to `delta_angles[1]`
    /// (docs/research/cod11-movers.md, section 12).
    pub fn turn_delta_yaw(&mut self, short: i32) {
        self.set_delta_yaw(self.delta_angles[1] + short);
    }

    /// `delta_angles[1]` as a blocked push found it.
    pub fn set_delta_yaw(&mut self, short: i32) {
        self.delta_angles[1] = short & 0xffff;
    }

    /// `ps.viewangles`, degrees, wire convention (pitch positive down).
    pub fn view_angles(&self) -> [f32; 3] {
        self.view_angles
    }

    /// `ClientThink_real`'s aim block, once per cmd ahead of its moves
    /// (`pmove::aim`): reads the view the previous cmd left and the state
    /// the moves have not yet touched, the way retail runs it before
    /// `Pmove`. `msec` is the cmd's whole length, which retail caps at 200.
    pub fn update_aim(&mut self, msec: i32, now_ms: i32, weapons: &[Option<WeaponDef>]) {
        self.aim_on(self.view_angles, msec, now_ms, weapons);
    }

    /// [`Self::update_aim`] on `view` for `ps.viewangles`.
    fn aim_on(&mut self, view: [f32; 3], msec: i32, now_ms: i32, weapons: &[Option<WeaponDef>]) {
        if self.pm_type != PmType::Normal || self.dead {
            return;
        }
        let def = weapons
            .get(self.ps.weapon as usize)
            .and_then(Option::as_ref);
        let msec = msec.min(200);
        if let Some(def) = def {
            self.gun_kick
                .step(def, self.ps.weapon_pos_frac, msec as f32 * 0.001);
        }
        let input = pmove::aim::AimInput {
            def,
            view,
            msec,
            now_ms,
            kick: self.kick,
            gun_kick: self.gun_kick.angles,
            sway_scale: 1.0,
        };
        self.aim = pmove::aim::aim_angles(&self.ps, &mut self.aim_state, &input);
        let wrap = |d: f32| (d + 180.0).rem_euclid(360.0) - 180.0;
        self.gun_offset = [wrap(self.aim[0] - view[0]), wrap(self.aim[1] - view[1])];
    }

    /// `G_PlayerEvent`'s fire arm on a 1.5 or UO server (1.5b 0x3b2e4, UO
    /// 0x435cc): `BG_WeaponFireRecoil` (1.5b 0x3246c, UO 0x396bc) draws the
    /// view kick, which the server drops, then the gun kick, which it adds
    /// to the spring's speeds: four `rand()`s, `rand` a draw in
    /// `0..=0x7fff_ffff`. The ranges are the sight's when the fraction is
    /// above 0 (cod15-movement.md 6). Nothing on 1.1.
    pub fn fire_recoil(&mut self, weapons: &[Option<WeaponDef>], mut rand: impl FnMut() -> i32) {
        if !self.ps.rules.v15() {
            return;
        }
        let Some(def) = weapons
            .get(self.ps.weapon as usize)
            .and_then(Option::as_ref)
        else {
            return;
        };
        let mut draw = |[min, max]: [f32; 2]| min + (max - min) * (rand() as f32 / 2_147_483_648.0);
        let a = &def.aim;
        let view = usize::from(self.ps.weapon_pos_frac == 1.0);
        draw(a.view_kick_pitch[view]);
        draw(a.view_kick_yaw[view]);
        let gun = usize::from(self.ps.weapon_pos_frac > 0.0);
        let pitch = draw(a.gun_kick_pitch[gun]);
        let yaw = draw(a.gun_kick_yaw[gun]);
        self.gun_kick.kick(pitch, yaw);
    }

    /// How far the last cmd's shot left off the view it was aimed from,
    /// (pitch, yaw) degrees: the sight sway, the turn lag and the kicks
    /// (combat doc 15). The bots aim it the way a player aims the reticle.
    pub fn gun_offset(&self) -> [f32; 2] {
        self.gun_offset
    }

    /// Where the next shot, swing or throw goes: `client+0x220c/0x2210`,
    /// degrees in the wire convention, as the last `update_aim` left it.
    pub fn aim_angles(&self) -> [f32; 2] {
        self.aim
    }

    /// The point a snapshot is built from: the eye. `SV_BuildClientSnapshot`
    /// adds the playerstate's view height to `origin[2]` (0x808f288) and
    /// `AddLeanToPosition` with the lean (0x808f2d4) before it looks the leaf
    /// up, so a client is tested from where it sees.
    pub fn eye_origin(&self) -> [f32; 3] {
        self.ps.view().eye.into()
    }

    /// What other clients are sent about this one. Retail sends a client no
    /// entity for itself -- the playerstate carries it -- so this is only ever
    /// built for somebody else (`docs/protocol-1.1.md`, "Which entities a
    /// client is sent"). The body model is not here either: it rides the
    /// `clientState` roster's `modelindex`, which stage 4 already sends.
    ///
    /// Measured against a retail capture of one probe watching another
    /// (`crates/server/tests/fixtures/entities/mp_carentan-dm-players.txt`).
    /// A moving player travels as a trajectory, not as a point: `pos.trType`
    /// 3 with a delta and a 50 ms duration, so the receiving client carries
    /// the motion between snapshots instead of stepping to each one.
    ///
    /// The event ring rides here as well as on the playerstate: it is how
    /// another client hears this one's shots and footsteps.
    pub fn to_entity(&self, p: &Protocol, slot: usize, command_time: i32) -> msg::EntityState {
        let mut e = msg::EntityState::null(p);
        e.number = slot as u32;
        let mut set = |name: &str, v: i32| {
            if let Some(i) = msg::EntityState::field_index(p, name) {
                e.fields[i] = v;
            }
        };
        set("eType", ET_PLAYER);
        set("clientNum", slot as i32);
        set("eFlags", self.entity_eflags());
        // Packed at link time: docs/research/cod11-player-clip.md.
        set("solid", self.linked_solid);
        set("legsAnim", self.anim.legs());
        // The torso does travel: the shoot, reload and putaway poses are the
        // weapon's, and the next task is what gives it a value.
        set("torsoAnim", self.anim.torso());
        set("weapon", i32::from(self.ps.weapon));
        set("iHeadIcon", self.head_icon);
        set("iHeadIconTeam", self.head_icon_team);
        // The gun a gunner mans (turrets doc, 4.4); 0 off one, as release
        // writes it.
        set(
            "otherEntityNum",
            self.mounted_on.map_or(0, |(gun, _)| gun as i32),
        );
        // A rider: the vehicle, and the seat, kind and motion packed
        // (`BG_PlayerStateToEntityState`, coduo-vehicles.md 11).
        if let Some(seat) = self.seat {
            set("otherEntityNum", seat.vehicle as i32);
            set("surfType", seat.pos | seat.kind << 3 | seat.motion << 6);
        }
        self.ring.write(&mut set);
        set("groundEntityNum", self.ps.ground_entity_num() as i32);
        // The lean the other client draws, the same -1..1 the playerstate
        // carries. Without it a leaning player stands straight to everyone
        // else.
        set("leanf", self.ps.lean.to_bits() as i32);
        // How the prone body bends over the ground (mantle doc, "The ground
        // samples").
        let body = self.entity_prone_body(command_time);
        set("fTorsoHeight", body.torso_height.to_bits() as i32);
        set("fTorsoPitch", body.torso_pitch.to_bits() as i32);
        set("fWaistPitch", body.waist_pitch.to_bits() as i32);
        set("pos.trType", trajectory::TR_LINEAR_STOP);
        // The time the position was simulated at, not the frame's: retail's
        // capture has trTime 2 to 18 ms behind the snapshot's serverTime,
        // which is the last usercmd's clock. Sending the frame time makes the
        // receiving client extrapolate from a base in its own future, and a
        // player standing still on a slope shivers.
        set("pos.trTime", command_time);
        set("pos.trDuration", PLAYER_TR_DURATION);
        set("apos.trType", trajectory::TR_INTERPOLATE);
        // The origin truncated to whole units: both writers snap `trBase`
        // under a 0xc00 control word on all three servers
        // (docs/research/coduo-vehicles.md 15.2, "The player's trBase").
        for axis in 0..3 {
            set(
                &format!("pos.trBase[{axis}]"),
                (self.ps.origin[axis] as i32 as f32).to_bits() as i32,
            );
            set(
                &format!("pos.trDelta[{axis}]"),
                self.ps.velocity[axis].to_bits() as i32,
            );
        }
        // A rider: `G_PlayerVehiclePositionAndBlend` ends on the plain
        // `BG_PlayerStateToEntityState` (game.mp.uo 0x20a35), `pos.trType`
        // 1 over the extrapolated write's time, duration and delta
        // (docs/research/coduo-vehicles.md 11.4).
        if self.seat.is_some() {
            set("pos.trType", trajectory::TR_INTERPOLATE);
        }
        // `ps.viewangles` whole, truncated to degrees the way
        // `BG_PlayerStateToEntityState` snaps it (0x2ccb4..0x2cd41): the
        // drawing client turns the body by the yaw and eases the spine after
        // the pitch (combat doc 16.3).
        for (axis, a) in self.view_angles.iter().enumerate() {
            set(
                &format!("apos.trBase[{axis}]"),
                (*a as i32 as f32).to_bits() as i32,
            );
        }
        // The legs' heading off the view, retail's `ps.movementDir` verbatim
        // (`BG_PlayerStateToEntityStateExtrapolate` @0x2d06d). Without it a
        // strafing player runs sideways with its legs pointing forward.
        set("angles2[1]", (self.ps.movement_dir as f32).to_bits() as i32);
        e
    }

    /// The end frame's `serverCursorHint` and its value. The shared
    /// pmove reads hint 7 off the playerstate as the client's does
    /// (`PM_UpdateAimDownSightFlag`, coduo-movement.md 11).
    pub fn set_cursor_hint(&mut self, hint: i32, val: i32) {
        self.cursor_hint = hint;
        self.cursor_hint_val = val;
        self.ps.mount_hint = (self.ps.rules == pmove::MoveRules::Uo
            && hint == crate::game::script::HINT_LMG)
            .then_some(if val != 0 {
                pmove::Stance::Crouch
            } else {
                pmove::Stance::Stand
            });
    }

    pub fn to_wire(&self, p: &Protocol, client_num: i32, command_time: i32) -> msg::PlayerState {
        let player = self.pm_type == PmType::Normal;
        let mut w = msg::PlayerState::null(p);
        // A field the protocol lacks (UO dropped `gunfx`, 1.1 has no sprint
        // fields) is left out.
        let mut set = |name: &str, v: i32| {
            if let Some(i) = msg::PlayerState::field_index(p, name) {
                w.fields[i] = v;
            }
        };
        set("clientNum", client_num);
        set("iCompassFriendInfo", self.compass_friend);
        let command_time = self.frozen_command_time.unwrap_or(command_time);
        set("commandTime", command_time);
        let (legs_timer, torso_timer) = self.anim.timers(command_time);
        set("legsTimer", legs_timer);
        set("torsoTimer", torso_timer);
        // Mode-dependent.
        set("pm_type", self.wire_pm_type());
        set("eFlags", self.ps_eflags());
        set("speed", self.ps.speed as i32);
        set("gravity", if player { self.ps.gravity as i32 } else { 0 });
        // The footstep phase, which a spectator's step pushes on too. The
        // predictor rebuilds from this on every wire round-trip
        // (`vcod_common::pmove::predict::from_wire`), so a stale 0 here
        // restarts the phase and shifts its footstep events onto other cmds
        // than the ones that raised them server-side.
        set("bobCycle", i32::from(self.ps.bob_cycle));
        if player {
            // Retail leaves all three at zero for a spectator.
            set("viewHeightCurrent", self.ps.view_height().to_bits() as i32);
            // `PM_CheckDuck` (cgame 0x30009de0) targets `deadViewHeight` for
            // `pm_type >= 6`; the client re-derives it, so this only keeps the
            // wire honest.
            let target = if self.dead_eye {
                pmove::VIEW_DEAD
            } else {
                self.ps.stance.view_height()
            };
            // 1.5 steps the target a stance at a time (cod15-movement.md 3).
            let target = match self.ps.rules {
                pmove::MoveRules::V1_5 | pmove::MoveRules::Uo if !self.dead_eye => {
                    self.ps.view_target
                }
                _ => target,
            };
            set("viewHeightTarget", target as i32);
            set("groundEntityNum", self.ps.ground_entity_num() as i32);
            // All three come out of one `ClientEndFrame` block a spectator
            // never reaches. Its guards are `sessionstate` playing,
            // `ps.clientNum == self` and, for the hint, `health > 0`;
            // nothing follows another client and nothing dies yet, so
            // `Normal` is the whole of that condition today. The hint is the
            // one that peels off first, at `health` 0.
            let stance_pmflags = if self.ps.ducked { PMF_DUCKED } else { 0 }
                | if self.ps.stance == pmove::Stance::Prone {
                    PMF_PRONE
                } else {
                    0
                }
                | if self.ps.prone_dive {
                    PMF_PRONE_DIVE
                } else {
                    0
                }
                | if self.ps.prone_blocked {
                    PMF_PRONE_BLOCKED
                } else {
                    0
                };
            let jump_held = if self.ps.jump_latched {
                PMF_JUMP_HELD
            } else {
                0
            };
            let ladder = if self.ps.on_ladder { PMF_LADDER } else { 0 };
            let backwards = if self.ps.backwards_run {
                PMF_BACKWARDS_RUN
            } else {
                0
            };
            let respawned = if self.respawned { PMF_RESPAWNED } else { 0 };
            let penalty = if self.ps.jump_penalty {
                vcod_common::net::flags::PMF_JUMP_PENALTY
            } else {
                0
            };
            let knockback = self.ps.knockback_flags;
            let disabled = if self.ps.weapon_disabled {
                vcod_common::net::flags::PMF_WEAPON_DISABLED
            } else {
                0
            };
            // UO's sprint and winded bits, which vcod keeps above 1.1's
            // (`flags::PMF_SPRINT_V22`); a 1.1 or 1.5 wire masks them off.
            let sprint = if self.ps.sprinting {
                vcod_common::net::flags::PMF_SPRINT_V22
            } else {
                0
            } | if self.ps.winded {
                vcod_common::net::flags::PMF_WINDED_V22
            } else {
                0
            };
            let overheated = if self.seat.is_some_and(|s| s.overheated) {
                vcod_common::net::flags::PMF_OVERHEATED_V22
            } else {
                0
            };
            set(
                "pm_flags",
                PMF_OWN_VIEW
                    | overheated
                    | sprint
                    | respawned
                    | stance_pmflags
                    | jump_held
                    | ladder
                    | backwards
                    | knockback
                    | penalty
                    | disabled
                    | if self.seat.is_some() {
                        // In a seat the sight flag is the button alone
                        // (`PM_UpdateAimDownSightFlag`), never walking.
                        if self.ps.ads_active {
                            pmove::weapon::PMF_ADS
                        } else {
                            0
                        }
                    } else {
                        pmove::weapon::ads_pm_flags(&self.ps)
                    },
            );
            set("pm_time", self.ps.knockback_ms as i32);
            set("jumpTime", self.jump_time);
            set("fJumpPeak", self.ps.jump_origin_z.to_bits() as i32);
            // The client predicts its own eye lerp; without these it restarts
            // from our value every snapshot and the view shakes for as long
            // as the lerp lasts.
            set("viewHeightLerpTarget", self.ps.view_lerp_target as i32);
            // The stance lerp's stamp; the dead eye's drop is not one.
            set(
                "viewHeightLerpTime",
                if self.dead_eye {
                    0
                } else {
                    self.view_lerp_start.unwrap_or(0)
                },
            );
            // The four feedback fields hold their last values (section 6).
            set("damageEvent", self.feedback.event & 0xff);
            set("damageCount", self.feedback.count);
            set("damageYaw", self.feedback.yaw & 0xff);
            set("damagePitch", self.feedback.pitch & 0xff);
            // The body's own yaw while prone; the client centres its view cone
            // on it, so a zero here aims the cone at world north.
            set("proneDirection", self.ps.prone_direction.to_bits() as i32);
            // The ground's pitch under the body and under the view; the
            // client centres its prone pitch cap on the second.
            set(
                "proneDirectionPitch",
                self.ps.prone_direction_pitch.to_bits() as i32,
            );
            set(
                "proneTorsoPitch",
                self.ps.prone_torso_pitch.to_bits() as i32,
            );
            // The last ladder's normal, kept off the rungs as retail keeps
            // it: the client's ladder probe and yaw cap read it.
            for (i, n) in self.ps.ladder_normal.to_array().into_iter().enumerate() {
                set(&format!("vLadderVec[{i}]"), n.to_bits() as i32);
            }
            let body = self.ps.prone_body;
            set("fTorsoHeight", body.torso_height.to_bits() as i32);
            set("fTorsoPitch", body.torso_pitch.to_bits() as i32);
            set("fWaistPitch", body.waist_pitch.to_bits() as i32);
            // 8 bits on the wire, so a leftward angle travels as its
            // unsigned byte; `angles2[1]` on the entity carries the signed
            // value as a float.
            set("movementDir", self.ps.movement_dir & 0xff);
            set("viewHeightLerpDown", i32::from(self.ps.view_lerp_down));
            // -1..1, left negative, the same convention retail sends.
            set("leanf", self.ps.lean.to_bits() as i32);
            set("serverCursorHint", self.cursor_hint & 0xff);
            set("serverCursorHintVal", self.cursor_hint_val & 0xff);
            set(
                "serverCursorHintString",
                if self.cursor_hint_string < 0 {
                    NO_CURSOR_HINT_STRING
                } else {
                    self.cursor_hint_string
                },
            );
            set("viewmodelIndex", self.viewmodel_index);
            set("legsAnim", self.anim.legs());
            set("torsoAnim", self.anim.torso());
            // Both are netfields the client predicts from, so a constant
            // here fights its own prediction once a snapshot: the ADS lerp
            // restarts and the spread reads a stale scale. Both come out of
            // the shared pmove code the client predicts with (combat doc,
            // 1.13 and 2.1).
            set("fWeaponPosFrac", self.ps.weapon_pos_frac.to_bits() as i32);
            set("aimSpreadScale", self.ps.aim_spread_scale.to_bits() as i32);
        }
        if !player && self.respawned {
            set("pm_flags", PMF_RESPAWNED);
        }
        // The spawn's standing eye, which the intermission's think never
        // moves: retail's 1.1d `probe_states` capture reads 60 and 60
        // (cod11-player-clip.md 1.1).
        if self.pm_type == PmType::Intermission {
            set("viewHeightCurrent", self.ps.view_height().to_bits() as i32);
            set("viewHeightTarget", self.ps.stance.view_height() as i32);
        }
        set("viewlocked", i32::from(self.viewlocked));
        set("shellshockIndex", self.shellshock[0]);
        set("shellshockTime", self.shellshock[1]);
        set("shellshockDuration", self.shellshock[2]);
        set("viewlocked_entNum", self.viewlocked_ent as i32);
        // An exit clears `vehPos` and leaves `vehType` and `vehMotion`.
        set("vehType", self.veh_type);
        set("vehMotion", self.veh_motion);
        if let Some(seat) = self.seat {
            set("vehPos", seat.pos);
        }
        set("gunfx", i32::from(self.gunfx));
        // What the client holds; both layouts are in the object model doc,
        // section 20. The sim owns all of it: the script host's copy is
        // mirrored into `ps` every frame, and the weapon machine writes
        // `ps.weapon` itself through a switch.
        set("weapons[0]", self.ps.weapons_held as u32 as i32);
        set("weapons[1]", (self.ps.weapons_held >> 32) as u32 as i32);
        let [lo, hi] = pmove::weapon::slot_words(&self.ps);
        set("weaponslots[0]", lo);
        set("weaponslots[4]", hi);
        set("weapon", i32::from(self.ps.weapon));
        // The weapon machine's own state (combat doc, section 1), and the
        // event ring both this and the entity carry.
        set("weaponstate", i32::from(self.ps.weaponstate));
        set("weapAnim", self.ps.weap_anim);
        set("weaponTime", self.ps.weapon_time_ms);
        set("weaponDelay", self.ps.weapon_delay_ms);
        set("grenadeTimeLeft", self.ps.grenade_time_left_ms);
        set("weaponrechamber[0]", self.ps.weapon_rechamber as u32 as i32);
        set(
            "weaponrechamber[1]",
            (self.ps.weapon_rechamber >> 32) as u32 as i32,
        );
        self.ring.write(&mut set);
        // Mode-independent: both captures agree on all of these. The box is
        // the standing one whatever the stance: retail transmits `maxs[2]`
        // 70 while crouched and prone too, and the mover derives its own
        // collision box from the stance rather than from these
        // (`crates/server/tests/playerstate_motion_ab.rs`).
        let (mins, maxs) = (
            self.ps.mins(),
            Vec3::new(
                self.ps.maxs().x,
                self.ps.maxs().y,
                pmove::Stance::Stand.height(),
            ),
        );
        for (i, (lo, hi)) in ["mins[0]", "mins[1]", "mins[2]"]
            .iter()
            .zip(["maxs[0]", "maxs[1]", "maxs[2]"])
            .enumerate()
        {
            set(lo, mins[i].to_bits() as i32);
            set(hi, maxs[i].to_bits() as i32);
        }
        set("proneViewHeight", pmove::VIEW_PRONE as i32);
        set("crouchViewHeight", pmove::VIEW_CROUCH as i32);
        set("standViewHeight", pmove::VIEW_STAND as i32);
        set("deadViewHeight", pmove::VIEW_DEAD as i32);
        set("walkSpeedScale", pmove::SCALE_WALK.to_bits() as i32);
        set("runSpeedScale", 1f32.to_bits() as i32);
        set("proneSpeedScale", pmove::SCALE_PRONE.to_bits() as i32);
        set("crouchSpeedScale", pmove::SCALE_CROUCH.to_bits() as i32);
        // The three axis scales the mover does not read; captured values.
        set("strafeSpeedScale", 0.8f32.to_bits() as i32);
        set("backSpeedScale", 0.7f32.to_bits() as i32);
        set("leanSpeedScale", 0.4f32.to_bits() as i32);
        set("friction", 1f32.to_bits() as i32);
        // United Offensive's: the sprint scale is constant, the fatigue
        // clock the sim's (docs/research/coduo-movement.md 3, 4).
        set("sprintSpeedScale", pmove::SCALE_SPRINT.to_bits() as i32);
        set("fatigueScale", self.ps.fatigue.to_bits() as i32);
        set("lastSprintTime", self.ps.last_sprint_time);
        // The last winded breath: a predictor rebuilt without it breathes on
        // every snapshot (coduo-movement.md 4).
        set("iFatigueSoundTime", self.ps.fatigue_sound_time);
        // Dynamic.
        for (i, axis) in ["origin[0]", "origin[1]", "origin[2]"].iter().enumerate() {
            set(axis, self.ps.origin[i].to_bits() as i32);
        }
        for (i, axis) in ["velocity[0]", "velocity[1]", "velocity[2]"]
            .iter()
            .enumerate()
        {
            set(axis, self.ps.velocity[i].to_bits() as i32);
        }
        // Every mode's: `PM_UpdateViewAngles` writes it for a `pm_type`
        // below 5, and a spawn's `SetClientViewAngle` is all the
        // intermission camera and a dead player keep (docs/protocol-1.1.md,
        // "View angles").
        for (i, axis) in ["viewangles[0]", "viewangles[1]", "viewangles[2]"]
            .iter()
            .enumerate()
        {
            set(axis, self.view_angles[i].to_bits() as i32);
        }
        for (i, axis) in ["delta_angles[0]", "delta_angles[1]", "delta_angles[2]"]
            .iter()
            .enumerate()
        {
            set(axis, self.delta_angles[i]);
        }
        // The two ammo arrays are not netfields: they travel in the
        // playerstate's array blocks (docs/protocol-1.1.md, "How `ammo[]` and
        // `ammoclip[]` are indexed"), so they are written last, once the
        // field setter's borrow is over.
        for (i, (ammo, clip)) in self.ps.ammo.iter().zip(&self.ps.ammoclip).enumerate() {
            w.set_ammo(i, *ammo);
            w.set_clip(i, *clip);
        }
        // `stats[0]`, `stats[2]` and the dead yaw in `stats[1]`
        // (docs/protocol-1.1.md, "Block 1").
        w.set_health(self.health);
        w.set_max_health(self.max_health);
        w.arrays.stats[1] = self.dead_yaw;
        // `stats[3]` is six raw bits, so "nobody" travels as 63; the compass
        // has no teammate to name until `TeamplayInfoMessage` exists.
        w.arrays.stats[3] = NO_TEAMMATE;
        w.arrays.stats[5] = i32::from(self.spawn_count);
        match &self.residue {
            Some(r) => r.under(w, p),
            None => w,
        }
    }
}

impl Residue {
    /// The copy with `own`'s [`SPECTATOR_OWNED`] fields written over it, its
    /// `pm_type` and `speed` once a cmd has flown, its `pm_flags` less the
    /// follow and ADS bits (0x46bb1), and its `eFlags` with the mount bits
    /// cleared (0x46b8c) and the teleport bit the sim flips.
    fn under(&self, own: msg::PlayerState, p: &Protocol) -> msg::PlayerState {
        let idx = |name| msg::PlayerState::field_index(p, name).unwrap();
        let mut w = self.wire.clone();
        w.fields[idx("pm_flags")] &= !(crate::follow::PMF_FOLLOW | pmove::weapon::PMF_ADS);
        let flown: &[&str] = if self.moved {
            &["pm_type", "speed"]
        } else {
            &[]
        };
        // UO has no `gunfx` (coduo-diff.md 7), so a name the table lacks is skipped.
        for name in SPECTATOR_OWNED.iter().chain(flown) {
            if let Some(i) = msg::PlayerState::field_index(p, name) {
                w.fields[i] = own.fields[i];
            }
        }
        let ef = idx("eFlags");
        w.fields[ef] = (w.fields[ef] & !(EF_MOUNTED_STAND | EF_TELEPORT_BIT))
            | (own.fields[ef] & EF_TELEPORT_BIT);
        w
    }
}

/// `PmoveSingle`'s `eFlags` 0x400 (game.mp.i386.so 0x33fa0..0x33fdf): the
/// attack bit without the talk bit, `weaponstate` ready or firing, and
/// `PM_WeaponAmmoAvailable` (0x3abe8), the clip the held weapon loads from.
/// The caller adds the `pm_type` and `PMF_RESPAWNED` gates.
pub fn attack_flag(ps: &pmove::PlayerState, buttons: u8, weapons: &[Option<WeaponDef>]) -> bool {
    use vcod_common::pmove::weapon::{WEAPON_FIRING, WEAPON_READY};
    buttons & msg::BUTTON_ATTACK != 0
        && buttons & msg::BUTTON_TALK == 0
        && matches!(ps.weaponstate, WEAPON_READY | WEAPON_FIRING)
        && weapons
            .get(ps.weapon as usize)
            .and_then(Option::as_ref)
            .and_then(|d| ps.ammoclip.get(d.clip_index))
            .is_some_and(|&c| c != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::turret::TurretStance;
    use vcod_common::net::msg::NULL_USERCMD;
    use vcod_common::net::protocol::{ENTITYNUM_NONE, PROTOCOL_V1};

    /// A stopped follow's copy is rebuilt on every protocol; UO lacks
    /// `gunfx`, and the lookup used to panic there.
    #[test]
    fn a_stopped_follow_rebuilds_on_every_protocol() {
        for p in Protocol::ALL {
            let mut own = msg::PlayerState::null(p);
            let at = |name| msg::PlayerState::field_index(p, name).unwrap();
            own.fields[at("clientNum")] = 3;
            let r = Residue {
                wire: msg::PlayerState::null(p),
                moved: true,
            };
            let w = r.under(own, p);
            assert_eq!(
                w.fields[at("clientNum")],
                3,
                "protocol {}",
                p.transport.version
            );
        }
    }

    /// The spawn's think runs the aim block on the spawn's own yaw, 180 as
    /// given where the cmd's view reads -180, so the sway's last view is it
    /// (combat doc 15.3, "The spawn's view").
    #[test]
    fn the_spawn_think_aims_along_the_spawns_yaw() {
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 180.0, NULL_USERCMD.angles);
        assert_eq!(sim.view_angles()[1], -180.0);
        sim.spawn_think_on(&NULL_USERCMD, 1000, None);
        assert_eq!(sim.aim, [0.0, 180.0]);
    }

    /// A jump's clock and origin travel as `jumpTime` and `fJumpPeak`, and
    /// the predictor reads the 500 ms cooldown back off them
    /// (docs/research/cod11-mantle.md, "Jumps").
    #[test]
    fn a_jump_puts_its_clock_and_origin_on_the_wire() {
        let p = &PROTOCOL_V1;
        let world = vcod_common::collision::test_world(&[]);
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        let mut t = 1000;
        for _ in 0..20 {
            t += 50;
            let idle = UserCmd {
                server_time: t,
                ..NULL_USERCMD
            };
            sim.step(&idle, 0.05, Some(MoveWorld::bare(&world)), &[]);
        }
        assert!(sim.ps.on_ground);
        let takeoff_z = sim.ps.origin.z;
        let jump = UserCmd {
            server_time: t + 16,
            up: 127,
            ..NULL_USERCMD
        };
        sim.step(&jump, 0.016, Some(MoveWorld::bare(&world)), &[]);
        let w = sim.to_wire(p, 0, t + 16);
        assert_eq!(w.field_i32(p, "jumpTime"), t + 16);
        assert_eq!(w.field_f32(p, "fJumpPeak"), takeoff_z + pmove::JUMP_HEIGHT);
        let back = vcod_common::pmove::predict::from_wire(p, &w, Some(&jump));
        assert_eq!(back.ps.since_jump_ms, 0.0);
        assert_eq!(back.ps.jump_origin_z, takeoff_z + pmove::JUMP_HEIGHT);
        assert!(back.ps.jump_latched);
    }

    /// The intermission camera (map-cycle doc 6.2, and the `pm_type=5`
    /// traces in `tests/fixtures/netchan/mp_carentan-dm-mapchange.txt`):
    /// `pm_type` 5, a spectator's `eFlags`, health 0, an origin nothing the
    /// client presses moves, and no entity for anyone else.
    #[test]
    fn an_intermission_client_is_frozen_unlinked_and_at_pm_type_five() {
        let p = &PROTOCOL_V1;
        let world = vcod_common::collision::test_world(&[]);
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        // The connect's own `spawnSpectator`, which every stock gametype runs
        // before the first player spawn: it is the first `EF_TELEPORT_BIT`
        // flip and the life after it reads 16 because of it.
        sim.become_spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.health = 100;
        sim.max_health = 100;
        assert!(sim.linked(), "a live player is linked");

        sim.become_intermission([384.0, -624.0, 184.0], 90.0, NULL_USERCMD.angles, 60050);
        let at = sim.ps.origin;
        let forward = UserCmd {
            forward: 127,
            angles: [1000, 2000, 0],
            ..NULL_USERCMD
        };
        for _ in 0..20 {
            assert!(
                sim.step(&forward, 0.05, Some(MoveWorld::bare(&world)), &[])
                    .is_empty(),
                "the intermission camera raised an event"
            );
        }
        assert_eq!(sim.ps.origin, at, "the intermission camera moved");
        assert!(!sim.linked(), "the intermission camera is linked");

        let w = sim.to_wire(p, 0, 60150);
        assert_eq!(w.field_i32(p, "pm_type"), PM_INTERMISSION);
        // Where the spawn put it, 100 ms behind its frame, whatever the cmds
        // since: the retail intermission run reads 59950 at 60050 to 60150.
        assert_eq!(w.field_i32(p, "commandTime"), 59950);
        // The spawn's view, whatever the cmds turned: `PM_UpdateViewAngles`
        // returns at `pm_type` 5, and the map-change capture reads 0, 90 on
        // every intermission frame.
        let view: Vec<f32> = (0..3)
            .map(|i| f32::from_bits(w.field_i32(p, &format!("viewangles[{i}]")) as u32))
            .collect();
        assert_eq!(view, [0.0, 90.0, 0.0]);
        assert_eq!(w.field_i32(p, "eFlags"), 24);
        assert_eq!(w.health(), 0, "the spawn's memset is never written back");
        assert_eq!(w.field_i32(p, "eventSequence"), 0);
    }

    /// The tank capture's exit (`mp_cassino-tdm-slope-8ms-tank-drive-uo.txt`,
    /// msg 483): the use cmd's frame ends with the rider on the spot and the
    /// 200 pop unspent, the cmds after the exit run under `pm_type` 1; the
    /// rise starts on the next frame's first cmd, past `G_RunClient`.
    #[test]
    fn the_cmds_left_in_an_exits_frame_move_nothing() {
        let world = vcod_common::collision::test_world(&[]);
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        let seat = Seat {
            vehicle: 70,
            pos: 1,
            kind: 1,
            motion: 1,
            angles: [0.0; 3],
            hull_view: true,
            overheated: false,
            link_offset: [0.0; 3],
        };
        sim.enter_seat(seat, [0.0, 0.0, 64.0], [0.0; 3]);
        sim.leave_seat(Some([0.0, 0.0, 100.0]), Some([0.0, 0.0, 200.0]));
        let spot = sim.ps.origin;
        let mut t = 1000;
        let mut cmd = |sim: &mut ClientSim, ms: i32| {
            t += ms;
            let c = UserCmd {
                server_time: t,
                ..NULL_USERCMD
            };
            sim.step(&c, ms as f32 / 1000.0, Some(MoveWorld::bare(&world)), &[]);
        };
        for _ in 0..3 {
            cmd(&mut sim, 8);
        }
        assert_eq!(sim.wire_pm_type(), PM_NORMAL_LINKED);
        assert_eq!(sim.ps.origin, spot);
        assert_eq!(sim.ps.velocity.z, 200.0);
        sim.run_client();
        assert_eq!(sim.wire_pm_type(), 0);
        cmd(&mut sim, 16);
        assert!(sim.ps.origin.z > spot.z, "{:?}", sim.ps.origin);
        assert_eq!(sim.ps.velocity.z, 187.0);
    }

    /// A climber's `pm_flags` carry the ladder bit, as retail's 0x40010 on
    /// the rungs (docs/research/cod11-combat.md, 9.7).
    #[test]
    fn a_climber_is_sent_pmf_ladder() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.ps.on_ladder = true;
        let pm_flags = sim.to_wire(p, 0, 0).field_i32(p, "pm_flags");
        assert_eq!(pm_flags, PMF_OWN_VIEW | PMF_LADDER);
    }

    /// `PMF_RESPAWNED` (`pm_flags` 0x800): every spawn sets it, and the
    /// spawn's own null cmd clears it again wherever `PmoveSingle` runs with
    /// `pm_type` 5 or below. So a live or spectating spawn never shows it, a
    /// dead spawn (the killcam's end, retail's 0x40800) and the intermission
    /// camera keep it, and the next spawn's think takes it off.
    #[test]
    fn only_a_dead_spawn_and_the_intermission_camera_keep_pmf_respawned() {
        let p = &PROTOCOL_V1;
        let world = vcod_common::collision::test_world(&[]);
        let pm_flags = |sim: &ClientSim| sim.to_wire(p, 0, 0).field_i32(p, "pm_flags");
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        assert_eq!(pm_flags(&sim) & PMF_RESPAWNED, 0);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        assert_eq!(pm_flags(&sim), PMF_OWN_VIEW);

        let copied = crate::follow::Copied {
            eye: [0.0, 0.0, 68.0],
            angles: [0.0, 90.0, 0.0],
            origin: [0.0, 0.0, 8.0],
            velocity: [0.0; 3],
            teleport_bit: false,
            frame: None,
        };
        sim.spawn_from_copy(
            &copied,
            false,
            NULL_USERCMD.angles,
            Some(MoveWorld::bare(&world)),
        );
        // The think's 100 ms of dead pmove: 60 less 18.
        assert_eq!(sim.ps.view_height(), 42.0);
        assert_eq!(pm_flags(&sim), PMF_OWN_VIEW | PMF_RESPAWNED);
        let idle = UserCmd {
            server_time: 50,
            ..NULL_USERCMD
        };
        sim.step(&idle, 0.05, Some(MoveWorld::bare(&world)), &[]);
        assert_eq!(pm_flags(&sim), PMF_OWN_VIEW | PMF_RESPAWNED);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        assert_eq!(pm_flags(&sim), PMF_OWN_VIEW);

        sim.become_intermission([384.0, -624.0, 184.0], 90.0, NULL_USERCMD.angles, 0);
        assert_eq!(pm_flags(&sim), PMF_RESPAWNED);
    }

    fn cmd(forward: i8, pitch_short: i32, yaw_short: i32) -> UserCmd {
        UserCmd {
            forward,
            angles: [pitch_short, yaw_short, 0],
            ..Default::default()
        }
    }

    /// One frame of the anim machine with the state set by hand, for the
    /// clauses the retail capture cannot reach: it was taken with one rifle,
    /// never held two axes at once and never strafed crouched. Names rather
    /// than indices -- `animtree.rs` is what pins the index order.
    #[test]
    fn the_conditions_pick_the_clause_the_script_names() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        use pmove::Stance::{Crouch, Prone, Stand};
        // label, weapon, class, stance, backpedalling, forward, right, anim
        type Case = (
            &'static str,
            &'static str,
            &'static str,
            pmove::Stance,
            bool,
            i8,
            i8,
            &'static str,
        );
        let cases: &[Case] = &[
            (
                "a pistol runs its own loop",
                "colt_mp",
                "pistol",
                Stand,
                false,
                127,
                0,
                "pb_sprint",
            ),
            (
                "a diagonal is a forward run, not a strafe",
                "m1carbine_mp",
                "rifle",
                Stand,
                false,
                127,
                127,
                "pb_combatrun_forward_loop",
            ),
            (
                "a crouched strafe has its own clause",
                "m1carbine_mp",
                "rifle",
                Crouch,
                false,
                0,
                -127,
                "pb_crouch_run_left",
            ),
            (
                "and the weapon still picks inside it",
                "colt_mp",
                "pistol",
                Stand,
                false,
                0,
                -127,
                "pb_combatrun_left_loop_pistol",
            ),
            (
                "a crouched backpedal is its own movetype",
                "m1carbine_mp",
                "rifle",
                Crouch,
                true,
                -127,
                0,
                "pb_crouch_run_back",
            ),
            (
                "so is a backwards crawl",
                "m1carbine_mp",
                "rifle",
                Prone,
                true,
                -127,
                0,
                "pb_prone_crawl_back",
            ),
        ];
        for (label, weapon, class, stance, back, forward, right, want) in cases {
            let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
            sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
            sim.ps.settle_stance(*stance);
            sim.ps.on_ground = true;
            sim.ps.backwards_run = *back;
            sim.ps.velocity = Vec3::new(120.0, 0.0, 0.0);
            sim.ps.footsteps_xyspeed = sim.ps.velocity.truncate().length();
            let cmd = UserCmd {
                forward: *forward,
                right: *right,
                ..NULL_USERCMD
            };
            let inputs = AnimInputs {
                anims: &anims,
                weapon,
                weapon_class: class,
            };
            sim.update_anims(&inputs, &cmd, (1000, 1000), &[], &mut 1u64);
            assert_eq!(anims.name(sim.anim.legs()), Some(*want), "{label}");
        }
    }

    /// Retail selects nothing while off the ground (@0x323a2), so the takeoff
    /// anim stays until the landing rather than falling back to the standing
    /// idle when the jump event's 5 ms duration runs out.
    #[test]
    fn a_jump_owns_the_legs_until_the_landing() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        let inputs = AnimInputs {
            anims: &anims,
            weapon: "m1carbine_mp",
            weapon_class: "rifle",
        };
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.ps.on_ground = true;
        sim.update_anims(&inputs, &NULL_USERCMD, (1000, 1000), &[], &mut 1u64);
        assert_eq!(anims.name(sim.anim.legs()), Some("pb_stand_alert"));

        // The impulse pmove reports, not merely leaving the ground.
        sim.ps.on_ground = false;
        sim.jumped = Some((
            pmove::Takeoff {
                forced: true,
                back: false,
                before_timers: false,
            },
            1050,
            vcod_common::animscript::Movetype::Idle,
        ));
        sim.update_anims(&inputs, &NULL_USERCMD, (1050, 1050), &[], &mut 1u64);
        assert_eq!(anims.name(sim.anim.legs()), Some("pb_standjump_takeoff"));
        // Well past the takeoff clause's `duration 5`.
        for t in [1100, 1150, 1200, 1250] {
            sim.update_anims(&inputs, &NULL_USERCMD, (t, t), &[], &mut 1u64);
            assert_eq!(
                anims.name(sim.anim.legs()),
                Some("pb_standjump_takeoff"),
                "the flight kept selecting at {t}"
            );
        }
        // The landing pmove reports, fast enough for the land anim.
        sim.ps.on_ground = true;
        sim.land_anim = Some(1300);
        sim.update_anims(&inputs, &NULL_USERCMD, (1300, 1300), &[], &mut 1u64);
        assert_eq!(anims.name(sim.anim.legs()), Some("pb_standjump_land"));
    }

    /// A landing slower than `LAND_ANIM_SPEED`, the one-unit drop a turret
    /// release ends in, plays no land anim: the retail turret capture keeps
    /// `pb_stand_alert` through it (`tests/turret_ab.rs`).
    #[test]
    fn a_slow_landing_plays_no_land_anim() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        let inputs = AnimInputs {
            anims: &anims,
            weapon: "m1carbine_mp",
            weapon_class: "rifle",
        };
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.ps.on_ground = true;
        sim.update_anims(&inputs, &NULL_USERCMD, (1000, 1000), &[], &mut 1u64);
        let before = sim.anim.legs();
        sim.ps.on_ground = false;
        sim.update_anims(&inputs, &NULL_USERCMD, (1050, 1050), &[], &mut 1u64);
        sim.ps.on_ground = true;
        sim.update_anims(&inputs, &NULL_USERCMD, (1100, 1100), &[], &mut 1u64);
        assert_eq!(sim.anim.legs(), before, "no land anim, no toggle flip");
    }

    /// The land anim's timer half: a fast landing while the last land anim
    /// still holds the legs (`legsTimer` running) raises no second one.
    #[test]
    fn a_landing_inside_the_last_land_anim_plays_none() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        let inputs = AnimInputs {
            anims: &anims,
            weapon: "m1carbine_mp",
            weapon_class: "rifle",
        };
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.ps.on_ground = true;
        sim.update_anims(&inputs, &NULL_USERCMD, (1000, 1000), &[], &mut 1u64);
        sim.ps.on_ground = false;
        sim.update_anims(&inputs, &NULL_USERCMD, (1050, 1050), &[], &mut 1u64);
        sim.ps.on_ground = true;
        sim.land_anim = Some(1100);
        sim.update_anims(&inputs, &NULL_USERCMD, (1100, 1100), &[], &mut 1u64);
        assert_eq!(anims.name(sim.anim.legs()), Some("pb_standjump_land"));
        let first = sim.anim.legs();
        // Airborne and down again inside the clause's `duration 100`.
        sim.ps.on_ground = false;
        sim.update_anims(&inputs, &NULL_USERCMD, (1120, 1120), &[], &mut 1u64);
        sim.ps.on_ground = true;
        sim.land_anim = Some(1150);
        sim.update_anims(&inputs, &NULL_USERCMD, (1150, 1150), &[], &mut 1u64);
        assert_eq!(
            sim.anim.legs(),
            first,
            "the held land anim is not restarted"
        );
    }

    /// Leaving the ground is not jumping. Retail's own mp_pavlov capture backs
    /// off a ledge at `run_back` and reads the run loop (index 93) while
    /// airborne, so a fall keeps whatever the legs were doing; raising the
    /// takeoff on the edge would freeze the standing jump for the whole fall.
    #[test]
    fn running_off_a_ledge_keeps_the_run_loop() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        let inputs = AnimInputs {
            anims: &anims,
            weapon: "m1carbine_mp",
            weapon_class: "rifle",
        };
        let running = UserCmd {
            forward: 127,
            ..NULL_USERCMD
        };
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.ps.on_ground = true;
        sim.ps.velocity = Vec3::new(190.0, 0.0, 0.0);
        sim.ps.footsteps_xyspeed = sim.ps.velocity.truncate().length();
        sim.update_anims(&inputs, &running, (1000, 1000), &[], &mut 1u64);
        assert_eq!(
            anims.name(sim.anim.legs()),
            Some("pb_combatrun_forward_loop")
        );

        // Off the edge: airborne, no impulse.
        sim.ps.on_ground = false;
        for t in [1050, 1100, 1150, 1200] {
            sim.ps.velocity.z -= 40.0;
            sim.update_anims(&inputs, &running, (t, t), &[], &mut 1u64);
            assert_eq!(
                anims.name(sim.anim.legs()),
                Some("pb_combatrun_forward_loop"),
                "the fall changed the anim at {t}"
            );
        }
    }

    /// A climber is off the ground and still animates: retail's ladder flag
    /// bypasses the airborne early-out, and the two climb blocks are the only
    /// thing that reads the climb direction. Mounting is not a jump either.
    #[test]
    fn a_ladder_climbs_rather_than_freezing() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        let inputs = AnimInputs {
            anims: &anims,
            weapon: "m1carbine_mp",
            weapon_class: "rifle",
        };
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.ps.on_ground = true;
        sim.update_anims(&inputs, &NULL_USERCMD, (1000, 1000), &[], &mut 1u64);

        // Mounted: off the ground without an impulse, climbing.
        sim.ps.on_ground = false;
        sim.ps.on_ladder = true;
        sim.ps.velocity = Vec3::new(0.0, 0.0, 60.0);
        sim.ps.footsteps_xyspeed = sim.ps.velocity.truncate().length();
        sim.update_anims(&inputs, &NULL_USERCMD, (1050, 1050), &[], &mut 1u64);
        assert_eq!(anims.name(sim.anim.legs()), Some("pb_climbup"));

        sim.ps.velocity.z = -60.0;
        sim.update_anims(&inputs, &NULL_USERCMD, (1100, 1100), &[], &mut 1u64);
        assert_eq!(anims.name(sim.anim.legs()), Some("pb_climbdown"));
    }

    /// 1.1's `PM_Footsteps` idles off `viewHeightLerpTarget` (0x324c8) and
    /// moves off the effective stance (0x32340): a lie-down from standing
    /// idles crouched through the eye's first leg and crawls if it moves.
    #[test]
    fn a_11_lie_down_idles_crouched_through_the_first_leg() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        let inputs = AnimInputs {
            anims: &anims,
            weapon: "m1carbine_mp",
            weapon_class: "rifle",
        };
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.ps.settle_stance(pmove::Stance::Stand);
        sim.ps.stance = pmove::Stance::Prone;
        sim.ps.view_lerp_target = pmove::VIEW_CROUCH;
        sim.ps.view_lerp_down = true;
        sim.ps.on_ground = true;
        sim.update_anims(&inputs, &NULL_USERCMD, (1000, 1000), &[], &mut 1u64);
        assert_eq!(sim.movetype, vcod_common::animscript::Movetype::IdleCr);
        sim.ps.velocity = Vec3::new(60.0, 0.0, 0.0);
        sim.ps.footsteps_xyspeed = sim.ps.velocity.truncate().length();
        let fwd = UserCmd {
            forward: 127,
            ..NULL_USERCMD
        };
        sim.update_anims(&inputs, &fwd, (1050, 1050), &[], &mut 1u64);
        assert_eq!(sim.movetype, vcod_common::animscript::Movetype::WalkProne);
        sim.ps.velocity = Vec3::ZERO;
        sim.ps.footsteps_xyspeed = sim.ps.velocity.truncate().length();
        sim.ps.view_lerp_target = pmove::VIEW_PRONE;
        sim.update_anims(&inputs, &NULL_USERCMD, (1100, 1100), &[], &mut 1u64);
        assert_eq!(sim.movetype, vcod_common::animscript::Movetype::IdleProne);
    }

    /// The backpedal latch picks the event as well as the movetype: retail's
    /// `jumpbk` block gates its first clause on the crouched and prone
    /// movetypes, which is the only place the two events differ for a
    /// rifleman.
    #[test]
    fn a_backwards_crouched_jump_raises_jumpbk() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        let inputs = AnimInputs {
            anims: &anims,
            weapon: "m1carbine_mp",
            weapon_class: "rifle",
        };
        let back = UserCmd {
            forward: -127,
            ..NULL_USERCMD
        };
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.ps.settle_stance(pmove::Stance::Crouch);
        sim.ps.backwards_run = true;
        sim.ps.on_ground = true;
        sim.ps.velocity = Vec3::new(120.0, 0.0, 0.0);
        sim.ps.footsteps_xyspeed = sim.ps.velocity.truncate().length();
        sim.update_anims(&inputs, &back, (1000, 1000), &[], &mut 1u64);
        assert_eq!(anims.name(sim.anim.legs()), Some("pb_crouch_run_back"));

        sim.ps.on_ground = false;
        sim.jumped = Some((
            pmove::Takeoff {
                forced: true,
                back: true,
                before_timers: false,
            },
            1050,
            vcod_common::animscript::Movetype::RunCrBk,
        ));
        sim.update_anims(&inputs, &back, (1050, 1050), &[], &mut 1u64);
        assert_eq!(
            anims.name(sim.anim.legs()),
            Some("pb_chicken_dance_crouch"),
            "`jump` would have given the standing takeoff"
        );
    }

    /// Retail updates the strafe condition only from a cmd that asks for
    /// movement: a cmd asking for neither axis leaves it alone, so a player
    /// coasting out of a strafe keeps strafing (@0x32504).
    #[test]
    fn a_cmd_asking_for_nothing_leaves_the_strafe_condition_alone() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        let inputs = AnimInputs {
            anims: &anims,
            weapon: "m1carbine_mp",
            weapon_class: "rifle",
        };
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.ps.on_ground = true;
        sim.ps.velocity = Vec3::new(120.0, 0.0, 0.0);
        sim.ps.footsteps_xyspeed = sim.ps.velocity.truncate().length();
        let strafe = UserCmd {
            right: 127,
            ..NULL_USERCMD
        };
        sim.update_anims(&inputs, &strafe, (1000, 1000), &[], &mut 1u64);
        assert_eq!(anims.name(sim.anim.legs()), Some("pb_combatrun_right_loop"));
        // Still sliding, no longer asking: retail does not touch the condition.
        sim.update_anims(&inputs, &NULL_USERCMD, (1050, 1050), &[], &mut 1u64);
        assert_eq!(anims.name(sim.anim.legs()), Some("pb_combatrun_right_loop"));
        // A forward cmd clears it, even though nothing about the velocity
        // changed.
        let forward = UserCmd {
            forward: 127,
            ..NULL_USERCMD
        };
        sim.update_anims(&inputs, &forward, (1100, 1100), &[], &mut 1u64);
        assert_eq!(
            anims.name(sim.anim.legs()),
            Some("pb_combatrun_forward_loop")
        );
    }

    /// A prone view past the 85-degree cone is pushed back by `delta_angles`,
    /// which is how retail enforces it (`PM_UpdateViewAngles` 0x331c8), and
    /// the body's own yaw goes out in `proneDirection`.
    #[test]
    fn a_prone_view_past_the_cap_pushes_delta_angles() {
        let p = &PROTOCOL_V1;
        let w = vcod_common::collision::test_world(&[]);
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        let prone = |t: i32, yaw_deg: f32| UserCmd {
            server_time: t,
            wbuttons: msg::WBUTTON_PRONE,
            up: -127,
            angles: [0, (yaw_deg * ANGLE2SHORT) as i32 & 0xffff, 0],
            ..NULL_USERCMD
        };

        let mut t = 1000;
        for _ in 0..20 {
            t += 50;
            sim.step(&prone(t, 0.0), 0.05, Some(MoveWorld::bare(&w)), &[]);
        }
        assert_eq!(sim.ps.stance, pmove::Stance::Prone);
        let before = sim.delta_angles[1];
        assert_eq!(
            sim.to_wire(p, 0, 0).field_i32(p, "proneDirection"),
            sim.ps.prone_direction.to_bits() as i32
        );

        // Well past the cone: the body cannot swing the whole way in one
        // frame, so the rest comes off the view.
        sim.step(&prone(t + 50, 150.0), 0.05, Some(MoveWorld::bare(&w)), &[]);
        assert_ne!(
            sim.delta_angles[1], before,
            "a view past the cap must be pushed back"
        );
    }

    /// The eye-height lerp is stamped with the serverTime it started at and
    /// cleared when it settles, which is retail's own bookkeeping: a settled
    /// capture reads 0 either way, so only a trace through the transition
    /// shows it (docs/protocol-1.1.md, "The view-height lerp").
    #[test]
    fn the_view_height_lerp_carries_the_time_it_started() {
        let p = &PROTOCOL_V1;
        let w = vcod_common::collision::test_world(&[]);
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        let lerp_time = |sim: &ClientSim| sim.to_wire(p, 0, 0).field_i32(p, "viewHeightLerpTime");
        let crouch = |t: i32| UserCmd {
            server_time: t,
            wbuttons: msg::WBUTTON_CROUCH,
            up: -127,
            ..NULL_USERCMD
        };

        let mut t = 1000;
        // Settle on the floor first, so the only thing moving is the eye.
        for _ in 0..20 {
            t += 50;
            sim.step(&NULL_USERCMD, 0.05, Some(MoveWorld::bare(&w)), &[]);
        }
        assert_eq!(lerp_time(&sim), 0, "a settled eye carries no stamp");

        t += 50;
        sim.step(&crouch(t), 0.05, Some(MoveWorld::bare(&w)), &[]);
        assert_eq!(lerp_time(&sim), t, "the stamp is the cmd that started it");
        let started = t;
        t += 50;
        sim.step(&crouch(t), 0.05, Some(MoveWorld::bare(&w)), &[]);
        assert_eq!(
            lerp_time(&sim),
            started,
            "and it does not move while lerping"
        );

        for _ in 0..20 {
            t += 50;
            sim.step(&crouch(t), 0.05, Some(MoveWorld::bare(&w)), &[]);
        }
        assert!(sim.ps.view_height_settled());
        assert_eq!(lerp_time(&sim), 0, "the stamp clears when the eye settles");
    }

    /// One field of the retail player capture the `playerstate_ab` gate diffs
    /// against. Both fixtures agree on everything this module derives, so one
    /// of them is enough here.
    fn retail_player(field: &str) -> i32 {
        let text = include_str!("../tests/fixtures/playerstate/mp_pavlov-dm.txt");
        let (_, value) = text
            .lines()
            .filter_map(|l| l.split_once(' '))
            .find(|(name, _)| *name == field)
            .unwrap_or_else(|| panic!("no {field} in the capture"));
        value
            .parse()
            .unwrap_or_else(|e| panic!("{field} is {value:?}, not an i32: {e}"))
    }

    /// The player half of `to_wire`, against that capture. The gate cannot
    /// reach this branch until a client can answer the team menu and spawn,
    /// so until then this is what pins the values.
    #[test]
    fn a_standing_player_carries_the_captured_values() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        // The connect's own `spawnSpectator`, which every stock gametype runs
        // before the first player spawn: it is the first `EF_TELEPORT_BIT`
        // flip and the life after it reads 16 because of it.
        sim.become_spectator([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        // The capture is of a player standing still on the floor.
        sim.ps.on_ground = true;
        let w = sim.to_wire(p, 0, 0);
        for f in [
            "pm_type",
            "eFlags",
            "speed",
            "gravity",
            "viewHeightCurrent",
            "viewHeightTarget",
            "groundEntityNum",
        ] {
            assert_eq!(w.field_i32(p, f), retail_player(f), "{f}");
        }
    }

    #[test]
    fn wire_carries_the_pinned_constants_and_dynamic_fields() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([10.0, 20.0, 30.0], 90.0, NULL_USERCMD.angles);
        sim.become_spectator([10.0, 20.0, 30.0], 90.0, NULL_USERCMD.angles);
        let w = sim.to_wire(p, 3, 114_800);
        assert_eq!(w.field_i32(p, "pm_type"), 4);
        assert_eq!(w.field_i32(p, "speed"), 400);
        assert_eq!(w.field_i32(p, "clientNum"), 3);
        assert_eq!(w.field_i32(p, "commandTime"), 114_800);
        assert_eq!(w.field_i32(p, "eFlags"), 24);
        assert_eq!(w.field_f32(p, "origin[0]"), 10.0);
        assert_eq!(w.field_f32(p, "origin[2]"), 30.0);
        // View heights are -8-bit int fields on the wire, not float bits.
        assert_eq!(w.field_i32(p, "standViewHeight"), 60);
        assert_eq!(w.field_i32(p, "crouchViewHeight"), 40);
        assert_eq!(w.field_i32(p, "proneViewHeight"), 11);
        assert_eq!(w.field_i32(p, "deadViewHeight"), 8);
        assert_eq!(w.field_f32(p, "walkSpeedScale"), 0.4);
        assert_eq!(w.field_i32(p, "mins[0]"), (-15f32).to_bits() as i32);
        assert_eq!(w.field_f32(p, "maxs[2]"), 70.0);
        assert_eq!(w.field_i32(p, "gravity"), 0);
        // Only a step ticks a spectator's footstep phase; the player half is
        // `bob_cycle_reaches_the_wire_for_a_player`.
        assert_eq!(w.field_i32(p, "bobCycle"), 0);
    }

    /// `from_wire` rebuilds the predictor's footstep phase from this field
    /// (`vcod_common::pmove::predict::from_wire`), so a value `step` left on
    /// `ps.bob_cycle` has to survive the round trip unchanged.
    #[test]
    fn bob_cycle_reaches_the_wire_for_a_player() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        sim.ps.bob_cycle = 173;
        let w = sim.to_wire(p, 0, 0);
        assert_eq!(w.field_i32(p, "bobCycle"), i32::from(sim.ps.bob_cycle));
    }

    /// `step` reads cmd angles in the wire's positive-down convention and
    /// stores the sim's positive-up one; a spawn yaw of 0 keeps
    /// `delta_angles` at zero so the cmd angle passes straight through.
    #[test]
    fn step_applies_the_camera_pitch_convention() {
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, NULL_USERCMD.angles);
        // 45 deg down on the wire, 90 deg yaw.
        let c = cmd(0, (45.0 * ANGLE2SHORT) as i32, (90.0 * ANGLE2SHORT) as i32);
        sim.step(&c, 0.05, None, &[]);
        assert_eq!(sim.ps.yaw.to_degrees(), 90.0);
        assert_eq!(sim.ps.pitch.to_degrees(), -45.0);
    }

    /// A spectator carries the captured spectator constants; a player does
    /// not. The values `to_wire` used to pin unconditionally are the ones the
    /// retail player capture disagrees with once the client is alive, which is
    /// why they move behind the mode instead of staying literals.
    #[test]
    fn the_wire_constants_follow_the_mode() {
        let p = &PROTOCOL_V1;
        let spec = ClientSim::spectator([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        let ps = spec.to_wire(p, 0, 0);
        assert_eq!(ps.field_i32(p, "pm_type"), 4);

        let mut player = ClientSim::spectator([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        player.become_player([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        let ps = player.to_wire(p, 0, 0);
        assert_eq!(ps.field_i32(p, "pm_type"), 0);
        assert_ne!(
            ps.field_i32(p, "speed"),
            pmove::SPEED_SPECTATOR as i32,
            "a player still moves at the spectator's speed"
        );
    }

    /// A player's hint reaches the wire; the next spawn clears it.
    #[test]
    fn the_cursor_hint_rides_a_players_wire_until_the_next_spawn() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        sim.cursor_hint = 79;
        let ps = sim.to_wire(p, 0, 0);
        assert_eq!(ps.field_i32(p, "serverCursorHint"), 79);
        assert_eq!(ps.field_i32(p, "serverCursorHintVal"), 0);
        assert_eq!(ps.field_i32(p, "serverCursorHintString"), 255);
        sim.become_spectator([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        assert_eq!(sim.cursor_hint, 0);
    }

    /// With no world a player still simulates rather than panicking, because
    /// every unit test here mounts no map.
    #[test]
    fn a_player_without_a_world_still_steps() {
        let mut sim = ClientSim::spectator([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        let cmd = UserCmd {
            forward: 127,
            ..UserCmd::default()
        };
        sim.step(&cmd, 0.05, None, &[]);
    }

    /// The spectator half of the three-part edit: a sim spawned facing 90
    /// degrees reports that yaw to a client whose cmd angles are zero,
    /// because the offset lives in `delta_angles` and the client adds it
    /// back; `step` must add it back the same way to keep simulating at the
    /// spawn yaw once a real cmd arrives. A partial edit fails this: dropping
    /// the delta write reports 0, and reverting `step` alone snaps the
    /// simulated yaw back to the cmd's raw 0 rather than 90. `16_384` is
    /// `ANGLE2SHORT(90)`, the same value the committed capture fixture
    /// carries (`crates/common/src/net/msg.rs:1826`). Pitch (`[0]`) is
    /// asserted on both fields too, a strict superset of the deleted
    /// `delta_angles_stay_zero`: only the yaw is spawn-dependent, so a
    /// `spawn_delta_angles` that put the yaw in the wrong slot must fail here
    /// as well.
    ///
    /// A spectator's `viewangles` is the same sum a player's is:
    /// `PM_UpdateViewAngles` takes its normal arm below `pm_type` 5
    /// (docs/protocol-1.1.md, "View angles"); the player half is
    /// [`a_players_viewangles_carry_the_summed_view`].
    #[test]
    fn delta_angles_carry_the_spawn_yaw_into_a_spectators_viewangles() {
        let p = &PROTOCOL_V1;
        let yaw = |sim: &ClientSim| {
            f32::from_bits(sim.to_wire(p, 0, 0).field_i32(p, "viewangles[1]") as u32)
        };
        let mut sim = ClientSim::spectator([0.0, 0.0, 64.0], 90.0, NULL_USERCMD.angles);
        let ps = sim.to_wire(p, 0, 0);
        assert_eq!(ps.field_i32(p, "delta_angles[0]"), 0);
        assert_eq!(ps.field_i32(p, "delta_angles[1]"), 16_384);
        assert_eq!(yaw(&sim), 90.0);

        // A cmd of raw zeros faces the spawn's 90: step must add
        // delta_angles back.
        sim.step(&cmd(0, 0, 0), 0.05, None, &[]);
        assert_eq!(sim.ps.yaw.to_degrees(), 90.0);
        assert_eq!(yaw(&sim), 90.0);
        // A client that subtracts the delta, as every vcod probe does, sends
        // -16384 for a view of 0 and reads 0 back: the two spectator
        // captures' zeros beside `delta_angles[1]` 16384.
        sim.step(&cmd(0, 0, -16_384 & 0xffff), 0.05, None, &[]);
        assert_eq!(yaw(&sim), 0.0);
    }

    /// The player half: a spawned client's `viewangles` is on the wire and is
    /// `SHORT2ANGLE(cmd.angles + delta_angles)` per axis, retail's
    /// `PM_UpdateViewAngles`.
    ///
    /// The numbers here are this test's own -- `become_player` at 225 degrees
    /// puts 40960 in `delta_angles[1]`, and a cmd yaw of 0 makes the sum
    /// -135.0 -- chosen so the two are not the same word and a write that
    /// echoed the delta instead of summing would fail. The relation they
    /// check is the retail one, measured off the `--save-ads` capture taken
    /// on `mp_carentan` under `tdm` with `kar98k_sniper_mp`, which reads
    /// `delta_angles[1]` 24576 and `viewangles[1]` -45.0 on every one of its
    /// 355 `!trace` lines; that capture is compared field for field by
    /// `playerstate_combat_ab`'s `the_scoped_sight_and_view_match_retail_on_mp_carentan`,
    /// and docs/protocol-1.1.md, "View angles", carries the derivation.
    ///
    /// The relation holds in every other retail player capture too, including
    /// the two whose `viewangles` reads 0 -- there the probe subtracts
    /// `delta_angles` when it builds each cmd, so the sum is zero and the
    /// field is not evidence of an unwritten one (`playerstate_ab.rs`,
    /// `check_spawn_shape`).
    ///
    /// A turn is asserted too, off the motion captures: those read the spawn
    /// yaw at every held pose and the spawn yaw plus the cmd's 60 degrees at
    /// `turn_left`/`turn_right`, so a write that pinned the spawn angle
    /// rather than summing would pass the first assert and fail this one.
    #[test]
    fn a_players_viewangles_carry_the_summed_view() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 64.0], 225.0, NULL_USERCMD.angles);
        let view = |ps: &msg::PlayerState, i: usize| {
            f32::from_bits(ps.field_i32(p, &format!("viewangles[{i}]")) as u32)
        };
        let ps = sim.to_wire(p, 0, 0);
        assert_eq!(ps.field_i32(p, "delta_angles[1]"), 40_960);
        assert_eq!(view(&ps, 1), -135.0);
        assert_eq!(view(&ps, 0), 0.0);
        assert_eq!(view(&ps, 2), 0.0);

        // Spawned facing 225 and turning 45 to the right: -135 + 45. The
        // angles are the shorts that land on a whole degree -- 8192 is 45 and
        // 4096 is 22.5 -- since ANGLE2SHORT truncates and a 60 would leave the
        // assert chasing the rounding rather than the sum.
        sim.step(&cmd(0, 0, 8192), 0.05, None, &[]);
        assert_eq!(view(&sim.to_wire(p, 0, 0), 1), -90.0);
        // Pitch travels on its own axis, positive down the way the wire reads
        // it, and the camera's own sign is the negative of it.
        sim.step(&cmd(0, 4096, 8192), 0.05, None, &[]);
        let ps = sim.to_wire(p, 0, 0);
        assert_eq!(view(&ps, 0), 22.5);
        assert_eq!(sim.ps.pitch.to_degrees(), -22.5);
    }

    /// The retail hit (combat doc, 8.4): the shooter at (1810, 2109.5), the
    /// target at (1800, 2696), a level carbine round to the head.
    fn hit_op(damage: i32, fatal: bool) -> SimOp {
        // A hair upward, as the capture's knockback (velocity z 0.1): its
        // `damagePitch` 255 is `vectoangles`' pitch just short of 360, which
        // a perfectly level shot would read as 0.
        let dir = Vec3::new(-10.0, 586.5, 0.5).normalize();
        SimOp::Damaged {
            damage,
            point: [1800.0, 2681.0, 36.1],
            dir: dir.into(),
            knockback: true,
            attacker: Some(1),
            // The shooter at (1810, 2109.5) from the target at (1800, 2696).
            dead_yaw: 270,
            fatal,
        }
    }

    /// A linked client's cmds run retail's linked arm: the frame the link
    /// lands on still reads the ground its free cmds left, every frame after
    /// reads `ENTITYNUM_NONE` on the wire and false to `isOnGround`, and a
    /// held walk moves nothing and raises no footstep (object-model doc,
    /// 23.2 and 23.5).
    #[test]
    fn a_linked_client_runs_the_linked_arm_at_pm_type_1() {
        let p = &PROTOCOL_V1;
        let w = vcod_common::collision::test_world(&[]);
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        let run = UserCmd {
            forward: 127,
            ..NULL_USERCMD
        };
        for _ in 0..10 {
            sim.step(&run, 0.05, Some(MoveWorld::bare(&w)), &[]);
        }
        assert!(sim.on_ground());
        assert_eq!(sim.wire_pm_type(), 0);
        sim.link_to = Some(Link {
            parent: vcod_gsc::EntId(200, 0),
            offset: [0.0; 3],
            velocity: [0.0; 3],
        });
        assert!(sim.on_ground(), "the link frame's cmds ran free");
        assert_eq!(sim.wire_pm_type(), PM_NORMAL_LINKED);
        let (origin, velocity) = (sim.ps.origin, sim.ps.velocity);
        assert!(velocity.length() > 100.0);
        for _ in 0..40 {
            let events = sim.step(&run, 0.05, Some(MoveWorld::bare(&w)), &[]);
            assert!(events.is_empty(), "a linked walk raised {events:?}");
        }
        assert_eq!((sim.ps.origin, sim.ps.velocity), (origin, velocity));
        assert!(!sim.on_ground());
        let ps = sim.to_wire(p, 0, 0);
        assert_eq!(
            ps.fields[msg::PlayerState::field_index(p, "groundEntityNum").unwrap()],
            ENTITYNUM_NONE as i32
        );
        sim.dead = true;
        sim.end_frame(0);
        assert_eq!(sim.wire_pm_type(), PM_DEAD_LINKED);
        // A linked corpse takes the linked arm: no fall, no slide, no snap.
        for _ in 0..10 {
            sim.step(&run, 0.05, Some(MoveWorld::bare(&w)), &[]);
        }
        assert_eq!((sim.ps.origin, sim.ps.velocity), (origin, velocity));
        assert!(!sim.on_ground());
        assert!(sim.ps.view_height() < 60.0, "the dead eye sinks");
        // A spawn unlinks: `ClientSpawn` calls `G_EntUnlink`.
        sim.become_player([0.0; 3], 0.0, NULL_USERCMD.angles);
        assert_eq!(sim.link_to, None);
    }

    /// UO's fatigue clock survives the round trip to a predictor: without
    /// `iFatigueSoundTime` a winded body breathes on every rebuild, and the
    /// client plays the shot that breath displaced twice
    /// (docs/research/coduo-movement.md 15).
    #[test]
    fn to_wire_carries_the_uo_fatigue_clock() {
        let p = &vcod_common::net::protocol::PROTOCOL_V22;
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.ps.fatigue = 0.4;
        sim.ps.last_sprint_time = 21_300;
        sim.ps.fatigue_sound_time = 21_460;
        let pred = pmove::predict::from_wire(p, &sim.to_wire(p, 0, 21_500), None);
        assert_eq!(pred.ps.fatigue, 0.4);
        assert_eq!(pred.ps.last_sprint_time, 21_300);
        assert_eq!(pred.ps.fatigue_sound_time, 21_460);
    }

    /// `to_wire` carries the knockback timer as `pm_time` plus `pm_flags`
    /// 0x100 (plan-phase read 3), and clears both once the timer is free.
    #[test]
    fn to_wire_carries_the_knockback_timer() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.ps.knockback_ms = 300.0;
        sim.ps.knockback_flags = pmove::PMF_TIME_KNOCKBACK;
        let ws = sim.to_wire(p, 0, 0);
        assert_eq!(
            ws.fields[msg::PlayerState::field_index(p, "pm_time").unwrap()],
            300
        );
        assert_ne!(
            ws.fields[msg::PlayerState::field_index(p, "pm_flags").unwrap()]
                & pmove::PMF_TIME_KNOCKBACK,
            0
        );

        sim.ps.knockback_ms = 0.0;
        sim.ps.knockback_flags = 0;
        let ws = sim.to_wire(p, 0, 0);
        assert_eq!(
            ws.fields[msg::PlayerState::field_index(p, "pm_time").unwrap()],
            0
        );
        assert_eq!(
            ws.fields[msg::PlayerState::field_index(p, "pm_flags").unwrap()]
                & pmove::PMF_TIME_KNOCKBACK,
            0
        );
    }

    fn target() -> ClientSim {
        let mut sim = ClientSim::spectator([1800.0, 2696.0, -23.9], 0.0, NULL_USERCMD.angles);
        sim.become_player([1800.0, 2696.0, -23.9], 0.0, NULL_USERCMD.angles);
        sim.ps.on_ground = true;
        sim.health = 100;
        sim.max_health = 100;
        sim
    }

    /// The feedback's pitch byte is `vectoangles`', down positive and
    /// wrapped: a frag going off under the target (`mp_carentan-tdm-
    /// grenade-target.txt`, ms 9557: knockback -86.9, -0.6, 14.0) reads
    /// `damageYaw` 128 and `damagePitch` 249, about 9 degrees up. Q3's
    /// sign read 6.
    #[test]
    fn a_blast_from_below_writes_the_capture_s_pitch_byte() {
        let p = &PROTOCOL_V1;
        let mut sim = target();
        let op = SimOp::Damaged {
            damage: 74,
            point: [1192.0, 3296.0, -23.9],
            dir: Vec3::new(-86.9, -0.6, 14.0).normalize().into(),
            knockback: true,
            attacker: Some(1),
            dead_yaw: 0,
            fatal: false,
        };
        sim.take_damage(&op, None, &mut 1, (1000, 1000));
        sim.health = 26;
        sim.end_frame(1000);
        let w = sim.to_wire(p, 0, 0);
        assert_eq!(w.field_i32(p, "damageYaw"), 128);
        assert_eq!(w.field_i32(p, "damagePitch"), 249);
    }

    /// One surviving hit writes the four feedback fields, the pain event
    /// and the knockback the capture measured; a second within 700 ms
    /// counts but raises no second `EV_PAIN`; one after does.
    #[test]
    fn a_hit_writes_the_feedback_the_capture_measured() {
        let p = &PROTOCOL_V1;
        let mut sim = target();
        sim.take_damage(&hit_op(67, false), None, &mut 1, (1000, 1000));
        sim.health = 33; // the host's mirror
        sim.end_frame(1000);
        let w = sim.to_wire(p, 0, 0);
        assert_eq!(w.field_i32(p, "damageEvent"), 1);
        assert_eq!(w.field_i32(p, "damageCount"), 67);
        assert_eq!(w.field_i32(p, "damageYaw"), 64);
        assert_eq!(w.field_i32(p, "damagePitch"), 255);
        assert_eq!(w.field_i32(p, "eventSequence"), 1);
        assert_eq!(w.field_i32(p, "events[0]"), EV_PAIN);
        assert_eq!(w.field_i32(p, "eventParms[0]"), 33);
        assert_eq!(w.field_i32(p, "pm_type"), 0);
        assert_eq!(w.health(), 33);
        assert!(
            (sim.ps.velocity.y - 80.0).abs() < 0.5 && sim.ps.velocity.x.abs() < 2.0,
            "80 u/s along the bearing, got {:?}",
            sim.ps.velocity
        );

        sim.take_damage(&hit_op(10, false), None, &mut 1, (1300, 1300));
        sim.health = 23;
        sim.end_frame(1300);
        let w = sim.to_wire(p, 0, 0);
        assert_eq!(w.field_i32(p, "damageEvent"), 2);
        assert_eq!(w.field_i32(p, "damageCount"), 10);
        assert_eq!(
            w.field_i32(p, "eventSequence"),
            1,
            "no second EV_PAIN inside 700 ms"
        );

        sim.take_damage(&hit_op(10, false), None, &mut 1, (1800, 1800));
        sim.health = 13;
        sim.end_frame(1800);
        let w = sim.to_wire(p, 0, 0);
        assert_eq!(w.field_i32(p, "eventSequence"), 2);
        assert_eq!(w.field_i32(p, "events[1]"), EV_PAIN);
        assert_eq!(w.field_i32(p, "eventParms[1]"), 13);

        // A frame with no damage changes none of the four.
        sim.end_frame(1850);
        let w = sim.to_wire(p, 0, 0);
        assert_eq!(w.field_i32(p, "damageEvent"), 3);
        assert_eq!(w.field_i32(p, "damageYaw"), 64);
    }

    /// The killing hit: `EV_DEATH` with parm 0, `pm_type` 6, the dead yaw
    /// toward the attacker in `stats[1]`, and the feedback left as the last
    /// surviving hit wrote it (combat doc, 8.4). Then the body: no input
    /// moves it, and the eye drops 9 units a frame to `deadViewHeight`.
    /// A killing hit is `player_die`'s CORPSE write at once, so the body
    /// stops blocking before the end frame zeroes it: a turret's kill lands
    /// after that frame's end-frame pass.
    #[test]
    fn a_killing_hit_leaves_a_corpse_no_mover_clips() {
        use vcod_common::collision::MASK_PLAYERSOLID;
        use vcod_common::movetrace::MASK_DEADSOLID;
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0; 3], 0.0, NULL_USERCMD.angles);
        assert_eq!(sim.body(3).map(|b| b.contents), Some(CONTENTS_BODY));
        sim.take_damage(&hit_op(100, true), None, &mut 1, (1000, 1000));
        assert_eq!(sim.contents, CONTENTS_CORPSE);
        let b = sim.body(3).expect("a corpse stays linked");
        assert_eq!(b.contents & (MASK_PLAYERSOLID | MASK_DEADSOLID), 0);
        sim.update_contents();
        assert_eq!(sim.body(3), None);
    }

    #[test]
    fn a_fatal_hit_kills_freezes_the_feedback_and_drops_the_eye() {
        let p = &PROTOCOL_V1;
        let w_test = vcod_common::collision::test_world(&[]);
        // On the test floor, with the attacker at the capture's bearing.
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.health = 100;
        sim.max_health = 100;
        let op = |fatal: bool| hit_op(67, fatal);
        for _ in 0..20 {
            sim.step(&NULL_USERCMD, 0.05, Some(MoveWorld::bare(&w_test)), &[]);
        }
        sim.take_damage(&op(false), None, &mut 1, (1000, 1000));
        sim.health = 33;
        sim.end_frame(1000);
        for _ in 0..10 {
            sim.step(&NULL_USERCMD, 0.05, Some(MoveWorld::bare(&w_test)), &[]);
        }
        assert_eq!(sim.ps.velocity.length(), 0.0, "the knockback has decayed");

        sim.take_damage(&op(true), None, &mut 1, (2000, 2000));
        sim.health = 0;
        sim.end_frame(2000);
        assert!(sim.dead);
        let w = sim.to_wire(p, 0, 0);
        assert_eq!(w.field_i32(p, "pm_type"), PM_DEAD);
        assert_eq!(w.health(), 0);
        assert_eq!(
            w.arrays.stats[1], 270,
            "the attacker sits at bearing 270.98"
        );
        assert_eq!(w.field_i32(p, "eventSequence"), 2);
        assert_eq!(w.field_i32(p, "events[1]"), EV_DEATH);
        assert_eq!(w.field_i32(p, "eventParms[1]"), 0);
        assert_eq!(
            w.field_i32(p, "damageEvent"),
            1,
            "the killing hit runs no feedback"
        );
        assert_eq!(w.field_i32(p, "damageCount"), 67);
        assert_eq!(w.field_i32(p, "deadViewHeight"), 8);
        assert_eq!(w.field_f32(p, "viewHeightCurrent"), 60.0);
        assert_eq!(
            w.field_i32(p, "torsoAnim"),
            512,
            "the torso restarts on nothing"
        );

        let run = UserCmd {
            forward: 127,
            buttons: msg::BUTTON_ATTACK,
            ..NULL_USERCMD
        };
        for expect in [51.0, 42.0, 33.0, 24.0, 15.0, 8.0, 8.0] {
            let events = sim.step(&run, 0.05, Some(MoveWorld::bare(&w_test)), &[]);
            assert!(events.is_empty(), "a dead player fires nothing");
            assert_eq!(
                sim.to_wire(p, 0, 0).field_f32(p, "viewHeightCurrent"),
                expect
            );
        }
        // The body slid on the killing hit's knockback and friction has
        // stopped it; from here only input could move it, and none does.
        assert_eq!(sim.ps.velocity.truncate(), glam::Vec2::ZERO);
        let before = sim.ps.origin;
        for _ in 0..5 {
            sim.step(&run, 0.05, Some(MoveWorld::bare(&w_test)), &[]);
        }
        // Only in the last bits of z: a still trace's end point carries the
        // rounding of the box-centre shift, as retail's does.
        assert_eq!(
            sim.ps.origin.truncate(),
            before.truncate(),
            "input does not move a body"
        );
        assert!((sim.ps.origin.z - before.z).abs() < 1e-5);

        // A respawn clears all of it.
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        let w = sim.to_wire(p, 0, 0);
        assert!(!sim.dead);
        assert_eq!(w.field_i32(p, "pm_type"), 0);
        assert_eq!(w.field_i32(p, "damageEvent"), 0);
        assert_eq!(w.arrays.stats[1], 0);
        // Retail's first frame of a new life reads an empty ring at sequence
        // 0 (combat doc, 9.2).
        assert_eq!(
            w.field_i32(p, "eventSequence"),
            0,
            "the respawn did not clear the ring"
        );
        assert_eq!(w.field_i32(p, "events[0]"), 0);
    }

    /// No direction is the 255/255 sentinel, and no knockback moves nothing.
    #[test]
    fn a_hit_with_no_direction_marks_both_angles_255() {
        let p = &PROTOCOL_V1;
        let mut sim = target();
        let op = SimOp::Damaged {
            damage: 20,
            point: [0.0; 3],
            dir: [0.0; 3],
            knockback: false,
            attacker: None,
            dead_yaw: 0,
            fatal: false,
        };
        sim.take_damage(&op, None, &mut 1, (500, 500));
        sim.end_frame(500);
        let w = sim.to_wire(p, 0, 0);
        assert_eq!(w.field_i32(p, "damageYaw"), 255);
        assert_eq!(w.field_i32(p, "damagePitch"), 255);
        assert_eq!(w.field_i32(p, "damageCount"), 20);
        assert_eq!(sim.ps.velocity, Vec3::ZERO);
    }

    #[test]
    fn set_view_angle_rewrites_delta_angles_against_the_last_cmd() {
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.last_cmd_angles = [100, 2000, 0];
        sim.set_view_angle([10.0, 90.0, 0.0]);
        let d = sim.delta_angles();
        assert_eq!(d[0], (10.0 * ANGLE2SHORT) as i32 - 100);
        assert_eq!(d[1], (90.0 * ANGLE2SHORT) as i32 - 2000);
        assert_eq!(sim.view_angles()[1], 90.0);
    }

    /// `SetClientViewAngle`'s prone arm (0x41e30): a prone player is
    /// handed a view no further than 45 degrees off his body, placed on the
    /// shorts grid, and `delta_angles` follows the held view.
    #[test]
    fn set_view_angle_holds_a_prone_player_inside_the_cone() {
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.become_player([0.0; 3], 0.0, [0; 3]);
        sim.ps.stance = vcod_common::pmove::Stance::Prone;
        sim.ps.prone_direction = 10.0;
        sim.set_view_angle([0.0, 100.0, 0.0]);
        // AngleNormalize360(10 + 45): 55 degrees is 10012.44 shorts.
        assert_eq!(sim.delta_angles()[1], 10012);
        assert_eq!(sim.view_angles()[1], 10012.0 * (360.0 / 65536.0));
    }

    /// `step` keeps the cmd's angles, so the view `set_view_angle` wrote is
    /// what the next cmd carrying the same angles derives.
    #[test]
    fn a_forced_view_survives_the_next_cmd() {
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        let c = cmd(0, 1234, -5678);
        sim.step(&c, 0.05, None, &[]);
        sim.set_view_angle([-20.0, 135.0, 0.0]);
        sim.step(&c, 0.05, None, &[]);
        let v = sim.view_angles();
        assert!((v[0] + 20.0).abs() < 0.01, "{v:?}");
        assert!((v[1] - 135.0).abs() < 0.01, "{v:?}");
    }

    #[test]
    fn a_mounted_client_writes_the_view_lock_and_stance_eflags() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.become_player([0.0; 3], 0.0, [0; 3]);
        sim.mounted_on = Some((298, TurretStance::Stand));
        sim.viewlocked = 1;
        sim.viewlocked_ent = 298;
        let ps = sim.to_wire(p, 0, 0);
        assert_eq!(ps.field_i32(p, "viewlocked"), 1);
        assert_eq!(ps.field_i32(p, "viewlocked_entNum"), 298);
        assert_eq!(ps.field_i32(p, "eFlags") & 0xC000, 0xC000);
        let e = sim.to_entity(p, 3, 0);
        assert_eq!(e.field_i32(p, "eFlags") & 0xC000, 0xC000);
        assert_eq!(e.field_i32(p, "otherEntityNum"), 298);

        sim.mounted_on = Some((298, TurretStance::Duck));
        assert_eq!(sim.to_wire(p, 0, 0).field_i32(p, "eFlags") & 0xC000, 0x8000);
        sim.mounted_on = Some((298, TurretStance::Prone));
        assert_eq!(sim.to_wire(p, 0, 0).field_i32(p, "eFlags") & 0xC000, 0x4000);
    }

    /// A rider's entity goes out as retail's capture has it
    /// (`snapshots-uo-jeep.bin`): `pos.trType` 1 at the origin truncated,
    /// `apos.trType` 1 (docs/research/coduo-vehicles.md 11.4).
    #[test]
    fn a_rider_travels_as_an_interpolated_point() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.become_player([-4156.97, 118.48, -80.32], 0.0, [0; 3]);
        sim.seat = Some(Seat {
            vehicle: 110,
            pos: 2,
            kind: 1,
            motion: 1,
            angles: [0.0; 3],
            hull_view: false,
            overheated: false,
            link_offset: [0.0; 3],
        });
        let e = sim.to_entity(p, 3, 0);
        assert_eq!(e.field_i32(p, "pos.trType"), trajectory::TR_INTERPOLATE);
        assert_eq!(e.field_i32(p, "apos.trType"), trajectory::TR_INTERPOLATE);
        let base = [0, 1, 2].map(|i| e.field_f32(p, &format!("pos.trBase[{i}]")));
        assert_eq!(base, [-4156.0, 118.0, -80.0]);
        sim.seat = None;
        let e = sim.to_entity(p, 3, 0);
        assert_eq!(e.field_i32(p, "pos.trType"), trajectory::TR_LINEAR_STOP);
    }

    /// A player on foot goes out at his origin truncated toward zero, as
    /// every retail capture has it (docs/protocol-1.1.md, "A player's
    /// `pos.trBase` is whole units").
    #[test]
    fn a_player_entity_carries_whole_units() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.become_player([-4156.97, 118.48, -80.32], 0.0, [0; 3]);
        let e = sim.to_entity(p, 3, 0);
        let base = [0, 1, 2].map(|i| e.field_f32(p, &format!("pos.trBase[{i}]")));
        assert_eq!(base, [-4156.0, 118.0, -80.0]);
    }

    /// A refused prone press reaches the wire as `pm_flags` 0x8000, which
    /// the client's "Prone Blocked" notice reads.
    #[test]
    fn a_refused_prone_writes_pm_flags_0x8000() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.become_player([0.0; 3], 0.0, [0; 3]);
        let pm_flags = |sim: &ClientSim| sim.to_wire(p, 0, 0).field_i32(p, "pm_flags");
        assert_eq!(pm_flags(&sim) & PMF_PRONE_BLOCKED, 0);
        sim.ps.prone_blocked = true;
        assert_eq!(pm_flags(&sim) & PMF_PRONE_BLOCKED, PMF_PRONE_BLOCKED);
    }

    /// `BG_PlayerStateToEntityState` copies `ps.eFlags` whole, stance bits
    /// included, and adds 0x200 for the sight flag on the entity alone
    /// (combat doc 16.4): the drawing client's torso yaw reads both.
    #[test]
    fn the_entity_carries_the_stance_bits_and_the_sight_flag() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.become_player([0.0; 3], 0.0, [0; 3]);
        sim.ps.stance = pmove::Stance::Crouch;
        sim.ps.ads_active = true;
        let e = sim.to_entity(p, 3, 0).field_i32(p, "eFlags");
        assert_eq!(e & (EF_CROUCH | vcod_common::playerpose::EF_ADS), 0x220);
        assert_eq!(sim.to_wire(p, 0, 0).field_i32(p, "eFlags") & 0x200, 0);
    }

    /// UO's bipod deploy is the sight flag on an LMG, so the deploy frame's
    /// entity carries the flag's bit, UO's 0x100 (`BG_PlayerStateToEntityState`,
    /// game.mp.uo 0x2cd8b; the mg capture's 160 on entity 1 reads 0x230 in
    /// 1.1's layout).
    #[test]
    fn a_uo_deploy_frame_carries_the_sight_bit_on_the_entity() {
        let p = &vcod_common::net::protocol::PROTOCOL_V22;
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.become_player([0.0; 3], 0.0, [0; 3]);
        sim.ps.rules = pmove::MoveRules::Uo;
        sim.ps.lmg = true;
        sim.ps.ads_active = true;
        sim.ps.weaponstate = vcod_common::pmove::weapon::WEAPON_DEPLOY;
        assert!(sim.ps.deployed());
        let e = sim.to_entity(p, 3, 0).field_i32(p, "eFlags");
        assert_eq!(
            e & vcod_common::playerpose::EF_ADS,
            vcod_common::playerpose::EF_ADS
        );
        assert_eq!(vcod_common::net::flags::eflags_v1_to_v22(e) & 0x100, 0x100);
    }

    /// `PmoveSingle`'s 0x400 (combat doc 16.5): the attack bit with the
    /// weapon ready or firing and a loaded clip, never with the talk bit.
    #[test]
    fn the_fire_bit_needs_the_trigger_a_ready_weapon_and_a_clip() {
        use vcod_common::pmove::weapon::{WEAPON_FIRING, WEAPON_READY, WEAPON_RELOADING};
        let weapons = vec![
            None,
            Some(WeaponDef {
                clip_index: 3,
                ..WeaponDef::default()
            }),
        ];
        let mut ps = pmove::PlayerState::spawn(glam::Vec3::ZERO, 0.0);
        ps.weapon = 1;
        ps.ammoclip[3] = 5;
        let attack = msg::BUTTON_ATTACK;
        for (state, want) in [
            (WEAPON_READY, true),
            (WEAPON_FIRING, true),
            (WEAPON_RELOADING, false),
        ] {
            ps.weaponstate = state;
            assert_eq!(attack_flag(&ps, attack, &weapons), want, "state {state}");
        }
        ps.weaponstate = WEAPON_READY;
        assert!(!attack_flag(&ps, 0, &weapons));
        assert!(!attack_flag(&ps, attack | msg::BUTTON_TALK, &weapons));
        ps.ammoclip[3] = 0;
        assert!(!attack_flag(&ps, attack, &weapons));
    }

    /// `PmoveSingle`'s only `pm_type` test for the bit is against 5, so a
    /// dead body's first cmd, which still holds the weapon, sets it; the
    /// dead arm takes the weapon away, and the next cmd clears it.
    #[test]
    fn a_dead_body_s_first_cmd_keeps_the_fire_bit() {
        use vcod_common::pmove::weapon::WEAPON_READY;
        let weapons = vec![
            None,
            Some(WeaponDef {
                clip_index: 3,
                ..WeaponDef::default()
            }),
        ];
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.become_player([0.0; 3], 0.0, [0; 3]);
        sim.step(&NULL_USERCMD, 0.008, None, &weapons);
        sim.ps.weapon = 1;
        sim.ps.ammoclip[3] = 5;
        sim.ps.weaponstate = WEAPON_READY;
        sim.die();
        sim.pm_dead = true;
        let fire = UserCmd {
            buttons: msg::BUTTON_ATTACK,
            ..NULL_USERCMD
        };
        sim.step(&fire, 0.008, None, &weapons);
        assert!(sim.attacking);
        sim.step(&fire, 0.008, None, &weapons);
        assert!(!sim.attacking);
    }

    /// The bit rides the entity and the playerstate both, off the last cmd
    /// of the frame.
    #[test]
    fn a_held_trigger_sets_the_fire_bit_on_the_wire() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.become_player([0.0; 3], 0.0, [0; 3]);
        sim.attacking = true;
        assert_eq!(
            sim.to_entity(p, 3, 0).field_i32(p, "eFlags") & EF_FIRING,
            EF_FIRING
        );
        assert_eq!(
            sim.to_wire(p, 0, 0).field_i32(p, "eFlags") & EF_FIRING,
            EF_FIRING
        );
        sim.become_player([0.0; 3], 0.0, [0; 3]);
        assert_eq!(sim.to_wire(p, 0, 0).field_i32(p, "eFlags") & EF_FIRING, 0);
    }

    /// The talk bit sets `eFlags` 0x40000 for the cmd and stops the body
    /// (the talk captures read 262168 while it is held, cod15-movement.md 5).
    #[test]
    fn a_talking_cmd_sets_the_talk_bit_and_moves_nothing() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.become_player([0.0; 3], 0.0, [0; 3]);
        let talk = UserCmd {
            buttons: msg::BUTTON_TALK,
            forward: 127,
            ..NULL_USERCMD
        };
        sim.step(&talk, 0.05, None, &[]);
        assert_eq!(
            sim.to_wire(p, 0, 0).field_i32(p, "eFlags") & EF_TALK,
            EF_TALK
        );
        assert_eq!(sim.ps.origin, glam::Vec3::ZERO);
        sim.step(&NULL_USERCMD, 0.05, None, &[]);
        assert_eq!(sim.to_wire(p, 0, 0).field_i32(p, "eFlags") & EF_TALK, 0);
    }

    /// `ClientSpawn`'s memset: the capture's first trace, a fresh spawn,
    /// reads `viewlocked_entNum` 0 (turrets doc, 12.7).
    #[test]
    fn a_respawn_clears_the_mount() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.become_player([0.0; 3], 0.0, [0; 3]);
        sim.mounted_on = Some((298, TurretStance::Stand));
        sim.viewlocked = 2;
        sim.viewlocked_ent = 298;
        sim.gunfx = 1;
        sim.ps.mounted = Some(pmove::Stance::Stand);
        sim.become_player([0.0; 3], 0.0, [0; 3]);
        assert_eq!(sim.mounted_on, None);
        assert_eq!(sim.ps.mounted, None);
        let w = sim.to_wire(p, 0, 0);
        assert_eq!(w.field_i32(p, "viewlocked"), 0);
        assert_eq!(w.field_i32(p, "viewlocked_entNum"), 0);
        assert_eq!(w.field_i32(p, "gunfx"), 0);
        assert_eq!(w.field_i32(p, "eFlags") & 0xC000, 0);
        assert_eq!(sim.to_entity(p, 3, 0).field_i32(p, "otherEntityNum"), 0);
    }

    /// `TeleportPlayer` (turrets doc, section 8 and 12.7): 200 at the old
    /// origin and 199 at the destination on temp entities naming the
    /// client, the origin a unit above the destination, the bit flipped and
    /// the view set.
    /// `setOrigin` links at the unsnapped origin, and that link holds until
    /// the next cmd relinks at the snapped one (combat doc 14.3).
    #[test]
    fn a_set_origin_links_unsnapped_until_the_next_cmd() {
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.become_player([5.7, -3.2, 8.9], 0.0, [0; 3]);
        assert_eq!(sim.link_origin(), Vec3::new(5.0, -3.0, 8.0));
        sim.teleport([10.5, 0.25, -22.9]);
        assert_eq!(sim.link_origin(), Vec3::new(10.5, 0.25, -22.9));
        sim.step(&NULL_USERCMD, 0.05, None, &[]);
        assert_eq!(sim.link_origin(), sim.ps.origin.trunc());
    }

    #[test]
    fn teleport_player_raises_out_and_in_and_flips_the_bit() {
        use crate::game::temp_entity::Scope;
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.become_player([5.7, -3.2, 8.9], 0.0, [0; 3]);
        let bit = sim.eflags() & EF_TELEPORT_BIT;
        let temps = sim.teleport_player(4, [10.5, 0.0, -23.9], [0.0, 45.0, 0.0]);
        assert_eq!(sim.ps.origin, Vec3::new(10.5, 0.0, -22.9));
        assert_ne!(sim.eflags() & EF_TELEPORT_BIT, bit);
        assert_eq!(sim.view_angles()[1], 45.0);
        let got: Vec<_> = temps
            .iter()
            .map(|t| (t.event, t.origin, t.client_num, t.scope))
            .collect();
        assert_eq!(
            got,
            vec![
                (200, [5.7, -3.2, 8.9], 4, Scope::Pvs),
                (199, [10.5, 0.0, -23.9], 4, Scope::Pvs),
            ]
        );

        // A client not playing moves and flips without either event.
        sim.dead = true;
        assert!(sim.teleport_player(4, [0.0; 3], [0.0; 3]).is_empty());
        assert_eq!(sim.ps.origin.z, 1.0);
    }

    /// A server whose map failed to load still flies its spectators, with no
    /// collision world to stop them.
    #[test]
    fn step_flies_without_a_collision_world() {
        let mut sim = ClientSim::spectator([1.0, 2.0, 3.0], 0.0, NULL_USERCMD.angles);
        let c = cmd(127, 0, 0);
        for _ in 0..125 {
            sim.step(&c, 1.0 / 125.0, None, &[]);
        }
        assert!(
            sim.ps.origin.x > 1.0,
            "yaw 0 faces +X, origin {:?}",
            sim.ps.origin
        );
        assert!(
            sim.ps.velocity.truncate().length() > 250.0,
            "and accelerates to spectator speed, v {:?}",
            sim.ps.velocity
        );
    }

    /// Retail's spectator on mp_carentan dm, flying +x from the intermission
    /// spot (`tools/offlan_capture.sh`, 2026-10-12): it steps 8.125 onto a
    /// window sill with `EV_STEP_VIEW` 136 and `bobCycle` 12, and stops
    /// against the caulk behind the shutters at 1019.875 with a velocity of
    /// (0, 0, -0) (docs/protocol-1.1.md, "Spectator").
    #[test]
    fn a_spectator_steps_onto_the_sill_retail_s_did() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            return;
        };
        let path = fs.resolve_map("mp_carentan").expect("map in the paks");
        let bsp = vcod_common::bsp::parse(&fs.read(&path).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let mut sim = ClientSim::spectator([384.0, -624.0, 184.0], 90.0, NULL_USERCMD.angles);
        // The probe's view is yaw 0 against the spawn's delta of 90.
        let forward = UserCmd {
            forward: 127,
            angles: [0, -16384 & 0xffff, 0],
            ..NULL_USERCMD
        };
        for _ in 0..200 {
            sim.step(
                &forward,
                0.016,
                Some(MoveWorld::bare(&world.collision)),
                &[],
            );
        }
        assert_eq!(sim.ps.origin, Vec3::new(1019.875, -624.0, 192.125));
        assert_eq!(sim.ps.bob_cycle, 12);
        // Pressed into the caulk the velocity is the crease of two opposed
        // planes, which retail's snapshots carry as (0, 0, -0).
        let bits = sim.ps.velocity.to_array().map(f32::to_bits);
        assert_eq!(bits, [0, 0, 0x8000_0000]);
        let r = sim.ring;
        let step = vcod_common::net::event_ids::EV_STEP_VIEW;
        assert_eq!((r.seq, r.events[0], r.parms[0]), (1, step, 136));
    }
}
