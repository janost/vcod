//! pmove's playerstate to and from the wire's netfield words
//! (`net::fields::PLAYER_STATE_FIELDS`), for a replay of a retail capture
//! and for prediction off a snapshot. `oldVelocity` and the anim
//! conditions are not on the wire.

use glam::Vec3;

use super::PlayerState;
use crate::net::PlayerState2;
use crate::net::fields::PLAYER_STATE_FIELDS;

/// A playerstate field's index (or `N` indices), resolved at compile time.
macro_rules! ix {
    ($($t:tt)*) => {
        crate::field!(PLAYER_STATE_FIELDS, $($t)*)
    };
}

fn word(ps: &PlayerState2, i: usize) -> u32 {
    ps.f[i]
}

/// A NaN or infinite float is no retail state: 0.
fn float(ps: &PlayerState2, i: usize) -> f32 {
    vcod_common::fguard::finite(f32::from_bits(word(ps, i)))
}

fn vec3(ps: &PlayerState2, ix: [usize; 3]) -> Vec3 {
    Vec3::new(float(ps, ix[0]), float(ps, ix[1]), float(ps, ix[2]))
}

fn set(ps: &mut PlayerState2, i: usize, v: u32) {
    ps.f[i] = v;
}

fn set_vec3(ps: &mut PlayerState2, ix: [usize; 3], v: Vec3) {
    for i in 0..3 {
        set(ps, ix[i], v[i].to_bits());
    }
}

impl PlayerState {
    /// The fields pmove reads, off a snapshot's playerstate; `old_velocity`
    /// is left 0, a still body's.
    pub fn from_wire(w: &PlayerState2) -> PlayerState {
        let i = |n: usize| word(w, n) as i32;
        let arr4 = |n: [usize; 4]| n.map(i);
        PlayerState {
            command_time: i(ix!("commandTime")),
            pm_type: i(ix!("pm_type")),
            bob_cycle: i(ix!("bobCycle")),
            pm_flags: word(w, ix!("pm_flags")),
            pm_time: i(ix!("pm_time")),
            origin: vec3(w, ix!(3 "origin")),
            velocity: vec3(w, ix!(3 "velocity")),
            old_velocity: [0.0; 2],
            gravity: i(ix!("gravity")),
            leanf: float(w, ix!("leanf")),
            speed: i(ix!("speed")),
            delta_angles: ix!(3 "delta_angles").map(i),
            ground_entity_num: i(ix!("groundEntityNum")),
            ladder_vec: vec3(w, ix!(3 "vLadderVec")),
            jump_time: i(ix!("jumpTime")),
            jump_origin_z: float(w, ix!("jumpOriginZ")),
            movement_dir: i(ix!("movementDir")),
            e_flags: word(w, ix!("eFlags")),
            event_sequence: i(ix!("eventSequence")),
            events: arr4(ix!(4 "events")),
            event_parms: arr4(ix!(4 "eventParms")),
            client_num: word(w, ix!("clientNum")),
            weapon_state: i(ix!("weaponstate")),
            view_angles: vec3(w, ix!(3 "viewangles")).to_array(),
            view_height_target: i(ix!("viewHeightTarget")),
            view_height_current: float(w, ix!("viewHeightCurrent")),
            view_height_lerp_time: i(ix!("viewHeightLerpTime")),
            view_height_lerp_target: i(ix!("viewHeightLerpTarget")),
            view_height_lerp_down: i(ix!("viewHeightLerpDown")) != 0,
            mins: vec3(w, ix!(3 "mins")),
            maxs: vec3(w, ix!(3 "maxs")),
            prone_direction: float(w, ix!("proneDirection")),
            prone_direction_pitch: float(w, ix!("proneDirectionPitch")),
            prone_torso_pitch: float(w, ix!("proneTorsoPitch")),
            prone_valid: [
                float(w, ix!("fTorsoHeight")),
                float(w, ix!("fTorsoPitch")),
                float(w, ix!("fWaistPitch")),
            ],
            aim_spread_scale: float(w, ix!("aimSpreadScale")),
            damage_timer: i(ix!("damageTimer")),
            view_angle_clamp_base: [
                float(w, ix!("viewAngleClampBase[0]")),
                float(w, ix!("viewAngleClampBase[1]")),
            ],
            view_angle_clamp_range: [
                float(w, ix!("viewAngleClampRange[0]")),
                float(w, ix!("viewAngleClampRange[1]")),
            ],
            damage_duration: i(ix!("damageDuration")),
            // 16 bits on the wire, a signed yaw in memory.
            flinch_yaw: i(ix!("flinchYaw")) as u16 as i16 as i32,
            mantle: super::mantle::MantleState {
                yaw: float(w, ix!("mantleState.yaw")),
                timer: i(ix!("mantleState.timer")),
                trans_index: i(ix!("mantleState.transIndex")),
                flags: word(w, ix!("mantleState.flags")),
            },
            legs_timer: i(ix!("legsTimer")),
            legs_anim: i(ix!("legsAnim")),
            torso_timer: i(ix!("torsoTimer")),
            torso_anim: i(ix!("torsoAnim")),
            anim_movetype: 0,
            anim_strafing: 0,
            foliage_sound_time: i(ix!("foliageSoundTime")),
            weapon_interrupt: false,
        }
    }

