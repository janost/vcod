//! CoD4 1.7's player movement: `Pmove` and `PmoveSingle` (docs/research/
//! cod4-pmove.md), seeded from CoD2's (`vcod_cod2::pmove`), whose descendant
//! it is, with its own 1.7 playerstate. The two share the collision kernels
//! (`vcod_common::clipmap`) and nothing else yet: the functions the moves
//! A/B shows equal fold later.
//!
//! What differs from CoD2 and why each piece reads as it does is in the
//! doc; `cod4_moves_ab` replays retail captures through [`player_step`].
//! `cod4_lnxded` keeps float locals on the x87 stack and rounds only what it
//! stores (cod4-pmove.md 1): the functions the A/B pinned compute that way
//! (`f64` locals, `as f32` at a store), the rest still in CoD2's
//! stored model ([`fpu`]).
//!
//! The weapon half ([`weapon`]: the aim spread, the sight, `PM_Weapon`)
//! runs when the caller hands the level's weapon defs in ([`Pmove::defs`]);
//! without them only the breath runs. The mantle ([`mantle`]) runs when
//! [`Rules::mantle_anims`] holds the clips. Not here: the anim script calls
//! and the melee charge.

pub mod aim;
pub mod anim;
mod breath;
pub mod charge;
mod footsteps;
mod fpu;
mod ground;
mod ladder;
pub mod mantle;
mod moves;
mod slide;
mod stance;
pub mod weapon;
pub mod wire;

use glam::Vec3;
use vcod_common::clipmap::ClipTrace;

use crate::collision::ClipMap;

pub use slide::clip_velocity;
pub use stance::lerp_duration as view_height_lerp_duration;

/// `groundEntityNum` off the ground, and the world's number.
pub const ENTITYNUM_NONE: i32 = 1023;
pub const ENTITYNUM_WORLD: i32 = 1022;
/// Entities below this are clients (`MAX_CLIENTS`).
pub const MAX_CLIENTS: i32 = 64;

/// `pm_type`: 1.7 puts `PM_LASTSTAND` at 6 and the dead at 7, one past
/// CoD2's (cod4-pmove.md 2).
pub mod pm_type {
    pub const NORMAL: i32 = 0;
    pub const NORMAL_LINKED: i32 = 1;
    pub const NOCLIP: i32 = 2;
    pub const UFO: i32 = 3;
    pub const SPECTATOR: i32 = 4;
    pub const INTERMISSION: i32 = 5;
    pub const LASTSTAND: i32 = 6;
    pub const DEAD: i32 = 7;
    pub const DEAD_LINKED: i32 = 8;
}

/// `pm_flags` (cod4-pmove.md 2), renumbered from CoD2's. The 1.7 binary
/// is stripped: the names are vcod's (CoD2's where the flag is CoD2's), and
/// each cites a `cod4_lnxded` instruction that sets or tests it.
pub mod pmf {
    /// Set off the stance (`PM_CheckDuck`), tested at 0x805bd9d.
    pub const PRONE: u32 = 0x1;
    /// Set at 0x805914e.
    pub const DUCKED: u32 = 0x2;
    /// Set at 0x8051c88 when a mantle starts.
    pub const MANTLE: u32 = 0x4;
    /// Refuses a sprint at 0x8057176.
    pub const LADDER: u32 = 0x8;
    /// Sighted; cleared on leaving the sight at 0x805fd18.
    pub const SIGHT: u32 = 0x10;
    /// Set at 0x805c417, cleared at 0x805bf2d.
    pub const BACKWARDS_RUN: u32 = 0x20;
    /// The sighted walk and its 0.4 move scale, set at 0x805c08a.
    pub const SIGHT_WALK: u32 = 0x40;
    /// The landing stun's timer, set at 0x805b952: friction x0.3, accel
    /// x0.25.
    pub const TIME_LAND: u32 = 0x80;
    /// Run out with the landing and the jump at 0x805c460.
    pub const TIME_KNOCKBACK: u32 = 0x100;
    /// A prone body's move keys held at 0 until one is pushed; cleared off
    /// the prone at 0x805bda7.
    pub const PRONE_STILL: u32 = 0x200;
    /// Tested at 0x805bdf1, cleared at 0x805bfaf.
    pub const RESPAWNED: u32 = 0x400;
    /// Tested at 0x805bd39.
    pub const FROZEN: u32 = 0x800;
    /// A prone key refused this cmd (`stance::may_lie_down` said no): set at
    /// 0x80567e8, cleared each frame at 0x805bd89.
    pub const PRONE_REFUSED: u32 = 0x1000;
    /// The ladder just left, set at 0x805c473.
    pub const LADDER_LEFT: u32 = 0x2000;
    /// The jump and its landing slowdown's timer, set at 0x80507b5.
    pub const JUMPING: u32 = 0x4000;
    /// Set at 0x805723f.
    pub const SPRINTING: u32 = 0x8000;
    /// Refuses a sprint at 0x80571b0.
    pub const SHELLSHOCKED: u32 = 0x10000;
    /// The lunge: set at 0x805621b, forces the forward key at 0x805bd2c.
    pub const MELEE_CHARGE: u32 = 0x20000;
    /// Refuses a sprint at 0x8057164.
    pub const NO_SPRINT: u32 = 0x40000;
    /// Refuses a jump at 0x8050a53.
    pub const NO_JUMP: u32 = 0x80000;
}

/// `usercmd_s.buttons` as 1.7's wire carries them (`vcod_cod4::net::clmsg`).
pub mod button {
    pub const ATTACK: u32 = 0x1;
    pub const SPRINT: u32 = 0x2;
    pub const MELEE: u32 = 0x4;
    pub const RELOAD: u32 = 0x10;
    pub const USE_RELOAD: u32 = 0x20;
    pub const LEAN_LEFT: u32 = 0x40;
    pub const LEAN_RIGHT: u32 = 0x80;
    pub const PRONE: u32 = 0x100;
    pub const CROUCH: u32 = 0x200;
    pub const JUMP: u32 = 0x400;
    /// The held sight key (`KEY_MASK_ADS_MODE`): what the sighted walk reads.
    pub const ADS_MODE: u32 = 0x800;
    /// Silences `PM_CheckDuck`'s refused-stance events.
    pub const STANCE_HELD: u32 = 0x1000;
    pub const FRAG: u32 = 0x4000;
    pub const SMOKE: u32 = 0x8000;
    /// The console, a menu or the chat line.
    pub const TALK: u32 = 0x10_0000;
}

/// `eFlags` bits pmove writes.
pub mod ef {
    pub const CROUCH: u32 = 0x4;
    pub const PRONE: u32 = 0x8;
    /// `EF_FIRING`.
    pub const FIRING: u32 = 0x40;
    /// A turret's stance bits; any set makes the player a gunner.
    pub const TURRET: u32 = 0x300;
    pub const TALK: u32 = 0x20_0000;
}

