//! CoD2 1.3's player movement: `Pmove` and `PmoveSingle` of `cod2_lnxded`
//! (docs/research/cod2-movement.md), a path of its own beside vcod-common's
//! CoD1 `pmove` rather than a `MoveRules` arm in it. The usercmd, the
//! playerstate, the flag numbering, the clip, the slide's plane order and
//! the float model all differ from CoD1's (the doc's section 15), so the
//! two share only the collision world they trace through.
//!
//! Arithmetic is the Linux server's: every float assignment stored
//! ([`fpu`]). The Windows client's MSVC build keeps intermediates, so a
//! client replay of a server capture will correct in the last bits.
//!
//! The weapon half (`PM_Weapon`, the
//! sight flag and lerp, the aim spread) is [`weapon`], which the caller runs
//! beside the step; the legs and torso anims are [`anim`].

pub mod aim;
pub mod anim;
mod footsteps;
mod fpu;
mod ground;
mod ladder;
pub mod mantle;
mod moves;
mod slide;
mod stance;
pub mod weapon;
mod wire;

use glam::Vec3;
use vcod_common::collision::Prim;
use vcod_common::movetrace::{CONTENTS_BODY, MoveWorld};

use crate::collision::CONTENTS_FOLIAGE;
use vcod_common::net::protocol::ENTITYNUM_WORLD;

use crate::net::UserCmd2;

pub use anim::{AnimInput, AnimTable};
pub use slide::clip_velocity;

/// `Vec3NormalizeTo` (0x80a45fc) as `cod2_lnxded` rounds it: the length
/// and its reciprocal stored, each component stored; a zero vector zeroed.
pub fn vec3_normalize_to(v: Vec3) -> Vec3 {
    fpu::normalize_to(v).0
}

/// `groundEntityNum` off the ground.
pub const ENTITYNUM_NONE: i32 = 1023;

/// `pm_type` (docs/research/cod2-movement.md 4; names zk_libcod's).
pub mod pm_type {
    pub const NORMAL: i32 = 0;
    pub const NORMAL_LINKED: i32 = 1;
    pub const NOCLIP: i32 = 2;
    pub const UFO: i32 = 3;
    pub const SPECTATOR: i32 = 4;
    pub const INTERMISSION: i32 = 5;
    pub const DEAD: i32 = 6;
    pub const DEAD_LINKED: i32 = 7;
}

/// `pm_flags` (docs/research/cod2-movement.md 12).
pub mod pmf {
    pub const PRONE: u32 = 0x1;
    pub const DUCKED: u32 = 0x2;
    pub const MANTLE: u32 = 0x4;
    pub const LADDER: u32 = 0x20;
    /// Sighted; `PM_ExitAimDownSight` clears it.
    pub const SIGHT: u32 = 0x40;
    pub const BACKWARDS_RUN: u32 = 0x80;
    /// The sighted walk: the 0.4 move scale.
    pub const SIGHT_WALK: u32 = 0x100;
    /// The landing stun's timer: friction x0.3, accel x0.25.
    pub const TIME_LAND: u32 = 0x200;
    /// The knockback timer: no friction, accel 1, gravity on the ground.
    pub const TIME_KNOCKBACK: u32 = 0x400;
    /// Sighted while prone: the move keys are zeroed.
    pub const PRONE_SIGHT: u32 = 0x800;
    pub const RESPAWNED: u32 = 0x1000;
    pub const FROZEN: u32 = 0x8000;
    /// A stance change or a prone turn refused this cmd.
    pub const REFUSED: u32 = 0x10000;
    pub const LADDER_LEFT: u32 = 0x40000;
    /// The jump and its landing slowdown's timer.
    pub const JUMPING: u32 = 0x80000;
}

/// `usercmd_s.buttons` (docs/research/cod2-movement.md 3).
pub mod button {
    pub const ATTACK: i32 = 0x1;
    pub const LEAN_LEFT: i32 = 0x40;
    pub const LEAN_RIGHT: i32 = 0x80;
    pub const PRONE: i32 = 0x100;
    pub const CROUCH: i32 = 0x200;
    pub const JUMP: i32 = 0x400;
    pub const SHELLSHOCK: i32 = 0x800;
    pub const ADS: i32 = 0x1000;
    /// Kept beside the stance keys by the frozen and talk masks, and
    /// silences `PM_CheckDuck`'s refused-stance events (0x80e4644). Its name
    /// is INFERRED: 1.1's held-stance bit plays the same part.
    pub const STANCE_HELD: i32 = 0x2000;
    /// `eFlags` 0x200000 follows it (`PmoveSingle` 0x80e8934): the console,
    /// a menu or the chat line.
    pub const TALK: i32 = 0x40000;
}

