//! The MP `EV_*` ids, one table for the server that raises them, the pmove
//! that predicts them and the client that plays them. Read out of
//! `cgame_mp_x86.dll` `eventnames[]` (VA 0x30077040), identical in every MP
//! module and in 1.5. The SP module's table diverges from 173 up.

pub const EV_NONE: i32 = 0;

/// ids 1-138 are six groups of 23, one entry per surface type (doc section 1).
pub const EV_FOOTSTEP_RUN_BASE: i32 = 1;
pub const EV_FOOTSTEP_WALK_BASE: i32 = 24;
pub const EV_FOOTSTEP_PRONE_BASE: i32 = 47;
pub const EV_JUMP_BASE: i32 = 70;
pub const EV_LANDING_BASE: i32 = 93;
pub const EV_LANDING_PAIN_BASE: i32 = 116;
pub const SURFACE_GROUP_SIZE: i32 = 23;

pub const EV_FOLIAGE_SOUND: i32 = 139;
pub const EV_STANCE_FORCE_STAND: i32 = 140;
pub const EV_STANCE_FORCE_CROUCH: i32 = 141;
pub const EV_STANCE_FORCE_PRONE: i32 = 142;
pub const EV_STEP_VIEW: i32 = 143;
pub const EV_WATER_TOUCH: i32 = 144;
pub const EV_WATER_LEAVE: i32 = 145;
pub const EV_ITEM_PICKUP: i32 = 146;
pub const EV_ITEM_PICKUP_QUIET: i32 = 147;
pub const EV_AMMO_PICKUP: i32 = 148;
pub const EV_NOAMMO: i32 = 149;
pub const EV_EMPTYCLIP: i32 = 150;
pub const EV_RELOAD: i32 = 151;
pub const EV_RELOAD_FROM_EMPTY: i32 = 152;
pub const EV_RELOAD_START: i32 = 153;
pub const EV_RELOAD_END: i32 = 154;
pub const EV_RAISE_WEAPON: i32 = 155;
pub const EV_PUTAWAY_WEAPON: i32 = 156;
pub const EV_WEAPON_ALT: i32 = 157;
pub const EV_PULLBACK_WEAPON: i32 = 158;
pub const EV_FIRE_WEAPON: i32 = 159;
pub const EV_FIRE_WEAPONB: i32 = 160;
pub const EV_FIRE_WEAPON_LASTSHOT: i32 = 161;
pub const EV_RECHAMBER_WEAPON: i32 = 162;
pub const EV_EJECT_BRASS: i32 = 163;
pub const EV_MELEE_SWIPE: i32 = 164;
pub const EV_FIRE_MELEE: i32 = 165;
pub const EV_MELEE_HIT: i32 = 166;
pub const EV_MELEE_MISS: i32 = 167;
pub const EV_FIRE_WEAPON_MG42: i32 = 168;
pub const EV_FIRE_QUADBARREL_1: i32 = 169;
pub const EV_FIRE_QUADBARREL_2: i32 = 170;
pub const EV_BULLET_TRACER: i32 = 171;
pub const EV_SOUND_ALIAS: i32 = 172;
pub const EV_BULLET_HIT_SMALL: i32 = 173;
pub const EV_BULLET_HIT_LARGE: i32 = 174;
/// Delivered to the victim and to whoever follows it (doc section 2).
pub const EV_BULLET_HIT_CLIENT_SMALL: i32 = 175;
pub const EV_BULLET_HIT_CLIENT_LARGE: i32 = 176;
pub const EV_GRENADE_BOUNCE: i32 = 177;
pub const EV_GRENADE_EXPLODE: i32 = 178;
pub const EV_ROCKET_EXPLODE: i32 = 179;
pub const EV_ROCKET_EXPLODE_NOMARKS: i32 = 180;
pub const EV_MOLOTOV_EXPLODE: i32 = 181;
pub const EV_MOLOTOV_EXPLODE_NOMARKS: i32 = 182;
pub const EV_CUSTOM_EXPLODE: i32 = 183;
pub const EV_CUSTOM_EXPLODE_NOMARKS: i32 = 184;
pub const EV_RAILTRAIL: i32 = 185;
pub const EV_BULLET: i32 = 186;
pub const EV_PAIN: i32 = 187;
pub const EV_CROUCH_PAIN: i32 = 188;
pub const EV_DEATH: i32 = 189;
pub const EV_DEBUG_LINE: i32 = 190;
pub const EV_PLAY_FX: i32 = 191;
pub const EV_PLAY_FX_DIR: i32 = 192;
pub const EV_PLAY_FX_ON_TAG: i32 = 193;
pub const EV_FLAMEBARREL_BOUNCE: i32 = 194;
pub const EV_EARTHQUAKE: i32 = 195;
pub const EV_DROPWEAPON: i32 = 196;
pub const EV_ITEM_RESPAWN: i32 = 197;
pub const EV_ITEM_POP: i32 = 198;
pub const EV_PLAYER_TELEPORT_IN: i32 = 199;
pub const EV_PLAYER_TELEPORT_OUT: i32 = 200;
pub const EV_OBITUARY: i32 = 201;