/// The events pmove raises, 1.7's numbers (`net::events`). A footstep, the
/// jump and a soft landing carry the surface in the parm; a hard landing
/// and a painful one add it to the event.
pub mod ev {
    pub const FOLIAGE_SOUND: u32 = 1;
    pub const STANCE_FORCE_STAND: u32 = 6;
    pub const STANCE_FORCE_CROUCH: u32 = 7;
    pub const STANCE_FORCE_PRONE: u32 = 8;
    pub const FOOTSTEP_SPRINT: u32 = 72;
    pub const FOOTSTEP_RUN: u32 = 73;
    pub const FOOTSTEP_WALK: u32 = 74;
    pub const FOOTSTEP_PRONE: u32 = 75;
    pub const JUMP: u32 = 76;
    pub const LANDING_FIRST: u32 = 77;
    pub const LANDING_PAIN_FIRST: u32 = 106;
}

/// `ClientThink_real`'s `pm.tracemask`: alive, dead and spectating.
pub const MASK_PLAYERSOLID: u32 = 0x2810011;
pub const MASK_DEADSOLID: u32 = 0x810011;
pub const MASK_SPECTATOR: u32 = 0x800811;
/// `CONTENTS_BODY`.
pub const CONTENTS_BODY: u32 = 0x200_0000;
/// `otherFlags` 4: a player in the world (`POF_PLAYER`).
pub const POF_PLAYER: u32 = 4;

/// The player box (`playerMins`, `playerMaxs`).
pub const PLAYER_MINS: Vec3 = Vec3::new(-15.0, -15.0, 0.0);
pub const PLAYER_MAXS: Vec3 = Vec3::new(15.0, 15.0, 70.0);

/// The eye's stance targets; 22 is the last stand's.
pub const VIEW_STAND: i32 = 60;
pub const VIEW_CROUCH: i32 = 40;
pub const VIEW_LASTSTAND: i32 = 22;
pub const VIEW_PRONE: i32 = 11;
pub const VIEW_DEAD: i32 = 8;

/// The movement dvars pmove reads. [`Rules::default`] holds 1.7's
/// registered defaults, which the walk capture's dvar mirror (CS 36..124
/// names, the values 128 slots on) carries unchanged
/// (docs/research/cod4-pmove.md, "Dvars").
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
    pub ladder_yawcap: f32,
    pub back_speed_scale: f32,
    pub strafe_speed_scale: f32,
    pub spectate_speed_scale: f32,
    pub view_pitch_up: f32,
    pub view_pitch_down: f32,
    pub prone_yawcap: f32,
    pub fall_damage_min_height: f32,
    pub fall_damage_max_height: f32,
    pub move_threshold: f32,
    pub footsteps_threshold: f32,
    pub sprint_time: f32,
    pub sprint_min_time: f32,
    pub sprint_recharge_pause: f32,
    pub sprint_speed_scale: f32,
    pub sprint_strafe_speed_scale: f32,
    pub sprint_forward_minimum: i32,
    pub sprint_camera_bob: f32,
    pub dmgtimer_max_time: f32,
    pub dmgtimer_min_scale: f32,
    /// `perk_sprintMultiplier`, under `specialty_longersprint` (perk 0x400).
    pub perk_sprint_multiplier: f32,
    /// `player_breath_hold_time`, `_gasp_time` (seconds), `_gasp_scale`
    /// and `_gasp_lerp`.
    pub breath_hold_time: f32,
    pub breath_gasp_time: f32,
    pub breath_gasp_scale: f32,
    pub breath_gasp_lerp: f32,
    /// `player_breath_hold_lerp`.
    pub breath_hold_lerp: f32,
    /// `mantle_enable`, `mantle_check_range`, `mantle_check_radius`,
    /// `mantle_check_angle` and `mantle_view_yawcap`.
    pub mantle_enable: bool,
    pub mantle_check_range: f32,
    pub mantle_check_radius: f32,
    pub mantle_check_angle: f32,
    pub mantle_view_yawcap: f32,
    /// The mantle clips; without them no mantle starts. The default takes
    /// what [`mantle::install`] kept.
    pub mantle_anims: Option<&'static mantle::MantleAnims>,
    /// The shellshock slots (`ps.shellshockIndex`, bit `index`) whose
    /// record sets `bg_shock_movement` (`crate::shock::movement_bits`):
    /// under `PMF_SHELLSHOCKED` they walk at 0.4 of the speed.
    pub shock_slows: u16,
    /// `player_meleeChargeFriction` (1200): the lunge's friction, which
    /// also sets its speed and length (cod4-pmove.md 17).
    pub melee_charge_friction: f32,
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
            spectate_speed_scale: 1.0,
            view_pitch_up: 85.0,
            view_pitch_down: 85.0,
            prone_yawcap: 85.0,
            fall_damage_min_height: 128.0,
            fall_damage_max_height: 300.0,
            move_threshold: 10.0,
            footsteps_threshold: 0.0,
            sprint_time: 4.0,
            sprint_min_time: 1.0,
            sprint_recharge_pause: 0.0,
            sprint_speed_scale: 1.5,
            sprint_strafe_speed_scale: f32::from_bits(0x3f2a_c083),
            sprint_forward_minimum: 105,
            sprint_camera_bob: 0.5,
            dmgtimer_max_time: 750.0,
            dmgtimer_min_scale: 0.0,
            perk_sprint_multiplier: 2.0,
            breath_hold_time: 4.5,
            breath_gasp_time: 1.0,
            breath_gasp_scale: 4.5,
            breath_gasp_lerp: 6.0,
            breath_hold_lerp: 1.0,
            mantle_enable: true,
            mantle_check_range: 20.0,
            mantle_check_radius: f32::from_bits(0x3dcc_cccd),
            mantle_check_angle: 60.0,
            mantle_view_yawcap: 60.0,
            mantle_anims: mantle::installed(),
            shock_slows: 0,
            melee_charge_friction: 1200.0,
        }
    }
}

/// The held weapon's `WeaponDef` fields pmove reads; [`WeaponMove::NONE`]
/// for weapon 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaponMove {
    pub move_speed_scale: f32,
    pub ads_move_speed_scale: f32,
    pub sprint_duration_scale: f32,
}

impl Default for WeaponMove {
    fn default() -> Self {
        WeaponMove::NONE
    }
}

impl WeaponMove {
    pub const NONE: WeaponMove = WeaponMove {
        move_speed_scale: 0.0,
        ads_move_speed_scale: 0.0,
        sprint_duration_scale: 0.0,
    };

    /// The fields off a weapon asset's raw `WeaponDef` (offsets in
    /// cod4-pmove.md, "The weapon's scales").
    pub fn from_raw(raw: &[u8]) -> Option<WeaponMove> {
        let f = |o: usize| Some(f32::from_le_bytes(raw.get(o..o + 4)?.try_into().ok()?));
        Some(WeaponMove {
            move_speed_scale: f(WEAPONDEF_MOVE_SPEED_SCALE)?,
            ads_move_speed_scale: f(WEAPONDEF_MOVE_SPEED_SCALE + 4)?,
            sprint_duration_scale: f(WEAPONDEF_MOVE_SPEED_SCALE + 8)?,
        })
    }
}

/// `WeaponDef.moveSpeedScale`: six floats ahead of `adsOverlayShader` (1072).
pub const WEAPONDEF_MOVE_SPEED_SCALE: usize = 1048;