/// `eFlags` bits pmove writes.
pub mod ef {
    pub const CROUCH: u32 = 0x4;
    pub const PRONE: u32 = 0x8;
    /// A turret's stance bits; any set makes the player a gunner.
    pub const TURRET: u32 = 0x300;
    /// `EF_FIRING`: `PmoveSingle` sets it on a cmd that holds the attack
    /// with rounds in the clip (cod2-turrets.md 6).
    pub const FIRING: u32 = 0x40;
    /// Mantling (`Mantle_CheckLedge` sets it, `Mantle_Move` clears it).
    pub const MANTLE: u32 = 0x4000;
    pub const TALK: u32 = 0x200000;
}

/// The events pmove raises, numbered as CoD2's table (cod2-movement.md 12).
pub mod ev {
    pub const FOOTSTEP_RUN: i32 = 1;
    pub const FOOTSTEP_WALK: i32 = 24;
    pub const FOOTSTEP_PRONE: i32 = 47;
    pub const JUMP: i32 = 70;
    pub const JUMP_LADDER: i32 = 91;
    pub const LANDING: i32 = 93;
    pub const LANDING_PAIN: i32 = 116;
    pub const STANCE_FORCE_STAND: i32 = 140;
    pub const STANCE_FORCE_CROUCH: i32 = 141;
    pub const STANCE_FORCE_PRONE: i32 = 142;
    pub const FOLIAGE_SOUND: i32 = 139;
    pub const STEP_VIEW: i32 = 143;
}

/// `ClientThink_real`'s `pm.tracemask` (0x80f5d2e): alive.
pub const MASK_PLAYERSOLID: u32 = 0x2810011;
/// ... and dead.
pub const MASK_DEADSOLID: u32 = 0x810011;
/// `SpectatorThink` (0x80f4eb2).
pub const MASK_SPECTATOR: u32 = 0x800811;

/// The player box `.data` 0x8187ba0 and 0x8187bac.
pub const PLAYER_MINS: Vec3 = Vec3::new(-15.0, -15.0, 0.0);
pub const PLAYER_MAXS: Vec3 = Vec3::new(15.0, 15.0, 70.0);

/// The eye's stance targets (immediates in `PM_CheckDuck`).
pub const VIEW_STAND: i32 = 60;
pub const VIEW_CROUCH: i32 = 40;
pub const VIEW_PRONE: i32 = 11;
pub const VIEW_DEAD: i32 = 8;

/// The `bg_`, `jump_`, `player_`, `inertia` and `mantle_` dvars pmove reads,
/// at their `*_RegisterDvars` defaults. A server sends its own (flag 0x100),
/// so a client predicts with these as the server set them
/// (docs/research/cod2-movement.md 13).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rules {
    pub friction: f32,
    pub stopspeed: f32,
    pub inertia_max: f32,
    pub inertia_angle: f32,
    pub jump_height: f32,
    pub jump_step_size: f32,
    pub jump_slowdown_enable: bool,
    pub jump_ladder_push_vel: f32,
    pub jump_spread_add: f32,
    /// `bg_ladder_yawcap`: how far a climber off the ground may look away
    /// from the ladder.
    pub ladder_yawcap: f32,
    pub back_speed_scale: f32,
    pub strafe_speed_scale: f32,
    pub spectate_speed_scale: f32,
    pub view_pitch_up: f32,
    pub view_pitch_down: f32,
    pub prone_yawcap: f32,
    pub fall_damage_min_height: f32,
    pub fall_damage_max_height: f32,
    /// `player_moveThreshhold`: under it `PM_Footsteps` idles.
    pub move_threshold: f32,
    /// `player_footstepsThreshhold`: the slowest run that makes a footstep.
    pub footsteps_threshold: f32,
    /// `player_dmgtimer_stumbleTime` (500): how long past a hit the
    /// stumbles and flinches play.
    pub stumble_time: i32,
    pub mantle_enable: bool,
    /// `mantle_check_range`: how far past the body a wall is looked for.
    pub mantle_check_range: f32,
    /// `mantle_check_radius`: the wall probe's half width.
    pub mantle_check_radius: f32,
    /// `mantle_check_angle`: how far off the wall the view may face.
    pub mantle_check_angle: f32,
    /// `mantle_view_yawcap`: how far off the mantle's yaw the view may turn.
    pub mantle_view_yawcap: f32,
    /// The `PLAYER_MANTLE` anims ([`mantle::anims`]); no mantle without.
    pub mantle_anims: Option<&'static mantle::MantleAnims>,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            friction: 5.5,
            stopspeed: 100.0,
            inertia_max: 50.0,
            inertia_angle: 0.0,
            jump_height: 39.0,
            jump_step_size: 18.0,
            jump_slowdown_enable: true,
            jump_ladder_push_vel: 128.0,
            jump_spread_add: 64.0,
            ladder_yawcap: 100.0,
            back_speed_scale: 0.7,
            strafe_speed_scale: 0.8,
            spectate_speed_scale: 2.0,
            view_pitch_up: 85.0,
            view_pitch_down: 85.0,
            prone_yawcap: 85.0,
            fall_damage_min_height: 256.0,
            fall_damage_max_height: 480.0,
            move_threshold: 10.0,
            footsteps_threshold: 0.0,
            stumble_time: 500,
            mantle_enable: true,
            mantle_check_range: 20.0,
            mantle_check_radius: f32::from_bits(0x3dcc_cccd),
            mantle_check_angle: 60.0,
            mantle_view_yawcap: 60.0,
            mantle_anims: None,
        }
    }
}

