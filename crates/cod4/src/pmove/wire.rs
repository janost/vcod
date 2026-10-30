//! The pmove playerstate to and from 1.7's netfield words
//! (`net::delta::PlayerState`, docs/research/cod4-netfields.md 1).

use glam::Vec3;

use super::{PlayerState, SprintState};
use crate::net::delta;
use crate::net::fields::PLAYER_STATE;

/// `name`'s word in a wire playerstate, looked up at compile time
/// through [`ix!`]: a name `PLAYER_STATE` lacks fails the build.
const fn index(name: &str) -> usize {
    let mut i = 0;
    while i < PLAYER_STATE.len() {
        let (a, b) = (PLAYER_STATE[i].name.as_bytes(), name.as_bytes());
        if a.len() == b.len() {
            let mut k = 0;
            while k < a.len() && a[k] == b[k] {
                k += 1;
            }
            if k == a.len() {
                return PLAYER_STATE[i].offset as usize / 4;
            }
        }
        i += 1;
    }
    panic!("no such playerstate field")
}

/// A field's word, or an array field's words (`ix!(3 "origin")` for
/// `origin[0..3]`), as constants.
macro_rules! ix {
    ($name:literal) => {
        const { index($name) }
    };
    (3 $name:literal) => {
        const {
            [
                index(concat!($name, "[0]")),
                index(concat!($name, "[1]")),
                index(concat!($name, "[2]")),
            ]
        }
    };
    (4 $name:literal) => {
        const {
            [
                index(concat!($name, "[0]")),
                index(concat!($name, "[1]")),
                index(concat!($name, "[2]")),
                index(concat!($name, "[3]")),
            ]
        }
    };
}

fn word(w: &delta::PlayerState, i: usize) -> u32 {
    w.words[i]
}

fn int(w: &delta::PlayerState, i: usize) -> i32 {
    word(w, i) as i32
}

fn float(w: &delta::PlayerState, i: usize) -> f32 {
    f32::from_bits(word(w, i))
}

fn vec3(w: &delta::PlayerState, i: [usize; 3]) -> Vec3 {
    Vec3::new(float(w, i[0]), float(w, i[1]), float(w, i[2]))
}

fn set(w: &mut delta::PlayerState, i: usize, v: u32) {
    w.words[i] = v;
}

fn set_f(w: &mut delta::PlayerState, i: usize, v: f32) {
    set(w, i, v.to_bits());
}

fn set_vec3(w: &mut delta::PlayerState, i: [usize; 3], v: Vec3) {
    for k in 0..3 {
        set_f(w, i[k], v[k]);
    }
}