/// `SprintState`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SprintState {
    pub button_up_required: bool,
    pub delay: bool,
    pub last_start: i32,
    pub last_end: i32,
    pub start_max_length: i32,
}

/// The part of 1.7's `playerState_s` pmove reads and writes, under the
/// retail names. Angles are degrees, wire convention (pitch positive
/// down); 1.7 keeps `delta_angles` as floats, CoD2 as shorts.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerState {
    pub command_time: i32,
    pub pm_type: i32,
    pub bob_cycle: i32,
    pub pm_flags: u32,
    pub pm_time: i32,
    pub origin: Vec3,
    pub velocity: Vec3,
    /// The inertia clamp's reference; not on the wire.
    pub old_velocity: [f32; 2],
    pub gravity: i32,
    pub leanf: f32,
    pub speed: i32,
    pub delta_angles: [f32; 3],
    pub ground_entity_num: i32,
    pub ladder_vec: Vec3,
    pub jump_time: i32,
    pub jump_origin_z: f32,
    pub movement_dir: i32,
    pub e_flags: u32,
    pub event_sequence: i32,
    pub events: [u32; 4],
    pub event_parms: [u32; 4],
    pub client_num: u32,
    pub weapon: u32,
    pub weapon_state: i32,
    pub weapon_pos_frac: f32,
    pub view_angles: [f32; 3],
    pub view_height_target: i32,
    pub view_height_current: f32,
    pub view_height_lerp_time: i32,
    pub view_height_lerp_target: i32,
    pub view_height_lerp_down: bool,
    pub damage_timer: i32,
    pub damage_duration: i32,
    pub view_angle_clamp_base: [f32; 2],
    pub view_angle_clamp_range: [f32; 2],
    pub prone_direction: f32,
    pub prone_direction_pitch: f32,
    pub prone_torso_pitch: f32,
    /// `fTorsoPitch`, `fWaistPitch`: `BG_CheckProneValid`'s outputs.
    pub torso_pitch: f32,
    pub waist_pitch: f32,
    pub aim_spread_scale: f32,
    pub foliage_sound_time: i32,
    pub legs_timer: i32,
    pub torso_timer: i32,
    /// `legsAnim`, `torsoAnim` (the animtree index, 0x200 the toggle) and
    /// `flinchYawAnim`; the script's `MOVETYPE` and `STRAFING` conditions,
    /// which the wire does not carry ([`anim`]).
    pub legs_anim: u32,
    pub torso_anim: u32,
    pub flinch_yaw_anim: u32,
    pub anim_movetype: u8,
    pub anim_strafing: u8,
    pub sprint: SprintState,
    pub move_speed_scale_multiplier: f32,
    /// `shellshockIndex`, which [`Rules::shock_slows`] reads.
    pub shellshock_index: i32,
    pub perks: u32,
    /// `weapFlags`, `holdBreathScale`, `holdBreathTimer`: what the breath
    /// update reads and writes.
    pub weap_flags: u32,
    pub hold_breath_scale: f32,
    pub hold_breath_timer: i32,
    /// The weapon half's fields ([`weapon`]).
    pub weapon_time: i32,
    pub weapon_delay: i32,
    pub weapon_shot_count: i32,
    pub weap_anim: u32,
    pub weapon_restrict_kick_time: i32,
    pub spread_override: i32,
    pub spread_override_state: i32,
    pub ads_delay_time: i32,
    pub grenade_time_left: i32,
    pub other_flags: u32,
    pub off_hand_index: u32,
    /// `offhandSecondary` (0 smoke, 1 flash), `throwBackGrenadeOwner`,
    /// `throwBackGrenadeTimeLeft` and `cursorHintEntIndex`: what the
    /// offhand reads.
    pub offhand_secondary: u32,
    pub throw_back_grenade_owner: i32,
    pub throw_back_grenade_time_left: i32,
    pub cursor_hint_ent_index: i32,
    pub weapons: [u32; 4],
    pub weaponold: [u32; 4],
    pub weaponrechamber: [u32; 4],
    pub ammo: [i32; 64],
    pub ammoclip: [i32; 128],
    pub mantle: mantle::MantleState,
    /// The lunge (`PMF_MELEE_CHARGE`): its yaw in degrees, its distance
    /// and the ms it has left, 0 until its first frame sets it off.
    pub melee_charge_yaw: f32,
    pub melee_charge_dist: i32,
    pub melee_charge_time: i32,
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
            delta_angles: [0.0; 3],
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
            weapon: 0,
            weapon_state: 0,
            weapon_pos_frac: 0.0,
            view_angles: [0.0; 3],
            view_height_target: VIEW_STAND,
            view_height_current: VIEW_STAND as f32,
            view_height_lerp_time: 0,
            view_height_lerp_target: 0,
            view_height_lerp_down: false,
            damage_timer: 0,
            damage_duration: 0,
            view_angle_clamp_base: [0.0; 2],
            view_angle_clamp_range: [0.0; 2],
            prone_direction: 0.0,
            prone_direction_pitch: 0.0,
            prone_torso_pitch: 0.0,
            torso_pitch: 0.0,
            waist_pitch: 0.0,
            aim_spread_scale: 0.0,
            foliage_sound_time: 0,
            legs_timer: 0,
            torso_timer: 0,
            legs_anim: 0,
            torso_anim: 0,
            flinch_yaw_anim: 0,
            anim_movetype: 0,
            anim_strafing: 0,
            sprint: SprintState::default(),
            move_speed_scale_multiplier: 1.0,
            shellshock_index: 0,
            perks: 0,
            weap_flags: 0,
            hold_breath_scale: 1.0,
            hold_breath_timer: 0,
            weapon_time: 0,
            weapon_delay: 0,
            weapon_shot_count: 0,
            weap_anim: 0,
            weapon_restrict_kick_time: 0,
            spread_override: 0,
            spread_override_state: 0,
            ads_delay_time: 0,
            grenade_time_left: 0,
            other_flags: 0,
            off_hand_index: 0,
            offhand_secondary: 0,
            throw_back_grenade_owner: ENTITYNUM_NONE,
            throw_back_grenade_time_left: 0,
            cursor_hint_ent_index: 0,
            weapons: [0; 4],
            weaponold: [0; 4],
            weaponrechamber: [0; 4],
            ammo: [0; 64],
            ammoclip: [0; 128],
            mantle: mantle::MantleState::default(),
            melee_charge_yaw: 0.0,
            melee_charge_dist: 0,
            melee_charge_time: 0,
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

    /// `BG_AddPredictableEventToPlayerstate`: event 0 is dropped, the parm
    /// is a byte, the sequence counts on.
    pub fn add_event(&mut self, event: u32, parm: u32) {
        if event == 0 {
            return;
        }
        let slot = (self.event_sequence & 3) as usize;
        self.events[slot] = event & 0xff;
        self.event_parms[slot] = parm & 0xff;
        self.event_sequence += 1;
    }

    /// `PM_GetEffectiveStance`: off the eye's target, 40 and 22 crouched,
    /// 11 prone.
    pub fn effective_stance(&self) -> Stance {
        match self.view_height_target {
            VIEW_CROUCH | VIEW_LASTSTAND => Stance::Crouch,
            VIEW_PRONE => Stance::Prone,
            _ => Stance::Stand,
        }
    }

    pub fn on_ground(&self) -> bool {
        self.ground_entity_num != ENTITYNUM_NONE
    }

    /// `PM_IsSprinting`.
    pub fn is_sprinting(&self) -> bool {
        self.sprint.last_start != 0 && self.sprint.last_start > self.sprint.last_end
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stance {
    Stand,
    Crouch,
    Prone,
}

/// A trace as 1.7's pmove reads it: no end point, `walkable` kept.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tr {
    pub fraction: f32,
    pub normal: Vec3,
    pub surface_flags: u32,
    pub contents: u32,
    pub entity_num: u32,
    pub startsolid: bool,
    pub allsolid: bool,
    pub walkable: bool,
}