/// Marks an event of another protocol with no 1.1 counterpart: the client
/// sees `EV_FOREIGN | id`, the other protocol's own id below the flag.
pub const EV_FOREIGN: i32 = 0x1000;

// United Offensive's own events (`uo_cgame_mp_x86.dll` `eventnames[]`;
// docs/research/coduo-diff.md section 8), and the three whose meaning
// changed: `EV_BULLET_HIT` picks its effects by the weapon's `ammoType` and
// `EV_PROJECTILE_EXPLODE(_NOMARKS)` by the explosion type in `es.scale`.
pub const EV_UO_FATIGUE_LAST_SOUND: i32 = EV_FOREIGN | 140;
pub const EV_UO_FATIGUE_SOUND: i32 = EV_FOREIGN | 141;
pub const EV_UO_DEPLOY_WEAPON: i32 = EV_FOREIGN | 160;
pub const EV_UO_BREAKDOWN_WEAPON: i32 = EV_FOREIGN | 161;
pub const EV_UO_FIRE_WEAPONC: i32 = EV_FOREIGN | 165;
pub const EV_UO_BULLET_HIT: i32 = EV_FOREIGN | 178;
pub const EV_UO_GRENADE_SPOON: i32 = EV_FOREIGN | 182;
pub const EV_UO_PROJECTILE_LAUNCH: i32 = EV_FOREIGN | 183;
pub const EV_UO_PROJECTILE_INCOMING: i32 = EV_FOREIGN | 184;
pub const EV_UO_PROJECTILE_EXPLODE: i32 = EV_FOREIGN | 185;
pub const EV_UO_PROJECTILE_EXPLODE_NOMARKS: i32 = EV_FOREIGN | 186;
pub const EV_UO_PLAY_FX_ON_PLAYER: i32 = EV_FOREIGN | 198;
pub const EV_UO_STOP_ATTACHED_FX: i32 = EV_FOREIGN | 199;
pub const EV_UO_REMOVE_WEAPON_ATTACHMENTS: i32 = EV_FOREIGN | 202;
pub const EV_UO_OVERHEATING: i32 = EV_FOREIGN | 208;
pub const EV_UO_GRENADE_SUICIDE: i32 = EV_FOREIGN | 210;