impl PlayerState {
    /// The pmove fields off a wire playerstate; `oldVelocity` is not on
    /// the wire and comes back 0.
    pub fn from_wire(w: &delta::PlayerState) -> PlayerState {
        let arr4 = |i: [usize; 4]| i.map(|i| word(w, i));
        PlayerState {
            command_time: int(w, ix!("commandTime")),
            pm_type: int(w, ix!("pm_type")),
            bob_cycle: int(w, ix!("bobCycle")),
            pm_flags: word(w, ix!("pm_flags")),
            pm_time: int(w, ix!("pm_time")),
            origin: vec3(w, ix!(3 "origin")),
            velocity: vec3(w, ix!(3 "velocity")),
            old_velocity: [0.0; 2],
            gravity: int(w, ix!("gravity")),
            leanf: float(w, ix!("leanf")),
            speed: int(w, ix!("speed")),
            delta_angles: ix!(3 "delta_angles").map(|i| float(w, i)),
            ground_entity_num: int(w, ix!("groundEntityNum")),
            ladder_vec: vec3(w, ix!(3 "vLadderVec")),
            jump_time: int(w, ix!("jumpTime")),
            jump_origin_z: float(w, ix!("jumpOriginZ")),
            movement_dir: int(w, ix!("movementDir")),
            e_flags: word(w, ix!("eFlags")),
            event_sequence: int(w, ix!("eventSequence")),
            events: arr4(ix!(4 "events")),
            event_parms: arr4(ix!(4 "eventParms")),
            client_num: word(w, ix!("clientNum")),
            weapon: word(w, ix!("weapon")),
            weapon_state: int(w, ix!("weaponstate")),
            weapon_pos_frac: float(w, ix!("fWeaponPosFrac")),
            view_angles: ix!(3 "viewangles").map(|i| float(w, i)),
            view_height_target: int(w, ix!("viewHeightTarget")),
            view_height_current: float(w, ix!("viewHeightCurrent")),
            view_height_lerp_time: int(w, ix!("viewHeightLerpTime")),
            view_height_lerp_target: int(w, ix!("viewHeightLerpTarget")),
            view_height_lerp_down: word(w, ix!("viewHeightLerpDown")) != 0,
            damage_timer: int(w, ix!("damageTimer")),
            damage_duration: int(w, ix!("damageDuration")),
            view_angle_clamp_base: [
                float(w, ix!("viewAngleClampBase[0]")),
                float(w, ix!("viewAngleClampBase[1]")),
            ],
            view_angle_clamp_range: [
                float(w, ix!("viewAngleClampRange[0]")),
                float(w, ix!("viewAngleClampRange[1]")),
            ],
            prone_direction: float(w, ix!("proneDirection")),
            prone_direction_pitch: float(w, ix!("proneDirectionPitch")),
            prone_torso_pitch: float(w, ix!("proneTorsoPitch")),
            torso_pitch: float(w, ix!("fTorsoPitch")),
            waist_pitch: float(w, ix!("fWaistPitch")),
            aim_spread_scale: float(w, ix!("aimSpreadScale")),
            foliage_sound_time: int(w, ix!("foliageSoundTime")),
            legs_timer: int(w, ix!("legsTimer")),
            torso_timer: int(w, ix!("torsoTimer")),
            legs_anim: word(w, ix!("legsAnim")),
            torso_anim: word(w, ix!("torsoAnim")),
            flinch_yaw_anim: word(w, ix!("flinchYawAnim")),
            anim_movetype: 0,
            anim_strafing: 0,
            sprint: SprintState {
                button_up_required: word(w, ix!("sprintState.sprintButtonUpRequired")) != 0,
                delay: word(w, ix!("sprintState.sprintDelay")) != 0,
                last_start: int(w, ix!("sprintState.lastSprintStart")),
                last_end: int(w, ix!("sprintState.lastSprintEnd")),
                start_max_length: int(w, ix!("sprintState.sprintStartMaxLength")),
            },
            move_speed_scale_multiplier: float(w, ix!("moveSpeedScaleMultiplier")),
            shellshock_index: int(w, ix!("shellshockIndex")),
            perks: word(w, ix!("perks")),
            weap_flags: word(w, ix!("weapFlags")),
            hold_breath_scale: float(w, ix!("holdBreathScale")),
            hold_breath_timer: int(w, ix!("holdBreathTimer")),
            weapon_time: int(w, ix!("weaponTime")),
            weapon_delay: int(w, ix!("weaponDelay")),
            weapon_shot_count: int(w, ix!("weaponShotCount")),
            weap_anim: word(w, ix!("weapAnim")),
            weapon_restrict_kick_time: int(w, ix!("weaponRestrictKickTime")),
            spread_override: int(w, ix!("spreadOverride")),
            spread_override_state: int(w, ix!("spreadOverrideState")),
            ads_delay_time: int(w, ix!("adsDelayTime")),
            grenade_time_left: int(w, ix!("grenadeTimeLeft")),
            other_flags: word(w, ix!("otherFlags")),
            off_hand_index: word(w, ix!("offHandIndex")),
            offhand_secondary: word(w, ix!("offhandSecondary")),
            throw_back_grenade_owner: int(w, ix!("throwBackGrenadeOwner")),
            throw_back_grenade_time_left: int(w, ix!("throwBackGrenadeTimeLeft")),
            cursor_hint_ent_index: int(w, ix!("cursorHintEntIndex")),
            melee_charge_yaw: float(w, ix!("meleeChargeYaw")),
            melee_charge_dist: int(w, ix!("meleeChargeDist")),
            melee_charge_time: int(w, ix!("meleeChargeTime")),
            weapons: arr4(ix!(4 "weapons")),
            weaponold: arr4(ix!(4 "weaponold")),
            weaponrechamber: arr4(ix!(4 "weaponrechamber")),
            ammo: w.ammo,
            ammoclip: w.ammoclip,
            mantle: super::mantle::MantleState {
                yaw: float(w, ix!("mantleState.yaw")),
                timer: int(w, ix!("mantleState.timer")),
                trans_index: int(w, ix!("mantleState.transIndex")),
                flags: word(w, ix!("mantleState.flags")),
            },
        }
    }