impl Default for Tr {
    fn default() -> Self {
        Tr {
            fraction: 1.0,
            normal: Vec3::ZERO,
            surface_flags: 0,
            contents: 0,
            entity_num: ENTITYNUM_NONE as u32,
            startsolid: false,
            allsolid: false,
            walkable: false,
        }
    }
}

/// What pmove traces through: the handler's capsule trace. [`ClipMap`]
/// alone is the world without entities; the server wraps it with the
/// bodies and brush entities.
pub trait PmWorld {
    /// A capsule (or point) sweep from `start` to `end` under `mask`,
    /// passing entity `pass`.
    fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, pass: u32, mask: u32) -> Tr;
}

/// A world trace's contact named as `SV_Trace` names it.
pub fn world_tr(t: &ClipTrace) -> Tr {
    let hit = t.fraction < 1.0 || t.startsolid;
    Tr {
        fraction: t.fraction,
        normal: t.normal,
        surface_flags: t.surface_flags,
        contents: t.contents,
        entity_num: if hit {
            ENTITYNUM_WORLD as u32
        } else {
            ENTITYNUM_NONE as u32
        },
        startsolid: t.startsolid,
        allsolid: t.allsolid,
        walkable: t.walkable,
    }
}

impl PmWorld for ClipMap {
    fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, _pass: u32, mask: u32) -> Tr {
        world_tr(&ClipMap::trace(self, start, end, mins, maxs, mask))
    }
}

/// `pml_t`: what one `PmoveSingle` keeps beside the playerstate.
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

/// The usercmd fields pmove reads (`vcod_cod4::net::clmsg::UserCmd`'s,
/// angles as the wire's shorts).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cmd {
    pub server_time: i32,
    pub buttons: u32,
    pub angles: [u16; 3],
    pub weapon: u8,
    /// The offhand the client has equipped (`offHandIndex`).
    pub offhand: u8,
    pub forward_move: i8,
    pub right_move: i8,
    /// The lunge the client's target pick asked for with the melee key:
    /// the yaw as the wire's short, the distance in units (0 none).
    pub melee_charge_yaw: u16,
    pub melee_charge_dist: u8,
}

impl From<&crate::net::clmsg::UserCmd> for Cmd {
    fn from(c: &crate::net::clmsg::UserCmd) -> Cmd {
        Cmd {
            server_time: c.server_time,
            buttons: c.buttons,
            angles: c.angles,
            weapon: c.weapon,
            offhand: c.offhand,
            forward_move: c.forwardmove,
            right_move: c.rightmove,
            melee_charge_yaw: c.melee_charge_yaw,
            melee_charge_dist: c.melee_charge_dist,
        }
    }
}

/// `pmove_t`: one cmd's move over `world`. `oldcmd` is the cmd before;
/// `tracemask` loses the bodies once a trace starts in one; `weapon` is the
/// held weapon's scales.
pub struct Pmove<'a> {
    pub ps: &'a mut PlayerState,
    pub cmd: Cmd,
    pub oldcmd: Cmd,
    pub tracemask: u32,
    pub world: &'a dyn PmWorld,
    pub rules: Rules,
    pub weapon: WeaponMove,
    /// The level's weapon defs: the weapon half runs only with them.
    pub defs: Option<&'a dyn weapon::WeaponDefs>,
    /// The anim script: the legs and torso anims are picked only with it.
    pub anim: Option<anim::AnimInput<'a>>,
    /// The process's `rand()`: an anim script event draws its command off
    /// it (cod4-combat.md 10.9); without it the first command plays.
    pub rand: Option<&'a mut dyn FnMut() -> i32>,
    /// `touchents`: every entity the cmd met, first meeting first.
    pub touch: Vec<u32>,
    pub prone_change: bool,
    /// The horizontal speed `PM_Footsteps` measured.
    pub xyspeed: f32,
    /// Each step `PM_StepSlideMove` took, as `(commandTime, dz)`: what it
    /// adds to `pmove_t.viewChange`, which the cgame's camera smooths
    /// (cod4-pmove.md 5).
    pub view_steps: Vec<(i32, f32)>,
    /// `mantleStarted` with `mantleEndPos` and `mantleDuration`: a mantle
    /// this cmd started and has not finished, for the server's blockage.
    pub mantle_started: Option<(Vec3, i32)>,
    pub(crate) mins: Vec3,
    pub(crate) maxs: Vec3,
}

const MAX_TOUCH: usize = 32;

impl<'a> Pmove<'a> {
    pub fn new(
        ps: &'a mut PlayerState,
        cmd: Cmd,
        oldcmd: Cmd,
        world: &'a dyn PmWorld,
        rules: Rules,
    ) -> Pmove<'a> {
        let tracemask = match ps.pm_type {
            pm_type::SPECTATOR => MASK_SPECTATOR,
            t if t >= pm_type::DEAD => MASK_DEADSOLID,
            _ => MASK_PLAYERSOLID,
        };
        Pmove {
            ps,
            cmd,
            oldcmd,
            tracemask,
            world,
            rules,
            weapon: WeaponMove::NONE,
            defs: None,
            anim: None,
            rand: None,
            touch: Vec::new(),
            prone_change: false,
            xyspeed: 0.0,
            view_steps: Vec::new(),
            mantle_started: None,
            mins: PLAYER_MINS,
            maxs: PLAYER_MAXS,
        }
    }

    /// `PM_AddTouchEnt`: once each, never the world.
    pub(crate) fn add_touch(&mut self, entity: u32) {
        if entity != ENTITYNUM_WORLD as u32
            && entity != ENTITYNUM_NONE as u32
            && self.touch.len() < MAX_TOUCH
            && !self.touch.contains(&entity)
        {
            self.touch.push(entity);
        }
    }

    /// The handler's trace (`PM_trace`).
    pub(crate) fn raw_trace(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
    ) -> Tr {
        self.world
            .trace(start, end, mins, maxs, self.ps.client_num, mask)
    }

    /// `PM_playerTrace` with the cmd's box.
    pub(crate) fn trace(&mut self, start: Vec3, end: Vec3) -> Tr {
        self.trace_box(start, end, self.mins, self.maxs, self.tracemask)
    }

    /// `PM_playerTrace`: a trace that starts in a body touches it, drops
    /// `CONTENTS_BODY` from `pm->tracemask` and goes again.
    pub(crate) fn trace_box(
        &mut self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
    ) -> Tr {
        let t = self.raw_trace(start, end, mins, maxs, mask);
        if !t.startsolid || t.contents & CONTENTS_BODY == 0 {
            return t;
        }
        self.add_touch(t.entity_num);
        self.tracemask &= !CONTENTS_BODY;
        self.raw_trace(start, end, mins, maxs, mask & !CONTENTS_BODY)
    }
}