    /// `base` with the fields [`PlayerState::from_wire`] reads written from
    /// `self`, as full words (an angle is not quantized here).
    pub fn to_wire(&self, base: &PlayerState2) -> PlayerState2 {
        let mut w = base.clone();
        let s = &mut w;
        set(s, ix!("commandTime"), self.command_time as u32);
        set(s, ix!("pm_type"), self.pm_type as u32);
        set(s, ix!("bobCycle"), self.bob_cycle as u32);
        set(s, ix!("pm_flags"), self.pm_flags);
        set(s, ix!("pm_time"), self.pm_time as u32);
        set_vec3(s, ix!(3 "origin"), self.origin);
        set_vec3(s, ix!(3 "velocity"), self.velocity);
        set(s, ix!("gravity"), self.gravity as u32);
        set(s, ix!("leanf"), self.leanf.to_bits());
        set(s, ix!("speed"), self.speed as u32);
        for k in 0..3 {
            set(s, ix!(3 "delta_angles")[k], self.delta_angles[k] as u32);
        }
        set(s, ix!("groundEntityNum"), self.ground_entity_num as u32);
        set_vec3(s, ix!(3 "vLadderVec"), self.ladder_vec);
        set(s, ix!("jumpTime"), self.jump_time as u32);
        set(s, ix!("jumpOriginZ"), self.jump_origin_z.to_bits());
        set(s, ix!("movementDir"), self.movement_dir as u32);
        set(s, ix!("eFlags"), self.e_flags);
        set(s, ix!("eventSequence"), self.event_sequence as u32);
        for k in 0..4 {
            set(s, ix!(4 "events")[k], self.events[k] as u32);
            set(s, ix!(4 "eventParms")[k], self.event_parms[k] as u32);
        }
        set(s, ix!("clientNum"), self.client_num);
        set(s, ix!("weaponstate"), self.weapon_state as u32);
        set_vec3(s, ix!(3 "viewangles"), Vec3::from_array(self.view_angles));
        set(s, ix!("viewHeightTarget"), self.view_height_target as u32);
        set(
            s,
            ix!("viewHeightCurrent"),
            self.view_height_current.to_bits(),
        );
        set(
            s,
            ix!("viewHeightLerpTime"),
            self.view_height_lerp_time as u32,
        );
        set(
            s,
            ix!("viewHeightLerpTarget"),
            self.view_height_lerp_target as u32,
        );
        set(
            s,
            ix!("viewHeightLerpDown"),
            u32::from(self.view_height_lerp_down),
        );
        set_vec3(s, ix!(3 "mins"), self.mins);
        set_vec3(s, ix!(3 "maxs"), self.maxs);
        set(s, ix!("proneDirection"), self.prone_direction.to_bits());
        set(
            s,
            ix!("proneDirectionPitch"),
            self.prone_direction_pitch.to_bits(),
        );
        set(s, ix!("proneTorsoPitch"), self.prone_torso_pitch.to_bits());
        set(s, ix!("fTorsoHeight"), self.prone_valid[0].to_bits());
        set(s, ix!("fTorsoPitch"), self.prone_valid[1].to_bits());
        set(s, ix!("fWaistPitch"), self.prone_valid[2].to_bits());
        set(s, ix!("aimSpreadScale"), self.aim_spread_scale.to_bits());
        set(s, ix!("damageTimer"), self.damage_timer as u32);
        for k in 0..2 {
            let (b, r) = (
                self.view_angle_clamp_base[k],
                self.view_angle_clamp_range[k],
            );
            set(s, ix!(2 "viewAngleClampBase")[k], b.to_bits());
            set(s, ix!(2 "viewAngleClampRange")[k], r.to_bits());
        }
        set(s, ix!("damageDuration"), self.damage_duration as u32);
        set(s, ix!("flinchYaw"), self.flinch_yaw as u32);
        set(s, ix!("mantleState.yaw"), self.mantle.yaw.to_bits());
        set(s, ix!("mantleState.timer"), self.mantle.timer as u32);
        set(
            s,
            ix!("mantleState.transIndex"),
            self.mantle.trans_index as u32,
        );
        set(s, ix!("mantleState.flags"), self.mantle.flags);
        set(s, ix!("legsTimer"), self.legs_timer as u32);
        set(s, ix!("legsAnim"), self.legs_anim as u32);
        set(s, ix!("torsoTimer"), self.torso_timer as u32);
        set(s, ix!("torsoAnim"), self.torso_anim as u32);
        set(s, ix!("foliageSoundTime"), self.foliage_sound_time as u32);
        w
    }
}

