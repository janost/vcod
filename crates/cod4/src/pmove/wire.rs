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

/// `r.maxs[2]` as `PM_CheckDuck` leaves it: 30 prone, 50 crouched, 70
/// standing.
pub fn body_height(pm_flags: u32) -> f32 {
    if pm_flags & super::pmf::PRONE != 0 {
        30.0
    } else if pm_flags & super::pmf::DUCKED != 0 {
        50.0
    } else {
        70.0
    }
}

/// The entity's `fTorsoPitch` and `fWaistPitch`: 0 unless the eye's target
/// is prone (`PM_GetEffectiveStance` 1, `cod4_lnxded` 0x8055ffe), else the
/// playerstate's wrapped to -180..180 (`(x - floor(x + 0.5)) * 360` of the
/// turns, the floor stored as a float) times the eye lerp's fraction toward
/// prone (`1 - f` on a lerp up). 0x8052cc1..0x8052d5f.
fn prone_pitches(ps: &delta::PlayerState) -> (f32, f32) {
    if int(ps, ix!("viewHeightTarget")) != 11 {
        return (0.0, 0.0);
    }
    let lerp_time = int(ps, ix!("viewHeightLerpTime"));
    let frac = if lerp_time == 0 {
        1.0
    } else {
        let down = word(ps, ix!("viewHeightLerpDown")) != 0;
        let dur = super::view_height_lerp_duration(int(ps, ix!("viewHeightLerpTarget")), down);
        let f =
            (f64::from(int(ps, ix!("commandTime")) - lerp_time) / f64::from(dur)).clamp(0.0, 1.0);
        if down { f } else { 1.0 - f }
    };
    let wrap = |i: usize| {
        let t = f64::from(float(ps, i)) * f64::from(1.0f32 / 360.0);
        let floor = (t + 0.5).floor() as f32;
        ((t - f64::from(floor)) * 360.0 * frac) as f32
    };
    (wrap(ix!("fTorsoPitch")), wrap(ix!("fWaistPitch")))
}