/// `Pmove`: a cmd behind the clock does nothing, one more than 1000 ms
/// ahead pulls `commandTime` up to 1000 short, and the rest runs in steps
/// of at most 66 ms, `oldcmd` taking the cmd after each.
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

/// One cmd for a player, as `ClientThink_real` runs it: [`pmove`] on a
/// fresh `pmove_t` whose `oldcmd` is the previous cmd as the client sent
/// it. Returns the touch list.
pub fn player_step(
    ps: &mut PlayerState,
    cmd: &Cmd,
    oldcmd: &mut Cmd,
    world: &dyn PmWorld,
    rules: Rules,
    weapon: WeaponMove,
) -> Vec<u32> {
    player_step_viewed(ps, cmd, oldcmd, world, rules, weapon).0
}

/// [`player_step`], with the steps the eye took ([`Pmove::view_steps`]).
pub fn player_step_viewed(
    ps: &mut PlayerState,
    cmd: &Cmd,
    oldcmd: &mut Cmd,
    world: &dyn PmWorld,
    rules: Rules,
    weapon: WeaponMove,
) -> (Vec<u32>, Vec<(i32, f32)>) {
    let mut pm = Pmove::new(ps, *cmd, *oldcmd, world, rules);
    pm.weapon = weapon;
    pmove(&mut pm);
    *oldcmd = *cmd;
    (pm.touch, pm.view_steps)
}

/// [`player_step`] with the weapon half: `defs` are the level's weapon defs
/// by number. Returns the touch list.
pub fn player_step_armed(
    ps: &mut PlayerState,
    cmd: &Cmd,
    oldcmd: &mut Cmd,
    world: &dyn PmWorld,
    rules: Rules,
    weapon: WeaponMove,
    defs: &dyn weapon::WeaponDefs,
) -> Vec<u32> {
    player_step_animated(ps, cmd, oldcmd, world, rules, weapon, defs, None, None).0
}

/// [`player_step_armed`] with the anim script picking the legs and torso
/// anims (the server's step), each event's command drawn off `rand`.
/// Returns the touch list and [`Pmove::mantle_started`].
#[allow(clippy::too_many_arguments)]
pub fn player_step_animated<'a>(
    ps: &'a mut PlayerState,
    cmd: &Cmd,
    oldcmd: &mut Cmd,
    world: &'a dyn PmWorld,
    rules: Rules,
    weapon: WeaponMove,
    defs: &'a dyn weapon::WeaponDefs,
    anim: Option<anim::AnimInput<'a>>,
    rand: Option<&'a mut dyn FnMut() -> i32>,
) -> (Vec<u32>, Option<(Vec3, i32)>) {
    let mut pm = Pmove::new(ps, *cmd, *oldcmd, world, rules);
    pm.weapon = weapon;
    pm.defs = Some(defs);
    pm.anim = anim;
    pm.rand = rand;
    pmove(&mut pm);
    *oldcmd = *cmd;
    (pm.touch, pm.mantle_started)
}

/// Seconds in a millisecond: `PmoveSingle`'s frametime factor (0x8209640,
/// read at 0x805bea3).
const MSEC_TO_SEC: f32 = f32::from_bits(0x3a83_126f);

/// What a frozen player's cmd keeps (`PmoveSingle` 0x805bd42): the stance
/// keys.
const FROZEN_KEEPS: u32 = button::PRONE | button::CROUCH | button::STANCE_HELD;
/// A player just respawned keeps the attack key as well (0x805c359).
const RESPAWNED_KEEPS: u32 = FROZEN_KEEPS | button::ATTACK;
/// A talking player keeps the talk bit, the stance keys, the sight and the
/// sprint (0x805bd79).
const TALKING_KEEPS: u32 = FROZEN_KEEPS | button::TALK | button::ADS_MODE | button::SPRINT;
/// Either key held keeps `PMF_RESPAWNED` on a living player.
const RESPAWN_HOLDS: u32 = button::ATTACK | button::PRONE;

/// One call an arm of `PmoveSingle` makes. Each arm is a fixed list of
/// them, in the order the 1.7 binary makes them (cod4-pmove.md 3).
#[derive(Clone, Copy)]
enum Step {
    /// The sight flag (`weapon::update_ads_flag`).
    Sight,
    Sprint,
    /// `PM_UpdatePlayerWalkingFlag`.
    SightWalk,
    /// `PM_DropTimers`.
    Timers,
    /// `PM_CheckDuck`.
    Duck,
    GroundTrace,
    Footsteps,
    /// `PM_Weapon` with the weapon defs, the breath alone without them.
    Weapon,
    Noclip,
    Fly,
    Mantle,
}

use Step::*;

const LINKED_STEPS: &[Step] = &[Sight, Sprint, SightWalk, Timers, Duck, Footsteps, Weapon];
const GUNNER_STEPS: &[Step] = &[Sight, Sprint, SightWalk, Timers, Duck, Footsteps];
const NOCLIP_STEPS: &[Step] = &[Sight, Sprint, SightWalk, Timers, Noclip];
const SPECTATOR_STEPS: &[Step] = &[Sight, Sprint, SightWalk, Duck, Timers, Fly];
const INTERMISSION_STEPS: &[Step] = &[Sight, Sprint];
/// Ahead of the mantle test, unless a mantle is already running.
const PRE_MANTLE_STEPS: &[Step] = &[Sight, Sprint, SightWalk, Duck, GroundTrace];
const MANTLE_STEPS: &[Step] = &[Sight, Sprint, SightWalk, Duck, Mantle, Weapon];

fn run_steps(pm: &mut Pmove, pml: &mut Pml, steps: &[Step]) {
    for step in steps {
        match step {
            Sight => weapon::update_ads_flag(pm, pml),
            Sprint => update_sprint(pm),
            SightWalk => walking_flag(pm),
            Timers => drop_timers(pm.ps, pml),
            Duck => stance::check_duck(pm, pml),
            GroundTrace => ground::ground_trace(pm, pml),
            Footsteps => footsteps::footsteps(pm, pml),
            Weapon if pm.defs.is_some() => weapon::pm_weapon(pm, pml),
            Weapon => breath::update(pm, pml),
            Noclip => moves::noclip_move(pm, pml),
            Fly => moves::fly_move(pm, pml),
            Mantle => mantle::mantle_move(pm, pml),
        }
    }
}

fn set_bit(word: &mut u32, bit: u32, on: bool) {
    if on {
        *word |= bit;
    } else {
        *word &= !bit;
    }
}

fn stop_keys(cmd: &mut Cmd) {
    cmd.forward_move = 0;
    cmd.right_move = 0;
}