    /// `base` with every field pmove owns written from `self`: what a
    /// server sends and a predictor rebuilds from.
    pub fn to_wire(&self, base: &delta::PlayerState) -> delta::PlayerState {
        let mut w = base.clone();
        set(&mut w, ix!("commandTime"), self.command_time as u32);
        set(&mut w, ix!("pm_type"), self.pm_type as u32);
        set(&mut w, ix!("bobCycle"), self.bob_cycle as u32);
        set(&mut w, ix!("pm_flags"), self.pm_flags);
        set(&mut w, ix!("pm_time"), self.pm_time as u32);
        set_vec3(&mut w, ix!(3 "origin"), self.origin);
        set_vec3(&mut w, ix!(3 "velocity"), self.velocity);
        set(&mut w, ix!("gravity"), self.gravity as u32);
        set_f(&mut w, ix!("leanf"), self.leanf);
        set(&mut w, ix!("speed"), self.speed as u32);
        for i in 0..3 {
            set_f(&mut w, ix!(3 "delta_angles")[i], self.delta_angles[i]);
            set_f(&mut w, ix!(3 "viewangles")[i], self.view_angles[i]);
        }
        set(
            &mut w,
            ix!("groundEntityNum"),
            self.ground_entity_num as u32,
        );
        set_vec3(&mut w, ix!(3 "vLadderVec"), self.ladder_vec);
        set(&mut w, ix!("jumpTime"), self.jump_time as u32);
        set_f(&mut w, ix!("jumpOriginZ"), self.jump_origin_z);
        set(&mut w, ix!("movementDir"), self.movement_dir as u32);
        set(&mut w, ix!("eFlags"), self.e_flags);
        set(&mut w, ix!("eventSequence"), self.event_sequence as u32);
        for i in 0..4 {
            set(&mut w, ix!(4 "events")[i], self.events[i]);
            set(&mut w, ix!(4 "eventParms")[i], self.event_parms[i]);
        }
        set(
            &mut w,
            ix!("viewHeightTarget"),
            self.view_height_target as u32,
        );
        set_f(&mut w, ix!("viewHeightCurrent"), self.view_height_current);
        set(
            &mut w,
            ix!("viewHeightLerpTime"),
            self.view_height_lerp_time as u32,
        );
        set(
            &mut w,
            ix!("viewHeightLerpTarget"),
            self.view_height_lerp_target as u32,
        );
        set(
            &mut w,
            ix!("viewHeightLerpDown"),
            u32::from(self.view_height_lerp_down),
        );
        set(&mut w, ix!("damageTimer"), self.damage_timer as u32);
        set_f(&mut w, ix!("proneDirection"), self.prone_direction);
        set_f(
            &mut w,
            ix!("proneDirectionPitch"),
            self.prone_direction_pitch,
        );
        set_f(&mut w, ix!("proneTorsoPitch"), self.prone_torso_pitch);
        set_f(&mut w, ix!("fTorsoPitch"), self.torso_pitch);
        set_f(&mut w, ix!("fWaistPitch"), self.waist_pitch);
        set_f(&mut w, ix!("aimSpreadScale"), self.aim_spread_scale);
        set(
            &mut w,
            ix!("foliageSoundTime"),
            self.foliage_sound_time as u32,
        );
        set(&mut w, ix!("legsTimer"), self.legs_timer as u32);
        set(&mut w, ix!("torsoTimer"), self.torso_timer as u32);
        set(&mut w, ix!("legsAnim"), self.legs_anim);
        set(&mut w, ix!("torsoAnim"), self.torso_anim);
        let s = &self.sprint;
        set(
            &mut w,
            ix!("sprintState.sprintButtonUpRequired"),
            u32::from(s.button_up_required),
        );
        set(&mut w, ix!("sprintState.sprintDelay"), u32::from(s.delay));
        set(
            &mut w,
            ix!("sprintState.lastSprintStart"),
            s.last_start as u32,
        );
        set(&mut w, ix!("sprintState.lastSprintEnd"), s.last_end as u32);
        set(
            &mut w,
            ix!("sprintState.sprintStartMaxLength"),
            s.start_max_length as u32,
        );
        set(&mut w, ix!("weapFlags"), self.weap_flags);
        set_f(&mut w, ix!("holdBreathScale"), self.hold_breath_scale);
        set(
            &mut w,
            ix!("holdBreathTimer"),
            self.hold_breath_timer as u32,
        );
        set(&mut w, ix!("weapon"), self.weapon);
        set(&mut w, ix!("weaponstate"), self.weapon_state as u32);
        set_f(&mut w, ix!("fWeaponPosFrac"), self.weapon_pos_frac);
        set(&mut w, ix!("weaponTime"), self.weapon_time as u32);
        set(&mut w, ix!("weaponDelay"), self.weapon_delay as u32);
        set_f(&mut w, ix!("meleeChargeYaw"), self.melee_charge_yaw);
        set(
            &mut w,
            ix!("meleeChargeDist"),
            self.melee_charge_dist as u32,
        );
        set(
            &mut w,
            ix!("meleeChargeTime"),
            self.melee_charge_time as u32,
        );
        set(
            &mut w,
            ix!("weaponShotCount"),
            self.weapon_shot_count as u32,
        );
        set(&mut w, ix!("weapAnim"), self.weap_anim);
        set(
            &mut w,
            ix!("weaponRestrictKickTime"),
            self.weapon_restrict_kick_time as u32,
        );
        set(
            &mut w,
            ix!("spreadOverrideState"),
            self.spread_override_state as u32,
        );
        set(&mut w, ix!("adsDelayTime"), self.ads_delay_time as u32);
        set(
            &mut w,
            ix!("grenadeTimeLeft"),
            self.grenade_time_left as u32,
        );
        set(&mut w, ix!("offHandIndex"), self.off_hand_index);
        set(
            &mut w,
            ix!("throwBackGrenadeOwner"),
            self.throw_back_grenade_owner as u32,
        );
        set(
            &mut w,
            ix!("throwBackGrenadeTimeLeft"),
            self.throw_back_grenade_time_left as u32,
        );
        for i in 0..4 {
            set(&mut w, ix!(4 "weapons")[i], self.weapons[i]);
            set(&mut w, ix!(4 "weaponold")[i], self.weaponold[i]);
            set(&mut w, ix!(4 "weaponrechamber")[i], self.weaponrechamber[i]);
        }
        w.ammo = self.ammo;
        w.ammoclip = self.ammoclip;
        set_f(&mut w, ix!("mantleState.yaw"), self.mantle.yaw);
        set(&mut w, ix!("mantleState.timer"), self.mantle.timer as u32);
        set(
            &mut w,
            ix!("mantleState.transIndex"),
            self.mantle.trans_index as u32,
        );
        set(&mut w, ix!("mantleState.flags"), self.mantle.flags);
        w
    }
}