impl super::weapon::WeaponState {
    /// `PM_Weapon`'s half of a snapshot's playerstate. `restrictKickTime`
    /// and `adsDelayTime` are left 0, as the A/B's resync keeps its own.
    pub fn from_wire(w: &PlayerState2) -> Self {
        // The 16-bit timers come back sign-extended.
        let short = |n: usize| word(w, n) as u16 as i16 as i32;
        let pair = |n: [usize; 2]| u128::from(word(w, n[0])) | u128::from(word(w, n[1])) << 32;
        Self {
            weapon: word(w, ix!("weapon")) as u8,
            offhand: word(w, ix!("offHandIndex")) as u8,
            weapon_time: short(ix!("weaponTime")),
            weapon_delay: short(ix!("weaponDelay")),
            state: word(w, ix!("weaponstate")) as i32,
            weap_anim: word(w, ix!("weapAnim")) as i32,
            pos_frac: float(w, ix!("fWeaponPosFrac")).clamp(0.0, 1.0),
            hold_breath_scale: float(w, ix!("holdBreathScale")),
            hold_breath_timer: word(w, ix!("holdBreathTimer")) as i32,
            grenade_time_left: short(ix!("grenadeTimeLeft")),
            held: pair(ix!(2 "weapons")),
            rechamber: pair(ix!(2 "weaponrechamber")),
            ammo: std::array::from_fn(|k| w.ammo.get(k).copied().unwrap_or(0)),
            clip: std::array::from_fn(|k| w.ammoclip.get(k).copied().unwrap_or(0)),
            ..Default::default()
        }
    }

    /// [`Self::from_wire`]'s fields written over `w`.
    pub fn write_wire(&self, w: &mut PlayerState2) {
        set(w, ix!("weapon"), u32::from(self.weapon));
        set(w, ix!("weaponstate"), self.state as u32);
        set(w, ix!("weaponTime"), self.weapon_time as u32);
        set(w, ix!("weaponDelay"), self.weapon_delay as u32);
        set(w, ix!("weapAnim"), self.weap_anim as u32);
        set(w, ix!("fWeaponPosFrac"), self.pos_frac.to_bits());
        set(w, ix!("holdBreathScale"), self.hold_breath_scale.to_bits());
        set(w, ix!("holdBreathTimer"), self.hold_breath_timer as u32);
        set(w, ix!("offHandIndex"), u32::from(self.offhand));
        set(w, ix!("grenadeTimeLeft"), self.grenade_time_left as u32);
        set(w, ix!("weapons[0]"), self.held as u32);
        set(w, ix!("weapons[1]"), (self.held >> 32) as u32);
        set(w, ix!("weaponrechamber[0]"), self.rechamber as u32);
        set(w, ix!("weaponrechamber[1]"), (self.rechamber >> 32) as u32);
        w.ammo.copy_from_slice(&self.ammo[..64]);
        w.ammoclip.copy_from_slice(&self.clip[..64]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pmove::weapon::WeaponState;

    /// A wire playerstate with every word distinct, a finite float, and
    /// every ammo slot its own count.
    fn distinct() -> PlayerState2 {
        let mut w = PlayerState2::default();
        for (i, word) in w.f.iter_mut().enumerate() {
            *word = (0.5 + i as f32).to_bits();
        }
        for (i, a) in w.ammo.iter_mut().chain(w.ammoclip.iter_mut()).enumerate() {
            *a = i as i32 + 1;
        }
        w
    }

    /// Every field `from_wire` reads comes back through `to_wire`: a field
    /// added to one side alone, or written to another word, fails here.
    #[test]
    fn to_wire_writes_back_what_from_wire_reads() {
        let w = distinct();
        let want = PlayerState::from_wire(&w);
        let back = want.to_wire(&PlayerState2::default());
        assert_eq!(PlayerState::from_wire(&back), want);
    }

    /// The same for the weapon's half and `write_wire`.
    #[test]
    fn write_wire_writes_back_what_the_weapons_from_wire_reads() {
        let w = distinct();
        let want = WeaponState::from_wire(&w);
        let mut back = PlayerState2::default();
        want.write_wire(&mut back);
        assert_eq!(WeaponState::from_wire(&back), want);
    }
}
