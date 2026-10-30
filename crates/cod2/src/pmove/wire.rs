//! pmove's playerstate to and from the wire's netfield words
//! (`net::fields::PLAYER_STATE_FIELDS`), for a replay of a retail capture
//! and for prediction off a snapshot. `oldVelocity` and the anim
//! conditions are not on the wire.

use glam::Vec3;

use super::PlayerState;
use crate::net::PlayerState2;
use crate::net::delta::field_index;
use crate::net::fields::PLAYER_STATE_FIELDS;

fn word(ps: &PlayerState2, name: &str) -> u32 {
    ps.f[field_index(&PLAYER_STATE_FIELDS, name)]
}

fn float(ps: &PlayerState2, name: &str) -> f32 {
    f32::from_bits(word(ps, name))
}

fn vec3(ps: &PlayerState2, name: &str) -> Vec3 {
    Vec3::new(
        float(ps, &format!("{name}[0]")),
        float(ps, &format!("{name}[1]")),
        float(ps, &format!("{name}[2]")),
    )
}

fn set(ps: &mut PlayerState2, name: &str, v: u32) {
    ps.f[field_index(&PLAYER_STATE_FIELDS, name)] = v;
}

fn set_vec3(ps: &mut PlayerState2, name: &str, v: Vec3) {
    for i in 0..3 {
        set(ps, &format!("{name}[{i}]"), v[i].to_bits());
    }
}

impl PlayerState {
    /// The fields pmove reads, off a snapshot's playerstate; `old_velocity`
    /// is left 0, a still body's.
    pub fn from_wire(w: &PlayerState2) -> PlayerState {
        let i = |n: &str| word(w, n) as i32;
        let arr4 = |n: &str| std::array::from_fn(|k| i(&format!("{n}[{k}]")));
        PlayerState {
            command_time: i("commandTime"),
            pm_type: i("pm_type"),
            bob_cycle: i("bobCycle"),
            pm_flags: word(w, "pm_flags"),
            pm_time: i("pm_time"),
            origin: vec3(w, "origin"),
            velocity: vec3(w, "velocity"),
            old_velocity: [0.0; 2],
            gravity: i("gravity"),
            leanf: float(w, "leanf"),
            speed: i("speed"),
            delta_angles: std::array::from_fn(|k| i(&format!("delta_angles[{k}]"))),
            ground_entity_num: i("groundEntityNum"),
            ladder_vec: vec3(w, "vLadderVec"),
            jump_time: i("jumpTime"),
            jump_origin_z: float(w, "jumpOriginZ"),
            movement_dir: i("movementDir"),
            e_flags: word(w, "eFlags"),
            event_sequence: i("eventSequence"),
            events: arr4("events"),
            event_parms: arr4("eventParms"),
            client_num: word(w, "clientNum"),
            weapon_state: i("weaponstate"),
            view_angles: vec3(w, "viewangles").to_array(),
            view_height_target: i("viewHeightTarget"),
            view_height_current: float(w, "viewHeightCurrent"),
            view_height_lerp_time: i("viewHeightLerpTime"),
            view_height_lerp_target: i("viewHeightLerpTarget"),
            view_height_lerp_down: i("viewHeightLerpDown") != 0,
            mins: vec3(w, "mins"),
            maxs: vec3(w, "maxs"),
            prone_direction: float(w, "proneDirection"),
            prone_direction_pitch: float(w, "proneDirectionPitch"),
            prone_torso_pitch: float(w, "proneTorsoPitch"),
            prone_valid: [
                float(w, "fTorsoHeight"),
                float(w, "fTorsoPitch"),
                float(w, "fWaistPitch"),
            ],
            aim_spread_scale: float(w, "aimSpreadScale"),
            damage_timer: i("damageTimer"),
            view_angle_clamp_base: [
                float(w, "viewAngleClampBase[0]"),
                float(w, "viewAngleClampBase[1]"),
            ],
            view_angle_clamp_range: [
                float(w, "viewAngleClampRange[0]"),
                float(w, "viewAngleClampRange[1]"),
            ],
            damage_duration: i("damageDuration"),
            // 16 bits on the wire, a signed yaw in memory.
            flinch_yaw: i("flinchYaw") as u16 as i16 as i32,
            mantle: super::mantle::MantleState {
                yaw: float(w, "mantleState.yaw"),
                timer: i("mantleState.timer"),
                trans_index: i("mantleState.transIndex"),
                flags: word(w, "mantleState.flags"),
            },
            legs_timer: i("legsTimer"),
            legs_anim: i("legsAnim"),
            torso_timer: i("torsoTimer"),
            torso_anim: i("torsoAnim"),
            anim_movetype: 0,
            anim_strafing: 0,
            foliage_sound_time: i("foliageSoundTime"),
        }
    }