/// The keys a frozen, respawned or talking player may still press
/// (0x805bd2c..0x805bd85); the lunge holds the forward key down.
fn filter_cmd(pm: &mut Pmove) {
    let flags = pm.ps.pm_flags;
    if flags & pmf::MELEE_CHARGE != 0 {
        pm.cmd.forward_move = 127;
    }
    let held = if flags & pmf::FROZEN != 0 {
        Some(FROZEN_KEEPS)
    } else if flags & pmf::RESPAWNED != 0 {
        Some(RESPAWNED_KEEPS)
    } else {
        None
    };
    if let Some(keeps) = held {
        pm.cmd.buttons &= keeps;
        stop_keys(&mut pm.cmd);
        pm.ps.velocity = Vec3::ZERO;
    }
    if pm.cmd.buttons & button::TALK != 0 {
        pm.cmd.buttons &= TALKING_KEEPS;
        stop_keys(&mut pm.cmd);
    }
}

/// `PMF_PRONE_STILL` lives only while prone (cleared at 0x805bda7), and
/// goes once a move key is pushed further than last cmd, taking the sight
/// with it, or once the weapon is idle and unsighted (weapon states up to
/// 4 and 7; the weapon tests are taken to interrupt).
fn update_prone_still(pm: &mut Pmove) {
    let pushed = |now: i8, before: i8| before.unsigned_abs() < now.unsigned_abs();
    let ps = &mut *pm.ps;
    let keep = if ps.pm_flags & pmf::PRONE == 0 {
        false
    } else if pushed(pm.cmd.forward_move, pm.oldcmd.forward_move)
        || pushed(pm.cmd.right_move, pm.oldcmd.right_move)
    {
        ps.pm_flags &= !pmf::SIGHT;
        false
    } else {
        let idle = ps.weapon_state <= 4 || ps.weapon_state == 7;
        ps.pm_flags & pmf::SIGHT != 0 || !idle
    };
    if !keep {
        ps.pm_flags &= !pmf::PRONE_STILL;
    }
}

/// `EF_FIRING` (cleared at 0x805bde0): the attack key on a playing,
/// unrespawned body in a ready (0) or firing (5) weapon state with a round
/// to fire, taken to have one without the weapon defs.
fn firing(pm: &Pmove) -> bool {
    let ps = &*pm.ps;
    pm.cmd.buttons & button::ATTACK != 0
        && ps.pm_type != pm_type::INTERMISSION
        && ps.pm_flags & pmf::RESPAWNED == 0
        && matches!(ps.weapon_state, 0 | 5)
        && pm.defs.is_none_or(|d| weapon::ammo_available(ps, d) != 0)
}

/// A linked player or a gunner: off the ladder and the ground, standing
/// still wherever the engine put him.
fn hold_in_place(pm: &mut Pmove, pml: &mut Pml) {
    leave_ladder(pm.ps);
    pm.ps.ground_entity_num = ENTITYNUM_NONE;
    pm.ps.velocity = Vec3::ZERO;
    pml.walking = false;
    pml.ground_plane = false;
    pml.almost_ground_plane = false;
}

/// `PmoveSingle` (0x805bd14): one chopped step of the cmd. The cmd's keys
/// are filtered and the frame's flags settled first, then the arm for the
/// `pm_type` runs its calls.
pub(crate) fn pmove_single(pm: &mut Pmove) {
    filter_cmd(pm);
    pm.ps.pm_flags &= !pmf::PRONE_REFUSED;
    if pm.ps.pm_type >= pm_type::DEAD {
        pm.tracemask &= !CONTENTS_BODY;
    }
    update_prone_still(pm);
    let stance = pm.ps.effective_stance();
    if stance == Stance::Prone && pm.ps.pm_flags & pmf::SIGHT != 0 {
        stop_keys(&mut pm.cmd);
    }
    let talking = pm.cmd.buttons & button::TALK != 0;
    set_bit(&mut pm.ps.e_flags, ef::TALK, talking);
    let fire = firing(pm);
    set_bit(&mut pm.ps.e_flags, ef::FIRING, fire);
    if pm.ps.pm_type < pm_type::DEAD && pm.cmd.buttons & RESPAWN_HOLDS == 0 {
        pm.ps.pm_flags &= !pmf::RESPAWNED;
    }

    // The step's length: 1..=200 ms.
    let msec = (pm.cmd.server_time - pm.ps.command_time).clamp(1, 200);
    let mut pml = Pml {
        msec,
        frametime: (f64::from(MSEC_TO_SEC) * f64::from(msec)) as f32,
        previous_origin: pm.ps.origin,
        previous_velocity: pm.ps.velocity,
        ..Default::default()
    };
    pm.ps.command_time = pm.cmd.server_time;
    if pm.defs.is_some() {
        weapon::adjust_aim_spread(pm, &pml);
    }
    stance::update_view_angles(pm, pml.msec as f32);
    [pml.forward, pml.right, pml.up] =
        vcod_common::x87::angle_vectors(Vec3::from_array(pm.ps.view_angles));
    // `Mantle_ClearHint` (0x8050d80), every frame before the arms.
    pm.ps.mantle.flags &= !mantle::flag::HINT;

    // The backpedal flag follows the forward key; a strafe alone clears it
    // (0x805bf2d, 0x805c417).
    match pm.cmd.forward_move {
        f if f < 0 => pm.ps.pm_flags |= pmf::BACKWARDS_RUN,
        0 if pm.cmd.right_move == 0 => {}
        _ => pm.ps.pm_flags &= !pmf::BACKWARDS_RUN,
    }
    let pinned = stance == Stance::Prone && pm.ps.pm_flags & pmf::PRONE_STILL != 0;
    if pinned || pm.ps.pm_type >= pm_type::LASTSTAND {
        stop_keys(&mut pm.cmd);
    }
    charge::update(pm, &pml);

    match pm.ps.pm_type {
        pm_type::NORMAL_LINKED | pm_type::DEAD_LINKED => {
            hold_in_place(pm, &mut pml);
            run_steps(pm, &mut pml, LINKED_STEPS);
        }
        pm_type::NOCLIP | pm_type::UFO => {
            leave_ladder(pm.ps);
            run_steps(pm, &mut pml, NOCLIP_STEPS);
        }
        pm_type::SPECTATOR => {
            leave_ladder(pm.ps);
            run_steps(pm, &mut pml, SPECTATOR_STEPS);
        }
        pm_type::INTERMISSION => {
            leave_ladder(pm.ps);
            run_steps(pm, &mut pml, INTERMISSION_STEPS);
        }
        _ => move_body(pm, &mut pml),
    }
}