/// The part of `playerState_s` pmove reads and writes, under the retail
/// names (cod2-movement.md 3 has the offsets). Angles are degrees, wire
/// convention (pitch positive down).
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerState {
    pub command_time: i32,
    pub pm_type: i32,
    pub bob_cycle: i32,
    pub pm_flags: u32,
    pub pm_time: i32,
    pub origin: Vec3,
    pub velocity: Vec3,
    /// The inertia clamp's reference, eased toward the velocity each step;
    /// not on the wire.
    pub old_velocity: [f32; 2],
    pub gravity: i32,
    pub leanf: f32,
    pub speed: i32,
    pub delta_angles: [i32; 3],
    pub ground_entity_num: i32,
    /// `vLadderVec`: the normal of the ladder the body holds.
    pub ladder_vec: Vec3,
    pub jump_time: i32,
    pub jump_origin_z: f32,
    pub movement_dir: i32,
    pub e_flags: u32,
    pub event_sequence: i32,
    pub events: [i32; 4],
    pub event_parms: [i32; 4],
    pub client_num: u32,
    pub weapon_state: i32,
    pub view_angles: [f32; 3],
    pub view_height_target: i32,
    pub view_height_current: f32,
    pub view_height_lerp_time: i32,
    pub view_height_lerp_target: i32,
    pub view_height_lerp_down: bool,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub prone_direction: f32,
    pub prone_direction_pitch: f32,
    pub prone_torso_pitch: f32,
    /// `BG_CheckProneValid`'s three outputs (ps 0x5a8..0x5b0): the first is
    /// always written 0, then the torso's and the legs' pitch.
    pub prone_valid: [f32; 3],
    pub aim_spread_scale: f32,
    pub damage_timer: i32,
    /// `viewAngleClampBase` and `viewAngleClampRange` (ps 0x10c, 0x114),
    /// pitch then yaw: a gunner's view is held `range` either side of
    /// `base` (`turret_use` writes them, cod2-turrets.md 4).
    pub view_angle_clamp_base: [f32; 2],
    pub view_angle_clamp_range: [f32; 2],
    /// ps+0x94 and ps+0x98: the `damageTimer` the last hit set, and the
    /// hit's yaw off the view, which `PM_Footsteps` stumbles and flinches by.
    pub damage_duration: i32,
    pub flinch_yaw: i32,
    pub mantle: mantle::MantleState,
    /// ps+0x78..0x84: the anims [`anim`] picks, each with the restart
    /// toggle 0x200, and the timers an event anim holds its channel by.
    pub legs_timer: i32,
    pub legs_anim: i32,
    pub torso_timer: i32,
    pub torso_anim: i32,
    /// The `MOVETYPE` and `STRAFING` conditions, which retail keeps per
    /// client beside the playerstate (`bgs` + 0xb4058 + clientNum * 0x4b8)
    /// and only the anim calls write; not on the wire.
    pub anim_movetype: u8,
    pub anim_strafing: u8,
    /// ps+0x44: the cmd time of the last `EV_FOLIAGE_SOUND`, 0 once the body
    /// slowed past the reset interval ([`foliage_sound`]).
    pub foliage_sound_time: i32,
    /// A prone move cut the melee's start or the binoculars off
    /// ([`prone_move_interrupts`]): `PM_Weapon` owes the `WeaponState` half
    /// of the interrupt. Not on the wire.
    pub weapon_interrupt: bool,
}