    /// `base` with the fields [`PlayerState::from_wire`] reads written from
    /// `self`, as full words (an angle is not quantized here).
    pub fn to_wire(&self, base: &PlayerState2) -> PlayerState2 {
        let mut w = base.clone();
        let s = &mut w;
        set(s, "commandTime", self.command_time as u32);
        set(s, "pm_type", self.pm_type as u32);
        set(s, "bobCycle", self.bob_cycle as u32);
        set(s, "pm_flags", self.pm_flags);
        set(s, "pm_time", self.pm_time as u32);
        set_vec3(s, "origin", self.origin);
        set_vec3(s, "velocity", self.velocity);
        set(s, "gravity", self.gravity as u32);
        set(s, "leanf", self.leanf.to_bits());
        set(s, "speed", self.speed as u32);
        for k in 0..3 {
            set(
                s,
                &format!("delta_angles[{k}]"),
                self.delta_angles[k] as u32,
            );
        }
        set(s, "groundEntityNum", self.ground_entity_num as u32);
        set_vec3(s, "vLadderVec", self.ladder_vec);
        set(s, "jumpTime", self.jump_time as u32);
        set(s, "jumpOriginZ", self.jump_origin_z.to_bits());
        set(s, "movementDir", self.movement_dir as u32);
        set(s, "eFlags", self.e_flags);
        set(s, "eventSequence", self.event_sequence as u32);
        for k in 0..4 {
            set(s, &format!("events[{k}]"), self.events[k] as u32);
            set(s, &format!("eventParms[{k}]"), self.event_parms[k] as u32);
        }
        set(s, "clientNum", self.client_num);
        set(s, "weaponstate", self.weapon_state as u32);
        set_vec3(s, "viewangles", Vec3::from_array(self.view_angles));
        set(s, "viewHeightTarget", self.view_height_target as u32);
        set(s, "viewHeightCurrent", self.view_height_current.to_bits());
        set(s, "viewHeightLerpTime", self.view_height_lerp_time as u32);
        set(
            s,
            "viewHeightLerpTarget",
            self.view_height_lerp_target as u32,
        );
        set(
            s,
            "viewHeightLerpDown",
            u32::from(self.view_height_lerp_down),
        );
        set_vec3(s, "mins", self.mins);
        set_vec3(s, "maxs", self.maxs);
        set(s, "proneDirection", self.prone_direction.to_bits());
        set(
            s,
            "proneDirectionPitch",
            self.prone_direction_pitch.to_bits(),
        );
        set(s, "proneTorsoPitch", self.prone_torso_pitch.to_bits());
        set(s, "fTorsoHeight", self.prone_valid[0].to_bits());
        set(s, "fTorsoPitch", self.prone_valid[1].to_bits());
        set(s, "fWaistPitch", self.prone_valid[2].to_bits());
        set(s, "aimSpreadScale", self.aim_spread_scale.to_bits());
        set(s, "damageTimer", self.damage_timer as u32);
        for k in 0..2 {
            let (b, r) = (
                self.view_angle_clamp_base[k],
                self.view_angle_clamp_range[k],
            );
            set(s, &format!("viewAngleClampBase[{k}]"), b.to_bits());
            set(s, &format!("viewAngleClampRange[{k}]"), r.to_bits());
        }
        set(s, "damageDuration", self.damage_duration as u32);
        set(s, "flinchYaw", self.flinch_yaw as u32);
        set(s, "mantleState.yaw", self.mantle.yaw.to_bits());
        set(s, "mantleState.timer", self.mantle.timer as u32);
        set(s, "mantleState.transIndex", self.mantle.trans_index as u32);
        set(s, "mantleState.flags", self.mantle.flags);
        set(s, "legsTimer", self.legs_timer as u32);
        set(s, "legsAnim", self.legs_anim as u32);
        set(s, "torsoTimer", self.torso_timer as u32);
        set(s, "torsoAnim", self.torso_anim as u32);
        set(s, "foliageSoundTime", self.foliage_sound_time as u32);
        w
    }
}

impl super::weapon::WeaponState {
    /// `PM_Weapon`'s half of a snapshot's playerstate. `restrictKickTime`
    /// and `adsDelayTime` are left 0, as the A/B's resync keeps its own.
    pub fn from_wire(w: &PlayerState2) -> Self {
        // The 16-bit timers come back sign-extended.
        let short = |n: &str| word(w, n) as u16 as i16 as i32;
        let pair = |n: &str| {
            u128::from(word(w, &format!("{n}[0]"))) | u128::from(word(w, &format!("{n}[1]"))) << 32
        };
        Self {
            weapon: word(w, "weapon") as u8,
            offhand: word(w, "offHandIndex") as u8,
            weapon_time: short("weaponTime"),
            weapon_delay: short("weaponDelay"),
            state: word(w, "weaponstate") as i32,
            weap_anim: word(w, "weapAnim") as i32,
            pos_frac: float(w, "fWeaponPosFrac"),
            hold_breath_scale: float(w, "holdBreathScale"),
            hold_breath_timer: word(w, "holdBreathTimer") as i32,
            grenade_time_left: short("grenadeTimeLeft"),
            held: pair("weapons"),
            rechamber: pair("weaponrechamber"),
            ammo: std::array::from_fn(|k| w.ammo.get(k).copied().unwrap_or(0)),
            clip: std::array::from_fn(|k| w.ammoclip.get(k).copied().unwrap_or(0)),
            ..Default::default()
        }
    }

    /// [`Self::from_wire`]'s fields written over `w`.
    pub fn write_wire(&self, w: &mut PlayerState2) {
        set(w, "weapon", u32::from(self.weapon));
        set(w, "weaponstate", self.state as u32);
        set(w, "weaponTime", self.weapon_time as u32);
        set(w, "weaponDelay", self.weapon_delay as u32);
        set(w, "weapAnim", self.weap_anim as u32);
        set(w, "fWeaponPosFrac", self.pos_frac.to_bits());
        set(w, "holdBreathScale", self.hold_breath_scale.to_bits());
        set(w, "holdBreathTimer", self.hold_breath_timer as u32);
        set(w, "offHandIndex", u32::from(self.offhand));
        set(w, "grenadeTimeLeft", self.grenade_time_left as u32);
        set(w, "weapons[0]", self.held as u32);
        set(w, "weapons[1]", (self.held >> 32) as u32);
        set(w, "weaponrechamber[0]", self.rechamber as u32);
        set(w, "weaponrechamber[1]", (self.rechamber >> 32) as u32);
        w.ammo.copy_from_slice(&self.ammo[..64]);
        w.ammoclip.copy_from_slice(&self.clip[..64]);
    }
}