/// The arm for a body that moves itself: walking, falling, climbing,
/// mantling or dead, with the last stand and the gunner peeled off first.
fn move_body(pm: &mut Pmove, pml: &mut Pml) {
    if pm.ps.pm_type == pm_type::LASTSTAND {
        // The last stand leaves the ladder and any turret.
        leave_ladder(pm.ps);
        pm.ps.e_flags &= !ef::TURRET;
    }
    if pm.ps.e_flags & ef::TURRET != 0 {
        hold_in_place(pm, pml);
        run_steps(pm, pml, GUNNER_STEPS);
        return;
    }
    if pm.ps.pm_flags & pmf::MANTLE == 0 {
        run_steps(pm, pml, PRE_MANTLE_STEPS);
    }
    mantle::check(pm, pml);
    if pm.ps.pm_flags & pmf::MANTLE != 0 {
        leave_ladder(pm.ps);
        pm.ps.ground_entity_num = ENTITYNUM_NONE;
        pml.walking = false;
        pml.ground_plane = false;
        run_steps(pm, pml, MANTLE_STEPS);
        return;
    }
    stance::update_prone_pitch(pm, pml);
    drop_timers(pm.ps, pml);
    if pm.ps.pm_type >= pm_type::LASTSTAND {
        moves::dead_move(pm.ps, pml);
    }
    ladder::check_ladder_move(pm, pml);
    if pm.ps.pm_flags & pmf::LADDER != 0 {
        ladder::ladder_move(pm, pml);
    } else if pml.walking {
        moves::walk_move(pm, pml);
    } else {
        moves::air_move(pm, pml);
    }
    ground::ground_trace(pm, pml);
    footsteps::footsteps(pm, pml);
    run_steps(pm, pml, &[Weapon]);
    foliage_sound(pm);
    settle_velocity(pm.ps, pml);
}

/// `PM_CanStand`: standing room at the origin, bodies aside.
fn can_stand(pm: &Pmove) -> bool {
    if pm.ps.pm_flags & (pmf::PRONE | pmf::DUCKED) == 0 {
        return true;
    }
    let o = pm.ps.origin;
    !pm.raw_trace(
        o,
        o,
        PLAYER_MINS,
        PLAYER_MAXS,
        pm.tracemask & !CONTENTS_BODY,
    )
    .allsolid
}

/// The reloading weapon states, which keep the sighted walk off.
fn reloading(state: i32) -> bool {
    matches!(state, 7..=11)
}

/// `PM_UpdatePlayerWalkingFlag`: the sighted walk follows the held sight
/// key on a sighted, unprone player who is not reloading.
fn walking_flag(pm: &mut Pmove) {
    let ps = &mut *pm.ps;
    ps.pm_flags &= !pmf::SIGHT_WALK;
    if ps.pm_type < pm_type::DEAD
        && pm.cmd.buttons & button::ADS_MODE != 0
        && ps.pm_flags & pmf::PRONE == 0
        && ps.pm_flags & pmf::SIGHT != 0
        && !reloading(ps.weapon_state)
    {
        ps.pm_flags |= pmf::SIGHT_WALK;
    }
}

/// `PM_ClearLadderFlag`: a player taken off the ladder notes it.
pub(crate) fn leave_ladder(ps: &mut PlayerState) {
    if ps.pm_flags & pmf::LADDER != 0 {
        ps.pm_flags |= pmf::LADDER_LEFT;
    }
    ps.pm_flags &= !pmf::LADDER;
}