impl Default for PlayerState {
    fn default() -> Self {
        PlayerState {
            command_time: 0,
            pm_type: pm_type::NORMAL,
            bob_cycle: 0,
            pm_flags: 0,
            pm_time: 0,
            origin: Vec3::ZERO,
            velocity: Vec3::ZERO,
            old_velocity: [0.0; 2],
            gravity: 800,
            leanf: 0.0,
            speed: 190,
            delta_angles: [0; 3],
            ground_entity_num: ENTITYNUM_NONE,
            ladder_vec: Vec3::ZERO,
            jump_time: 0,
            jump_origin_z: 0.0,
            movement_dir: 0,
            e_flags: 0,
            event_sequence: 0,
            events: [0; 4],
            event_parms: [0; 4],
            client_num: 0,
            weapon_state: 0,
            view_angles: [0.0; 3],
            view_height_target: VIEW_STAND,
            view_height_current: VIEW_STAND as f32,
            view_height_lerp_time: 0,
            view_height_lerp_target: 0,
            view_height_lerp_down: false,
            mins: PLAYER_MINS,
            maxs: PLAYER_MAXS,
            prone_direction: 0.0,
            prone_direction_pitch: 0.0,
            prone_torso_pitch: 0.0,
            prone_valid: [0.0; 3],
            aim_spread_scale: 0.0,
            damage_timer: 0,
            view_angle_clamp_base: [0.0; 2],
            view_angle_clamp_range: [0.0; 2],
            damage_duration: 0,
            flinch_yaw: 0,
            mantle: mantle::MantleState::default(),
            legs_timer: 0,
            legs_anim: 0,
            torso_timer: 0,
            torso_anim: 0,
            anim_movetype: 0,
            anim_strafing: 0,
            foliage_sound_time: 0,
            weapon_interrupt: false,
        }
    }
}

impl PlayerState {
    /// A living player standing at `origin`, its clock at `time`.
    pub fn spawn(origin: Vec3, yaw: f32, time: i32) -> PlayerState {
        let mut ps = PlayerState {
            command_time: time,
            origin,
            ..Default::default()
        };
        ps.view_angles[1] = yaw;
        ps
    }

    /// `BG_AddPredictableEventToPlayerstate` (0x80dfc78): event 0 is
    /// dropped, the parm is a byte, the sequence counts on.
    pub fn add_event(&mut self, event: i32, parm: i32) {
        if event == 0 {
            return;
        }
        let slot = (self.event_sequence & 3) as usize;
        self.events[slot] = event & 0xff;
        self.event_parms[slot] = parm & 0xff;
        self.event_sequence += 1;
    }

    /// `PM_GetEffectiveStance` (0x80e1532): off the eye's target, 40 crouch
    /// and 11 prone.
    pub fn effective_stance(&self) -> Stance {
        match self.view_height_target {
            VIEW_CROUCH => Stance::Crouch,
            VIEW_PRONE => Stance::Prone,
            _ => Stance::Stand,
        }
    }

    pub fn on_ground(&self) -> bool {
        self.ground_entity_num != ENTITYNUM_NONE
    }

    /// The box `PM_CheckDuck` traces with for the stance `pm_flags` hold:
    /// `maxs` keeps the standing height, the duck cuts it to 50 or 30.
    pub fn stance_box(&self) -> (Vec3, Vec3) {
        let mut maxs = self.maxs;
        if self.pm_flags & pmf::PRONE != 0 {
            maxs.z = stance::HEIGHT_PRONE;
        } else if self.pm_flags & pmf::DUCKED != 0 {
            maxs.z = stance::HEIGHT_CROUCH;
        }
        (self.mins, maxs)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stance {
    Stand,
    Crouch,
    Prone,
}

/// A trace as CoD2's pmove reads it: no end point.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Tr {
    pub fraction: f32,
    pub normal: Vec3,
    pub surface_flags: u32,
    pub entity_num: u32,
    pub startsolid: bool,
    pub allsolid: bool,
    /// The contact is a body (`contents & CONTENTS_BODY`).
    pub body: bool,
}

/// `pml_t` (STABS, 124 bytes): what one `PmoveSingle` keeps beside the
/// playerstate.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Pml {
    pub forward: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub frametime: f32,
    pub msec: i32,
    pub walking: bool,
    pub ground_plane: bool,
    pub almost_ground_plane: bool,
    pub ground_trace: Tr,
    pub impact_speed: f32,
    pub previous_origin: Vec3,
    pub previous_velocity: Vec3,
}

/// `pmove_t`: one cmd's move over `world`. `oldcmd` is the cmd before
/// (`Pmove` copies `cmd` into it after each chopped step); `tracemask` is
/// the caller's (`MASK_PLAYERSOLID`, `MASK_DEADSOLID` or `MASK_SPECTATOR`)
/// and loses the bodies once a trace starts in one; `move_scale` is the
/// held weapon's `moveSpeedScale` (`WeaponDef` +0x264), 0 for none.
pub struct Pmove<'a> {
    pub ps: &'a mut PlayerState,
    pub cmd: UserCmd2,
    pub oldcmd: UserCmd2,
    pub tracemask: u32,
    pub world: MoveWorld<'a>,
    pub rules: Rules,
    pub move_scale: f32,
    /// `touchents`: every entity the cmd met, first meeting first.
    pub touch: Vec<u32>,
    pub prone_change: bool,
    /// The anim script and the held weapon's anim conditions; `None` picks no
    /// anim ([`anim`]).
    pub anim: Option<AnimInput<'a>>,
    /// The horizontal speed `PM_Footsteps` measured (`pmove_t` +0xdc).
    pub(crate) xyspeed: f32,
    pub(crate) mins: Vec3,
    pub(crate) maxs: Vec3,
}