/// United Offensive's `EV_*` ids (`uo_cgame_mp_x86.dll` `eventnames[]`, VA
/// 0x30082428, 211 entries) as vcod's: 0..139 are 1.1's, the rest moved
/// around eleven new events and two merges (docs/research/coduo-diff.md,
/// section 8). The new ones, `EV_BULLET_HIT` (178) and the projectile
/// explodes (185, 186) come out as `EV_FOREIGN | id` (the `EV_UO_*` names).
pub fn event_v22_to_v1(id: i32) -> i32 {
    match id {
        0..=139 => id,
        142..=159 => id - 2,
        162..=164 => id - 4,
        166..=177 => id - 5,
        179..=181 => id - 4,
        187 | 188 => id - 6,
        189..=197 => id - 4,
        200 | 201 => id - 6,
        203..=207 => id - 7,
        209 => EV_OBITUARY,
        _ => EV_FOREIGN | id,
    }
}

/// The inverse of [`event_v22_to_v1`], for a protocol 22 server: vcod's
/// (1.1's) id in United Offensive's numbering. 1.1's two bullet hits are
/// UO's one `EV_BULLET_HIT`, and its grenade, rocket and custom explodes
/// UO's `EV_PROJECTILE_EXPLODE(_NOMARKS)`; an `EV_FOREIGN` id is UO's own.
pub fn event_v1_to_v22(id: i32) -> i32 {
    if id & EV_FOREIGN != 0 {
        return id & !EV_FOREIGN;
    }
    match id {
        0..=139 => id,
        140..=157 => id + 2,
        158..=160 => id + 4,
        161..=172 => id + 5,
        EV_BULLET_HIT_SMALL | EV_BULLET_HIT_LARGE => 178,
        175..=177 => id + 4,
        EV_GRENADE_EXPLODE | EV_ROCKET_EXPLODE | EV_CUSTOM_EXPLODE => 185,
        EV_ROCKET_EXPLODE_NOMARKS | EV_CUSTOM_EXPLODE_NOMARKS => 186,
        181 | 182 => id + 6,
        185..=193 => id + 4,
        194 | 195 => id + 6,
        196..=200 => id + 7,
        EV_OBITUARY => 209,
        _ => id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_events_go_back_to_their_uo_ids() {
        for uo in 0..=210 {
            assert_eq!(event_v1_to_v22(event_v22_to_v1(uo)), uo, "UO event {uo}");
        }
        assert_eq!(event_v1_to_v22(EV_BULLET_HIT_LARGE), 178);
        assert_eq!(event_v1_to_v22(EV_ROCKET_EXPLODE_NOMARKS), 186);
    }

    #[test]
    fn v22_events_by_name() {
        let map = event_v22_to_v1;
        // Pairs read off the two `eventnames[]` tables.
        for (uo, v1) in [
            (139, EV_FOLIAGE_SOUND),
            (142, EV_STANCE_FORCE_STAND),
            (159, EV_WEAPON_ALT),
            (162, EV_PULLBACK_WEAPON),
            (163, EV_FIRE_WEAPON),
            (164, EV_FIRE_WEAPONB),
            (166, EV_FIRE_WEAPON_LASTSHOT),
            (177, EV_SOUND_ALIAS),
            (179, EV_BULLET_HIT_CLIENT_SMALL),
            (181, EV_GRENADE_BOUNCE),
            (187, EV_MOLOTOV_EXPLODE),
            (189, EV_RAILTRAIL),
            (193, EV_DEATH),
            (195, EV_PLAY_FX),
            (197, EV_PLAY_FX_ON_TAG),
            (200, EV_FLAMEBARREL_BOUNCE),
            (201, EV_EARTHQUAKE),
            (203, EV_DROPWEAPON),
            (207, EV_PLAYER_TELEPORT_OUT),
            (209, EV_OBITUARY),
        ] {
            assert_eq!(map(uo), v1, "UO event {uo}");
        }
        // Fatigue, deploy, the gunner's fire, the bullet hit, the spoon,
        // artillery, the explodes, the attached fx pair, attachments,
        // overheating, grenade suicide.
        for uo in [
            140, 141, 160, 161, 165, 178, 182, 183, 184, 185, 186, 198, 199, 202, 208, 210,
        ] {
            assert_eq!(map(uo), EV_FOREIGN | uo);
        }
    }
}