/// `BG_PlayerStateToEntityState` (1.7's `cod4_lnxded` 0x8052940, inlined):
/// the snapped origin and view angles (the capture's teammate at z 236 off
/// its playerstate's 236.125), the lean, a prone body's torso and waist
/// pitches, the weapon and its camo, the stance flags, the ground, and
/// the box packed as `SV_LinkEntity` does (`x | (1 - mins.z) << 8 |
/// (maxs.z + 32) << 16`: 0x66010f for the 30 x 70 body, VERIFIED live);
/// `pos` reads `trType` 3 and `apos` 1 on the capture's still teammate
/// (VERIFIED live). The angles' and pitches' sources are
/// cod4-player-anims.md 8. The server sends it for every other client;
/// the client builds its own body from it (`cg.predictedPlayerEntity`,
/// cod4-client-view.md 16).
pub fn player_entity(slot: usize, ps: &delta::PlayerState) -> delta::EntityState {
    let mut e = delta::EntityState::default();
    e.0[0] = slot as u32;
    e.0[1] = 1;
    let mut put = |name: &str, v: u32| {
        if let Some(f) = delta::EntityState::table(1).iter().find(|f| f.name == name) {
            e.0[f.word()] = v;
        }
    };
    let origin = ix!(3 "origin");
    let angles = ix!(3 "viewangles");
    for i in 0..3 {
        put(
            &format!("lerp.pos.trBase[{i}]"),
            float(ps, origin[i]).trunc().to_bits(),
        );
        put(
            &format!("lerp.apos.trBase[{i}]"),
            float(ps, angles[i]).trunc().to_bits(),
        );
    }
    put("lerp.u.player.leanf", word(ps, ix!("leanf")));
    let (torso, waist) = prone_pitches(ps);
    put("fTorsoPitch", torso.to_bits());
    put("fWaistPitch", waist.to_bits());
    put("lerp.u.player.movementDir", word(ps, ix!("movementDir")));
    put("lerp.pos.trType", 3);
    put("lerp.apos.trType", 1);
    // 0x20000 for a dead body (`pm_type` past 6), 0x40000 for one down
    // its sights (`PMF_SIGHT` 0x10), each cleared otherwise
    // (0x8052a54..0x8052a82, 0x8052ca2, 0x8052cb1).
    let own = word(ps, ix!("eFlags"));
    let mut e_flags = own & !0x6_0000;
    if word(ps, ix!("pm_type")) > 6 {
        e_flags |= 0x2_0000;
    }
    if word(ps, ix!("pm_flags")) & 0x10 != 0 {
        e_flags |= 0x4_0000;
    }
    put("lerp.eFlags", e_flags);
    let held = word(ps, ix!("weapon"));
    put("weapon", held);
    // The held weapon's camo, `weaponmodels[weapon]` (0x8052af5..0x8052b03).
    put(
        "weaponModel",
        ps.weapon_models
            .get(held as usize)
            .map_or(0, |&m| u32::from(m)),
    );
    put("legsAnim", word(ps, ix!("legsAnim")));
    put("torsoAnim", word(ps, ix!("torsoAnim")));
    put("groundEntityNum", word(ps, ix!("groundEntityNum")));
    // A gunner names his gun: `eFlags & 0x300` copies `viewlocked_entNum`
    // (ps+0x59c) into `otherEntityNum` (0x8052ab9; cod4-gsc.md 9.2).
    if own & 0x300 != 0 {
        put("otherEntityNum", word(ps, ix!("viewlocked_entNum")));
    }
    put("clientNum", slot as u32);
    let h = body_height(word(ps, ix!("pm_flags"))) as u32;
    put("solid", 15 | (1 << 8) | ((h + 32) << 16));
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_named(ps: &mut delta::PlayerState, name: &str, v: u32) {
        let f = PLAYER_STATE.iter().find(|f| f.name == name).unwrap();
        ps.words[f.word()] = v;
    }

    fn ent_f32(e: &delta::EntityState, name: &str) -> f32 {
        let f = delta::EntityState::table(1)
            .iter()
            .find(|f| f.name == name)
            .unwrap();
        f32::from_bits(e.0[f.word()])
    }

    /// Others see a body's aim and lean: `apos` is the view `(int)`, which
    /// the -100 field sends a half short up (the kill capture's aiming
    /// shooter reads pitch 1.99951 and yaw -68.99414, cod4-player-anims.md
    /// 8), `leanf` rides along, and the torso and waist pitches only
    /// prone, eased by the eye's lerp.
    #[test]
    fn a_body_carries_its_aim_its_lean_and_its_prone_pitches() {
        let f = |v: f32| v.to_bits();
        let mut ps = delta::PlayerState::default();
        set_named(&mut ps, "viewangles[0]", f(2.7));
        set_named(&mut ps, "viewangles[1]", f(-69.6));
        set_named(&mut ps, "leanf", f(-0.25));
        set_named(&mut ps, "fTorsoPitch", f(10.0));
        set_named(&mut ps, "fWaistPitch", f(350.0));
        set_named(&mut ps, "viewHeightTarget", 60);
        let e = player_entity(3, &ps);
        assert_eq!(ent_f32(&e, "lerp.apos.trBase[0]"), 2.0);
        assert_eq!(ent_f32(&e, "lerp.apos.trBase[1]"), -69.0);
        let sent = e.sent();
        assert_eq!(ent_f32(&sent, "lerp.apos.trBase[0]"), 1.9995117);
        assert_eq!(ent_f32(&sent, "lerp.apos.trBase[1]"), -68.99414);
        assert_eq!(ent_f32(&e, "lerp.u.player.leanf"), -0.25);
        assert_eq!(ent_f32(&e, "fTorsoPitch"), 0.0);
        set_named(&mut ps, "viewHeightTarget", 11);
        let e = player_entity(3, &ps);
        assert_eq!(ent_f32(&e, "fTorsoPitch"), 10.0);
        assert!((ent_f32(&e, "fWaistPitch") + 10.0).abs() < 1e-3);
        // Halfway down into prone: half the pitch.
        set_named(&mut ps, "commandTime", 1200);
        set_named(&mut ps, "viewHeightLerpTime", 1000);
        set_named(&mut ps, "viewHeightLerpTarget", 11);
        set_named(&mut ps, "viewHeightLerpDown", 1);
        let e = player_entity(3, &ps);
        assert_eq!(ent_f32(&e, "fTorsoPitch"), 5.0);
    }

    /// Others see the held weapon's camo: `weaponModel` is
    /// `weaponmodels[weapon]` (cod4-client-view.md 4.7), and it rides the
    /// wire's 4 bits.
    #[test]
    fn a_body_carries_its_weapons_camo() {
        let mut ps = delta::PlayerState::default();
        set_named(&mut ps, "weapon", 2);
        ps.weapon_models[1] = 6;
        ps.weapon_models[2] = 3;
        let model = |e: &delta::EntityState| ent_f32(e, "weaponModel").to_bits();
        let e = player_entity(3, &ps);
        assert_eq!(model(&e), 3);
        assert_eq!(model(&e.sent()), 3);
        set_named(&mut ps, "weapon", 1);
        assert_eq!(model(&player_entity(3, &ps)), 6);
        set_named(&mut ps, "weapon", 0);
        assert_eq!(model(&player_entity(3, &ps)), 0);
    }
}