const MAX_TOUCH: usize = 32;

impl<'a> Pmove<'a> {
    pub fn new(
        ps: &'a mut PlayerState,
        cmd: UserCmd2,
        oldcmd: UserCmd2,
        world: MoveWorld<'a>,
        rules: Rules,
    ) -> Pmove<'a> {
        let tracemask = match ps.pm_type {
            pm_type::SPECTATOR => MASK_SPECTATOR,
            t if t > pm_type::INTERMISSION => MASK_DEADSOLID,
            _ => MASK_PLAYERSOLID,
        };
        let (mins, maxs) = (ps.mins, ps.maxs);
        Pmove {
            ps,
            cmd,
            oldcmd,
            tracemask,
            world,
            rules,
            move_scale: 0.0,
            touch: Vec::new(),
            prone_change: false,
            anim: None,
            xyspeed: 0.0,
            mins,
            maxs,
        }
    }

    /// `PM_AddTouchEnt` (0x80e1474): once each, never the world.
    pub(crate) fn add_touch(&mut self, entity: u32) {
        if entity != ENTITYNUM_WORLD
            && entity != ENTITYNUM_NONE as u32
            && self.touch.len() < MAX_TOUCH
            && !self.touch.contains(&entity)
        {
            self.touch.push(entity);
        }
    }

    /// The capsule trace `PM_playerTrace`'s table row calls, under `mask`.
    pub(crate) fn raw_trace(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
    ) -> Tr {
        let t = self.world.box_trace(start, end, mins, maxs, mask);
        Tr {
            fraction: t.fraction,
            normal: t.normal,
            surface_flags: t.surface_flags,
            entity_num: if t.fraction < 1.0 || t.startsolid {
                self.world.entity_num(&t)
            } else {
                ENTITYNUM_NONE as u32
            },
            startsolid: t.startsolid,
            allsolid: t.allsolid,
            body: matches!(t.hit, Some(Prim::Body(_))),
        }
    }

    /// `PM_playerTrace` (0x80e1360) with the cmd's box: a trace that starts
    /// in a body touches it, drops `CONTENTS_BODY` from `pm->tracemask` and
    /// goes again.
    pub(crate) fn trace(&mut self, start: Vec3, end: Vec3) -> Tr {
        self.trace_box(start, end, self.mins, self.maxs, self.tracemask)
    }

    pub(crate) fn trace_box(
        &mut self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
    ) -> Tr {
        let t = self.raw_trace(start, end, mins, maxs, mask);
        if !t.startsolid || !t.body {
            return t;
        }
        self.add_touch(t.entity_num);
        self.tracemask &= !CONTENTS_BODY;
        self.raw_trace(start, end, mins, maxs, mask & !CONTENTS_BODY)
    }
}

/// `Pmove` (0x80e9464): a cmd behind the clock does nothing, one more than
/// 1000 ms ahead pulls `commandTime` up to 1000 short, and the rest is run in
/// steps of at most 66 ms, `oldcmd` taking the cmd after each.
pub fn pmove(pm: &mut Pmove) {
    let final_time = pm.cmd.server_time;
    if final_time < pm.ps.command_time {
        return;
    }
    if pm.ps.command_time + 1000 < final_time {
        pm.ps.command_time = final_time - 1000;
    }
    pm.touch.clear();
    while pm.ps.command_time != final_time {
        let msec = (final_time - pm.ps.command_time).min(66);
        pm.cmd.server_time = pm.ps.command_time + msec;
        pmove_single(pm);
        pm.oldcmd = pm.cmd;
    }
}

/// One cmd for a living or dead player, a spectator or a noclip flyer:
/// [`pmove`] on a fresh `pmove_t`. `oldcmd` is the previous cmd as it came
/// from the client and comes back as `cmd`, untouched by the step:
/// `ClientThink` (0x80f65f8) copies `pers.cmd` to `pers.oldcmd` before it
/// reads the next, and `ClientThink_real` (0x80f5d2e) seeds `pm.oldcmd` from
/// that copy, so a jump the step cleared off a held key stays held.
/// Returns the touch list.
pub fn player_step(
    ps: &mut PlayerState,
    cmd: &UserCmd2,
    oldcmd: &mut UserCmd2,
    world: MoveWorld,
    rules: Rules,
    move_scale: f32,
) -> Vec<u32> {
    player_step_anim(ps, cmd, oldcmd, world, rules, move_scale, None)
}

/// [`player_step`] that also picks the legs and torso anims off `anim`.
pub fn player_step_anim(
    ps: &mut PlayerState,
    cmd: &UserCmd2,
    oldcmd: &mut UserCmd2,
    world: MoveWorld,
    rules: Rules,
    move_scale: f32,
    anim: Option<AnimInput>,
) -> Vec<u32> {
    let mut pm = Pmove::new(ps, *cmd, *oldcmd, world, rules);
    pm.move_scale = move_scale;
    pm.anim = anim;
    pmove(&mut pm);
    *oldcmd = *cmd;
    pm.touch
}