/// `PM_DropTimers`: `pm_time` runs out, and with it the land, knockback
/// and jump timers' flags (the jump's state cleared); the legs and torso
/// timers run down to 0.
fn drop_timers(ps: &mut PlayerState, pml: &Pml) {
    if ps.pm_time != 0 {
        if pml.msec < ps.pm_time {
            ps.pm_time -= pml.msec;
        } else {
            if ps.pm_flags & pmf::JUMPING != 0 {
                slide::jump_clear(ps);
            }
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

/// `perks` bit of `specialty_longersprint`.
const PERK_LONGER_SPRINT: u32 = 0x400;
/// The longest sprint in ms (`BG_GetMaxSprintTime` 0x80526f6).
const MAX_SPRINT_MS: i32 = 0x3fff;
/// Keys that keep a sprint from starting (`PM_UpdateSprint` 0x805718d):
/// fire, melee, both reloads, the jump and both grenades.
const SPRINT_BLOCKERS: u32 = button::ATTACK
    | button::MELEE
    | button::RELOAD
    | button::USE_RELOAD
    | button::JUMP
    | button::FRAG
    | button::SMOKE;
/// Keys that end a running sprint (0x80572af): the stance keys as well.
const SPRINT_BREAKERS: u32 = SPRINT_BLOCKERS | button::CROUCH | button::PRONE;
/// `pm_flags` that refuse a sprint (0x80571b0) and that end one
/// (0x805728c).
const SPRINT_REFUSERS: u32 = pmf::MANTLE | pmf::LADDER | pmf::SIGHT | pmf::SHELLSHOCKED;
const SPRINT_ENDERS: u32 = pmf::LADDER | pmf::SIGHT | pmf::SHELLSHOCKED;

/// `BG_GetMaxSprintTime` (0x80526a4): `player_sprintTime` in ms, stored,
/// times the weapon's `sprintDurationScale` and under the perk
/// `perk_sprintMultiplier`, truncated and capped.
fn max_sprint_time(pm: &Pmove) -> i32 {
    let base = (f64::from(pm.rules.sprint_time) * 1000.0) as f32;
    let mut ms = f64::from(base) * f64::from(pm.weapon.sprint_duration_scale);
    if pm.ps.perks & PERK_LONGER_SPRINT != 0 {
        ms *= f64::from(pm.rules.perk_sprint_multiplier);
    }
    (ms as f32 as i32).min(MAX_SPRINT_MS)
}

/// `player_sprintRechargePause` in ms, as the binary compares it, unrounded.
fn recharge_pause_ms(pm: &Pmove) -> f64 {
    f64::from(pm.rules.sprint_recharge_pause) * 1000.0
}

/// `PM_GetSprintLeft`: the ms of sprint the body holds at `now`, between 0
/// and the longest. A sprint running drains its allowance; once it ends,
/// what it left recharges a ms per ms, less the recharge pause (stored and
/// truncated, 0x80573cb) after one that ran out.
fn sprint_left(pm: &Pmove, now: i32) -> i32 {
    let longest = max_sprint_time(pm);
    let s = pm.ps.sprint;
    let left = if s.last_start == 0 {
        longest
    } else if s.last_start > s.last_end {
        s.start_max_length - (now - s.last_start)
    } else {
        let unused = s.start_max_length - (s.last_end - s.last_start);
        let pause = if s.delay {
            recharge_pause_ms(pm) as f32 as i32
        } else {
            0
        };
        unused + (now - s.last_end) - pause
    };
    left.max(0).min(longest)
}

/// `PM_EndSprint` (inlined at 0x80570d7): a running sprint stops at the
/// cmd's time with no pause owed, and a held key has to come up before the
/// next.
fn end_sprint(pm: &mut Pmove) {
    if pm.ps.pm_flags & pmf::SPRINTING == 0 {
        return;
    }
    let held = pm.cmd.buttons & button::SPRINT != 0;
    let s = &mut pm.ps.sprint;
    s.last_end = pm.cmd.server_time;
    s.delay = false;
    s.button_up_required |= held;
    pm.ps.pm_flags &= !pmf::SPRINTING;
}

/// The cmd leaves room to sprint: the forward key past
/// `player_sprintForwardMinimum`, none of `keys` and no lean.
fn sprint_posture(pm: &Pmove, keys: u32) -> bool {
    i32::from(pm.cmd.forward_move) > pm.rules.sprint_forward_minimum
        && pm.cmd.buttons & keys == 0
        && pm.ps.leanf == 0.0
}

/// The melee and offhand weapon states (12..=20).
fn busy_hands(state: i32) -> bool {
    (12..=20).contains(&state)
}

/// `PM_UpdateSprint` (0x80570ae). Only a normal player (linked or not)
/// sprints, and only with a sprint to give.
fn update_sprint(pm: &mut Pmove) {
    if pm.cmd.buttons & button::SPRINT == 0 {
        pm.ps.sprint.button_up_required = false;
    }
    if pm.ps.pm_type > pm_type::NORMAL_LINKED || max_sprint_time(pm) <= 0 {
        end_sprint(pm);
    } else if pm.ps.pm_flags & pmf::SPRINTING == 0 {
        start_sprint(pm);
    } else {
        keep_sprint(pm);
    }
}

/// A running sprint ends when its allowance is spent (and owes the pause,
/// 0x8057276), when the body or the keys break it (weapon states 25 and 26
/// as well as the busy hands), or on a fresh press of the key.
fn keep_sprint(pm: &mut Pmove) {
    let ps = &*pm.ps;
    let now = pm.cmd.server_time;
    if now - ps.sprint.last_start >= ps.sprint.start_max_length {
        end_sprint(pm);
        pm.ps.sprint.delay = true;
        return;
    }
    let broken = ps.pm_flags & SPRINT_ENDERS != 0
        || !sprint_posture(pm, SPRINT_BREAKERS)
        || busy_hands(ps.weapon_state)
        || matches!(ps.weapon_state, 25 | 26);
    let pressed = pm.cmd.buttons & button::SPRINT != 0 && pm.oldcmd.buttons & button::SPRINT == 0;
    if broken {
        end_sprint(pm);
    } else if pressed {
        end_sprint(pm);
        pm.ps.sprint.button_up_required = true;
    }
}

/// The key starts a sprint once any pause has run, when nothing refuses it,
/// the body could stand (a jump's landed hands aside, a busy weapon
/// refuses), and more than `player_sprintMinTime` is left.
fn start_sprint(pm: &mut Pmove) {
    let ps = &*pm.ps;
    let now = pm.cmd.server_time;
    let rested = !ps.sprint.delay || recharge_pause_ms(pm) <= f64::from(now - ps.sprint.last_end);
    let landed = ps.pm_flags & pmf::JUMPING != 0 && ps.pm_time == 0;
    let allowed = rested
        && pm.cmd.buttons & button::SPRINT != 0
        && ps.pm_flags & (pmf::NO_SPRINT | SPRINT_REFUSERS) == 0
        && !ps.sprint.button_up_required
        && sprint_posture(pm, SPRINT_BLOCKERS)
        && (landed || !busy_hands(ps.weapon_state))
        && can_stand(pm);
    if !allowed {
        return;
    }
    let left = sprint_left(pm, now);
    if f64::from(left) > f64::from(pm.rules.sprint_min_time) * 1000.0 {
        let ps = &mut *pm.ps;
        ps.sprint.start_max_length = left;
        ps.sprint.last_start = now;
        ps.pm_flags |= pmf::SPRINTING;
    }
}

/// The `bg_foliagesnd_*` dvars' defaults (`minspeed`, `maxspeed`,
/// `slowinterval`, `fastinterval`, `resetinterval`).
const FOLIAGE_MIN_SPEED: f32 = 40.0;
const FOLIAGE_MAX_SPEED: f32 = 180.0;
const FOLIAGE_SLOW_MS: i32 = 1500;
const FOLIAGE_FAST_MS: i32 = 500;
const FOLIAGE_RESET_MS: i32 = 500;
/// The probe box: the body's scaled by 0.75 across (0x820a328) and 0.9
/// tall (0x820df28).
const FOLIAGE_BOX_SCALE: f32 = 0.75;
const FOLIAGE_BOX_HEIGHT: f32 = f32::from_bits(0x3f66_6666);
const CONTENTS_FOLIAGE: u32 = 2;

/// `PM_FoliageSounds` (inlined at 0x805d05e): a body moving at 40 or more
/// raises `EV_FOLIAGE_SOUND` each time its interval (1.5 s slow, down to
/// 0.5 s at 180) has passed while its probe box starts in foliage; a
/// slower body clears the timer once it is half a second old.
fn foliage_sound(pm: &mut Pmove) {
    let now = pm.cmd.server_time;
    let last = pm.ps.foliage_sound_time;
    if pm.xyspeed < FOLIAGE_MIN_SPEED {
        if last + FOLIAGE_RESET_MS < now {
            pm.ps.foliage_sound_time = 0;
        }
        return;
    }
    let span = f64::from(FOLIAGE_MAX_SPEED) - f64::from(FOLIAGE_MIN_SPEED);
    let t = ((f64::from(pm.xyspeed) - f64::from(FOLIAGE_MIN_SPEED)) / span).min(1.0);
    let interval =
        (f64::from(FOLIAGE_SLOW_MS) + f64::from(FOLIAGE_FAST_MS - FOLIAGE_SLOW_MS) * t) as f32;
    if last + interval as i32 >= now {
        return;
    }
    let mins = fpu::scale(pm.mins, FOLIAGE_BOX_SCALE);
    let maxs = fpu::scale(pm.maxs, FOLIAGE_BOX_SCALE)
        .with_z((f64::from(FOLIAGE_BOX_HEIGHT) * f64::from(pm.maxs.z)) as f32);
    let at = pm.ps.origin;
    if pm
        .trace_box(at, at, mins, maxs, CONTENTS_FOLIAGE)
        .startsolid
    {
        pm.ps.add_event(ev::FOLIAGE_SOUND, 0);
        pm.ps.foliage_sound_time = now;
    }
}

/// The ratio of the squared speeds below which the frame's displacement
/// replaces the velocity (0x820966c).
const STALL_RATIO: f64 = 0.25;

/// The moving arm's end (0x805d091): a velocity far above what the frame
/// moved (blocked) becomes the displacement over the frame, `oldVelocity`
/// eases toward it by the frame's share of a second, and the velocity is
/// snapped to whole units (`rint`).
fn settle_velocity(ps: &mut PlayerState, pml: &Pml) {
    // The displacement and both squares stay unrounded on the x87 stack.
    let ft = f64::from(pml.frametime);
    let moved = ps.origin.as_dvec3() - pml.previous_origin.as_dvec3();
    let speed_sq = ps.velocity.as_dvec3().length_squared();
    if moved.length_squared() / (ft * ft) < STALL_RATIO * speed_sq {
        ps.velocity = (moved * (1.0 / ft)).as_vec3();
    }
    // Each product stored (0x805d175, 0x805d184).
    let share = f64::from(pml.frametime.min(1.0));
    for (old, &now) in ps.old_velocity.iter_mut().zip(&ps.velocity.to_array()[..2]) {
        let gap = (f64::from(now) - f64::from(*old)) as f32;
        let step = (share * f64::from(gap)) as f32;
        *old = (f64::from(*old) + f64::from(step)) as f32;
    }
    ps.velocity = ps.velocity.map(f32::round_ties_even);
}

#[cfg(test)]
mod tests;