/// rodata 0x815516c: msec to seconds.
const MSEC_TO_SEC: f32 = f32::from_bits(0x3a83_126f);
/// rodata 0x8155170: the tail's displacement-to-velocity ratio, squared.
const TAIL_RATIO: f32 = 0.25;

fn grew(now: i8, before: i8) -> bool {
    now != before && f32::from(before).abs() < f32::from(now).abs()
}

/// `PM_InteruptWeaponWithProneMove` (0x80ed2de): whether a prone move leaves
/// the sight. Firing, the melee's relax and the offhand states keep it; the
/// melee's start and the binoculars are cut off first by the weapon
/// interrupt (0x80ef946), whose playerstate half runs here and whose
/// `WeaponState` half `PM_Weapon` owes through `weapon_interrupt`.
fn prone_move_interrupts(ps: &mut PlayerState) -> bool {
    use weapon::state;
    match ps.weapon_state {
        state::READY..=state::DROPPING | state::RECHAMBERING..=state::RELOAD_END => true,
        state::FIRING | state::MELEE_RELAX..=state::OFFHAND => false,
        s => {
            ps.pm_flags &= !(weapon::wpf::OFFHAND | pmf::PRONE_SIGHT);
            if (state::BINOCULARS_ADS_EXIT..=state::BINOCULARS_RAISE_WEAPON).contains(&s) {
                ps.add_event(weapon::ev::RESET_ADS, 0);
                ps.pm_flags &= !(pmf::SIGHT | weapon::wpf::BINOCULARS);
                ps.add_event(weapon::ev::BINOCULAR_EXIT, 0);
            }
            ps.weapon_state = state::READY;
            ps.weapon_interrupt = true;
            true
        }
    }
}

/// `PmoveSingle` (0x80e8934).
pub(crate) fn pmove_single(pm: &mut Pmove) {
    if pm.ps.pm_flags & pmf::FROZEN != 0 {
        pm.cmd.buttons &= button::PRONE | button::CROUCH | button::STANCE_HELD;
        pm.cmd.forward_move = 0;
        pm.cmd.right_move = 0;
        pm.ps.velocity = Vec3::ZERO;
    }
    if pm.cmd.buttons & button::TALK != 0 {
        pm.cmd.buttons &= button::TALK | button::PRONE | button::CROUCH | button::STANCE_HELD;
        pm.cmd.forward_move = 0;
        pm.cmd.right_move = 0;
    }
    pm.ps.pm_flags &= !pmf::REFUSED;
    if pm.ps.pm_type > pm_type::INTERMISSION {
        pm.tracemask &= !CONTENTS_BODY;
    }
    // Prone: a move key pressed harder than last cmd leaves the sight unless
    // the weapon is busy (cod2-movement.md 17.3).
    if pm.ps.pm_flags & pmf::PRONE == 0 {
        pm.ps.pm_flags &= !pmf::PRONE_SIGHT;
    } else if grew(pm.cmd.forward_move, pm.oldcmd.forward_move)
        || grew(pm.cmd.right_move, pm.oldcmd.right_move)
    {
        if prone_move_interrupts(pm.ps) {
            pm.ps.pm_flags &= !pmf::PRONE_SIGHT;
            pm.ps.add_event(weapon::ev::RESET_ADS, 0);
            pm.ps.pm_flags &= !pmf::SIGHT;
        }
    } else if pm.ps.pm_flags & pmf::SIGHT == 0 && matches!(pm.ps.weapon_state, 0 | 1 | 2 | 5) {
        pm.ps.pm_flags &= !pmf::PRONE_SIGHT;
    }
    let stance = pm.ps.effective_stance();
    if pm.ps.pm_flags & pmf::SIGHT != 0 && stance == Stance::Prone {
        pm.cmd.forward_move = 0;
        pm.cmd.right_move = 0;
    }
    if pm.cmd.buttons & button::TALK != 0 {
        pm.ps.e_flags |= ef::TALK;
    } else {
        pm.ps.e_flags &= !ef::TALK;
    }
    if pm.ps.pm_type < pm_type::DEAD && pm.cmd.buttons & 0x4001 == 0 {
        pm.ps.pm_flags &= !pmf::RESPAWNED;
    }

    let msec = (pm.cmd.server_time - pm.ps.command_time).clamp(1, 200);
    let mut pml = Pml {
        msec,
        previous_origin: pm.ps.origin,
        previous_velocity: pm.ps.velocity,
        frametime: (fpu::d(MSEC_TO_SEC) * f64::from(msec)) as f32,
        ..Default::default()
    };
    pm.ps.command_time = pm.cmd.server_time;

    stance::update_view_angles(pm, pml.msec as f32);
    let [f, r, u] = vcod_common::x87::angle_vectors(Vec3::from_array(pm.ps.view_angles));
    (pml.forward, pml.right, pml.up) = (f, r, u);

    if pm.cmd.forward_move < 0 {
        pm.ps.pm_flags |= pmf::BACKWARDS_RUN;
    } else if pm.cmd.forward_move > 0 || pm.cmd.right_move != 0 {
        pm.ps.pm_flags &= !pmf::BACKWARDS_RUN;
    }
    if pm.ps.pm_type > pm_type::INTERMISSION
        || (stance == Stance::Prone && pm.ps.pm_flags & pmf::PRONE_SIGHT != 0)
    {
        pm.cmd.forward_move = 0;
        pm.cmd.right_move = 0;
    }

    pm.ps.mantle.flags &= !mantle::flag::HINT;
    match pm.ps.pm_type {
        pm_type::NORMAL_LINKED | pm_type::DEAD_LINKED => linked(pm, &mut pml),
        pm_type::NOCLIP | pm_type::UFO => {
            leave_ladder(pm.ps);
            sight_walk(pm);
            moves::noclip_move(pm, &mut pml);
            drop_timers(pm.ps, &pml);
        }
        pm_type::SPECTATOR => {
            leave_ladder(pm.ps);
            sight_walk(pm);
            stance::check_duck(pm, &mut pml);
            moves::fly_move(pm, &mut pml);
            drop_timers(pm.ps, &pml);
        }
        pm_type::INTERMISSION => leave_ladder(pm.ps),
        _ if pm.ps.e_flags & ef::TURRET != 0 => linked(pm, &mut pml),
        _ => {
            if pm.ps.pm_flags & pmf::MANTLE == 0 {
                stance::check_duck(pm, &mut pml);
                ground::ground_trace(pm, &mut pml);
            }
            mantle::check(pm, &pml);
            if pm.ps.pm_flags & pmf::MANTLE != 0 {
                // A mantle, this frame's start included: no ground, no
                // timers, no footsteps and no tail.
                leave_ladder(pm.ps);
                pm.ps.ground_entity_num = ENTITYNUM_NONE;
                pml.ground_plane = false;
                pml.walking = false;
                sight_walk(pm);
                stance::check_duck(pm, &mut pml);
                mantle::mantle_move(pm, &pml);
                return;
            }
            sight_walk(pm);
            stance::update_prone_pitch(pm, &pml);
            if pm.ps.pm_type == pm_type::DEAD {
                moves::dead_move(pm.ps, &pml);
            }
            ladder::check_ladder_move(pm, &mut pml);
            drop_timers(pm.ps, &pml);
            if pm.ps.pm_flags & pmf::LADDER != 0 {
                ladder::ladder_move(pm, &mut pml);
            } else if pml.walking {
                moves::walk_move(pm, &mut pml);
            } else {
                moves::air_move(pm, &mut pml);
            }
            ground::ground_trace(pm, &mut pml);
            footsteps::footsteps(pm, &pml);
            foliage_sound(pm);
            tail(pm.ps, &pml);
        }
    }
}

/// The `bg_foliagesnd_*` defaults (`BG_RegisterDvars`, Mac 0x694d4): the
/// speeds the interval scales between, the intervals at either end and the
/// stop that clears the timer. vcod keeps the defaults, as for [`Rules`].
const FOLIAGE_MIN_SPEED: f32 = 40.0;
const FOLIAGE_MAX_SPEED: f32 = 180.0;
const FOLIAGE_SLOW_MS: i32 = 1500;
const FOLIAGE_FAST_MS: i32 = 500;
const FOLIAGE_RESET_MS: i32 = 500;
/// rodata 0x8155108: the foliage box's height scale; its other sides take
/// 0.75 (immediate 0x3f400000).
const FOLIAGE_BOX_Z: f32 = f32::from_bits(0x3f66_6666);

/// `PM_FoliageSounds` (lnxded 0x80e6784, inlined in the Mac's
/// `PmoveSingle`), after `PM_Footsteps`: at `xyspeed` 40 or more, once the
/// interval (1500 ms at 40 down to 500 at 180, truncated) has passed since
/// the last, a box of the body's shrunk to 0.75 (0.9 tall) that starts in
/// foliage (contents 2) raises `EV_FOLIAGE_SOUND`. Slower, the timer goes to
/// 0 once 500 ms have passed.
fn foliage_sound(pm: &mut Pmove) {
    use fpu::d;
    let now = pm.cmd.server_time;
    if pm.xyspeed < FOLIAGE_MIN_SPEED {
        if pm.ps.foliage_sound_time + FOLIAGE_RESET_MS < now {
            pm.ps.foliage_sound_time = 0;
        }
        return;
    }
    let f = ((d(pm.xyspeed) - d(FOLIAGE_MIN_SPEED)) / (d(FOLIAGE_MAX_SPEED) - d(FOLIAGE_MIN_SPEED)))
        as f32;
    let f = f.min(1.0);
    let interval =
        (f64::from(FOLIAGE_SLOW_MS) + f64::from(FOLIAGE_FAST_MS - FOLIAGE_SLOW_MS) * d(f)) as i32;
    if pm.ps.foliage_sound_time + interval >= now {
        return;
    }
    let mins = fpu::scale(pm.mins, 0.75);
    let mut maxs = fpu::scale(pm.maxs, 0.75);
    maxs.z = (d(FOLIAGE_BOX_Z) * d(pm.maxs.z)) as f32;
    let o = pm.ps.origin;
    if pm.trace_box(o, o, mins, maxs, CONTENTS_FOLIAGE).startsolid {
        pm.ps.add_event(ev::FOLIAGE_SOUND, 0);
        pm.ps.foliage_sound_time = now;
    }
}

/// A linked player (`pm_type` 1, 7) or a gunner: off the ground, still, the
/// duck and the timers only.
fn linked(pm: &mut Pmove, pml: &mut Pml) {
    leave_ladder(pm.ps);
    pm.ps.ground_entity_num = ENTITYNUM_NONE;
    pml.ground_plane = false;
    pml.almost_ground_plane = false;
    pml.walking = false;
    pm.ps.velocity = Vec3::ZERO;
    sight_walk(pm);
    stance::check_duck(pm, pml);
    drop_timers(pm.ps, pml);
    footsteps::footsteps(pm, pml);
}

/// 0x80e8026: a player taken off the ladder notes it.
pub(crate) fn leave_ladder(ps: &mut PlayerState) {
    if ps.pm_flags & pmf::LADDER != 0 {
        ps.pm_flags |= pmf::LADDER_LEFT;
    }
    ps.pm_flags &= !pmf::LADDER;
}

/// 0x80e7f68: the sighted walk follows the sight key on a sighted, unprone
/// player whose weapon is not in states 5..9.
fn sight_walk(pm: &mut Pmove) {
    let ps = &mut *pm.ps;
    ps.pm_flags &= !pmf::SIGHT_WALK;
    if ps.pm_type < pm_type::DEAD
        && pm.cmd.buttons & button::ADS != 0
        && ps.pm_flags & pmf::PRONE == 0
        && ps.pm_flags & pmf::SIGHT != 0
        && !(5..=9).contains(&ps.weapon_state)
    {
        ps.pm_flags |= pmf::SIGHT_WALK;
    }
}

/// `PM_DropTimers` (0x80e6936): `pm_time` runs out, and with it the land,
/// knockback and jump timers' flags; the legs and torso timers run down to 0.
fn drop_timers(ps: &mut PlayerState, pml: &Pml) {
    if ps.pm_time != 0 {
        if pml.msec < ps.pm_time {
            ps.pm_time -= pml.msec;
        } else {
            ps.pm_flags &= !(pmf::TIME_LAND | pmf::TIME_KNOCKBACK | pmf::JUMPING);
            ps.pm_time = 0;
        }
    }
    for t in [&mut ps.legs_timer, &mut ps.torso_timer] {
        if *t > 0 {
            *t = (*t - pml.msec).max(0);
        }
    }
}

/// The default arm's tail: a velocity well over what the frame moved is
/// cut to the displacement, `oldVelocity` eases toward the velocity by the
/// frame (at most all the way), and the velocity is snapped with libc
/// `rint`, which keeps the sign of a zero (0x80d5e94).
fn tail(ps: &mut PlayerState, pml: &Pml) {
    let ft = fpu::d(pml.frametime);
    let delta = ps.origin - pml.previous_origin;
    let moved = (fpu::length_sq(delta) / (ft * ft)) as f32;
    if f64::from(moved) < fpu::d(TAIL_RATIO) * f64::from(fpu::length_sq(ps.velocity) as f32) {
        ps.velocity = fpu::scale(delta, (1.0 / ft) as f32);
    }
    let k = pml.frametime.min(1.0);
    for i in 0..2 {
        let step = (fpu::d(ps.velocity[i]) - fpu::d(ps.old_velocity[i])) as f32;
        let step = (fpu::d(k) * fpu::d(step)) as f32;
        ps.old_velocity[i] = (fpu::d(ps.old_velocity[i]) + fpu::d(step)) as f32;
    }
    ps.velocity = Vec3::new(
        f64::from(ps.velocity.x).round_ties_even() as f32,
        f64::from(ps.velocity.y).round_ties_even() as f32,
        f64::from(ps.velocity.z).round_ties_even() as f32,
    );
}

#[cfg(test)]
mod tests;
