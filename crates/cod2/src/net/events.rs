//! CoD2 1.3's entity types and `EV_*` events, in CoD2's own numbering
//! (vcod maps nothing onto 1.1's). VERIFIED for every name and number: the
//! event names are the 199 pointers of the table at `.data` 0x8184da0 in
//! `cod2_lnxded` (strings from rodata 0x8153940), dumped in order; the
//! `eType`s are the arms of `CG_ProcessEntity` (Mac 0x164bd4) and
//! `G_PrintEntities`' names (docs/research/cod2-protocol.md 13).
//!
//! One exception: the table names 181 `EV_BULLET_HIT_SMALL`, 182
//! `EV_BULLET_HIT_LARGE` and 183 `EV_SHOTGUN_HIT`, but `CG_BulletHitEvent`
//! (Mac 0x1db1f4) plays the `bullet_small` sound and impact rows 0/1 for
//! 182, `bullet_large` and rows 2/3 for 183, and `bulletspray_small` and
//! the shotgun rows 4/5 for 181 (VERIFIED; the rows are `g_TypeName`'s,
//! the aliases `CG_RegisterSounds`' `cgs + 0x27f24/0x27fdc/0x27f80`). vcod
//! names the three by that reading (docs/research/cod2-efx.md 6.2).

/// `entityType_t`. 0..6 are named by `G_PrintEntities`' strings; 7 is
/// `CG_SoundBlend`'s arm, 8 the arm that plays `fx` configstring
/// `es.index`-style effects on a timer, 9 `G_SpawnTurret`'s (Mac 0x1bc84a
/// stores 9). The names of 7..9 are vcod's.
pub const ET_GENERAL: i32 = 0;
pub const ET_PLAYER: i32 = 1;
pub const ET_CORPSE: i32 = 2;
pub const ET_ITEM: i32 = 3;
pub const ET_MISSILE: i32 = 4;
pub const ET_INVISIBLE: i32 = 5;
pub const ET_SCRIPTMOVER: i32 = 6;
pub const ET_SOUND_BLEND: i32 = 7;
pub const ET_FX: i32 = 8;
pub const ET_TURRET: i32 = 9;
/// An `eType` above this is a temp event entity carrying event
/// `eType - ET_EVENTS`: `G_TempEntity` (Mac 0x1b7052) stores `event + 10`,
/// `CG_CheckEvents` (Mac 0x1e614a) tests `10 < eType`.
pub const ET_EVENTS: i32 = 10;

/// The event a temp entity's `eType` carries, `None` for an ordinary one.
pub fn event_of(etype: i32) -> Option<i32> {
    (etype > ET_EVENTS).then_some(etype - ET_EVENTS)
}

/// The surface-type count each per-surface block holds (default, bark, ...
/// asphalt); the block's event for surface `s` is its base plus `s`.
pub const SURFACE_EVENTS: i32 = 23;
pub const EV_FOOTSTEP_RUN: i32 = 1;
pub const EV_FOOTSTEP_WALK: i32 = 24;
pub const EV_FOOTSTEP_PRONE: i32 = 47;
pub const EV_JUMP: i32 = 70;
pub const EV_LANDING: i32 = 93;
pub const EV_LANDING_PAIN: i32 = 116;

/// The event's name, `None` past the table.
pub fn event_name(id: i32) -> Option<&'static str> {
    usize::try_from(id)
        .ok()
        .and_then(|i| EV_NAMES.get(i).copied())
}

pub const EV_NAMES: [&str; 199] = [
    "EV_NONE",
    "EV_FOOTSTEP_RUN_DEFAULT",
    "EV_FOOTSTEP_RUN_BARK",
    "EV_FOOTSTEP_RUN_BRICK",
    "EV_FOOTSTEP_RUN_CARPET",
    "EV_FOOTSTEP_RUN_CLOTH",
    "EV_FOOTSTEP_RUN_CONCRETE",
    "EV_FOOTSTEP_RUN_DIRT",
    "EV_FOOTSTEP_RUN_FLESH",
    "EV_FOOTSTEP_RUN_FOLIAGE",
    "EV_FOOTSTEP_RUN_GLASS",
    "EV_FOOTSTEP_RUN_GRASS",
    "EV_FOOTSTEP_RUN_GRAVEL",
    "EV_FOOTSTEP_RUN_ICE",
    "EV_FOOTSTEP_RUN_METAL",
    "EV_FOOTSTEP_RUN_MUD",
    "EV_FOOTSTEP_RUN_PAPER",
    "EV_FOOTSTEP_RUN_PLASTER",
    "EV_FOOTSTEP_RUN_ROCK",
    "EV_FOOTSTEP_RUN_SAND",
    "EV_FOOTSTEP_RUN_SNOW",
    "EV_FOOTSTEP_RUN_WATER",
    "EV_FOOTSTEP_RUN_WOOD",
    "EV_FOOTSTEP_RUN_ASPHALT",
    "EV_FOOTSTEP_WALK_DEFAULT",
    "EV_FOOTSTEP_WALK_BARK",
    "EV_FOOTSTEP_WALK_BRICK",
    "EV_FOOTSTEP_WALK_CARPET",
    "EV_FOOTSTEP_WALK_CLOTH",
    "EV_FOOTSTEP_WALK_CONCRETE",
    "EV_FOOTSTEP_WALK_DIRT",
    "EV_FOOTSTEP_WALK_FLESH",
    "EV_FOOTSTEP_WALK_FOLIAGE",
    "EV_FOOTSTEP_WALK_GLASS",
    "EV_FOOTSTEP_WALK_GRASS",
    "EV_FOOTSTEP_WALK_GRAVEL",
    "EV_FOOTSTEP_WALK_ICE",
    "EV_FOOTSTEP_WALK_METAL",
    "EV_FOOTSTEP_WALK_MUD",
    "EV_FOOTSTEP_WALK_PAPER",
    "EV_FOOTSTEP_WALK_PLASTER",
    "EV_FOOTSTEP_WALK_ROCK",
    "EV_FOOTSTEP_WALK_SAND",
    "EV_FOOTSTEP_WALK_SNOW",
    "EV_FOOTSTEP_WALK_WATER",
    "EV_FOOTSTEP_WALK_WOOD",
    "EV_FOOTSTEP_WALK_ASPHALT",
    "EV_FOOTSTEP_PRONE_DEFAULT",
    "EV_FOOTSTEP_PRONE_BARK",
    "EV_FOOTSTEP_PRONE_BRICK",
    "EV_FOOTSTEP_PRONE_CARPET",
    "EV_FOOTSTEP_PRONE_CLOTH",
    "EV_FOOTSTEP_PRONE_CONCRETE",
    "EV_FOOTSTEP_PRONE_DIRT",
    "EV_FOOTSTEP_PRONE_FLESH",
    "EV_FOOTSTEP_PRONE_FOLIAGE",
    "EV_FOOTSTEP_PRONE_GLASS",
    "EV_FOOTSTEP_PRONE_GRASS",
    "EV_FOOTSTEP_PRONE_GRAVEL",
    "EV_FOOTSTEP_PRONE_ICE",
    "EV_FOOTSTEP_PRONE_METAL",
    "EV_FOOTSTEP_PRONE_MUD",
    "EV_FOOTSTEP_PRONE_PAPER",
    "EV_FOOTSTEP_PRONE_PLASTER",
    "EV_FOOTSTEP_PRONE_ROCK",
    "EV_FOOTSTEP_PRONE_SAND",
    "EV_FOOTSTEP_PRONE_SNOW",
    "EV_FOOTSTEP_PRONE_WATER",
    "EV_FOOTSTEP_PRONE_WOOD",
    "EV_FOOTSTEP_PRONE_ASPHALT",
    "EV_JUMP_DEFAULT",
    "EV_JUMP_BARK",
    "EV_JUMP_BRICK",
    "EV_JUMP_CARPET",
    "EV_JUMP_CLOTH",
    "EV_JUMP_CONCRETE",
    "EV_JUMP_DIRT",
    "EV_JUMP_FLESH",
    "EV_JUMP_FOLIAGE",
    "EV_JUMP_GLASS",
    "EV_JUMP_GRASS",
    "EV_JUMP_GRAVEL",
    "EV_JUMP_ICE",
    "EV_JUMP_METAL",
    "EV_JUMP_MUD",
    "EV_JUMP_PAPER",
    "EV_JUMP_PLASTER",
    "EV_JUMP_ROCK",
    "EV_JUMP_SAND",
    "EV_JUMP_SNOW",
    "EV_JUMP_WATER",
    "EV_JUMP_WOOD",
    "EV_JUMP_ASPHALT",
    "EV_LANDING_DEFAULT",
    "EV_LANDING_BARK",
    "EV_LANDING_BRICK",
    "EV_LANDING_CARPET",
    "EV_LANDING_CLOTH",
    "EV_LANDING_CONCRETE",
    "EV_LANDING_DIRT",
    "EV_LANDING_FLESH",
    "EV_LANDING_FOLIAGE",
    "EV_LANDING_GLASS",
    "EV_LANDING_GRASS",
    "EV_LANDING_GRAVEL",
    "EV_LANDING_ICE",
    "EV_LANDING_METAL",
    "EV_LANDING_MUD",
    "EV_LANDING_PAPER",
    "EV_LANDING_PLASTER",
    "EV_LANDING_ROCK",
    "EV_LANDING_SAND",
    "EV_LANDING_SNOW",
    "EV_LANDING_WATER",
    "EV_LANDING_WOOD",
    "EV_LANDING_ASPHALT",
    "EV_LANDING_PAIN_DEFAULT",
    "EV_LANDING_PAIN_BARK",
    "EV_LANDING_PAIN_BRICK",
    "EV_LANDING_PAIN_CARPET",
    "EV_LANDING_PAIN_CLOTH",
    "EV_LANDING_PAIN_CONCRETE",
    "EV_LANDING_PAIN_DIRT",
    "EV_LANDING_PAIN_FLESH",
    "EV_LANDING_PAIN_FOLIAGE",
    "EV_LANDING_PAIN_GLASS",
    "EV_LANDING_PAIN_GRASS",
    "EV_LANDING_PAIN_GRAVEL",
    "EV_LANDING_PAIN_ICE",
    "EV_LANDING_PAIN_METAL",
    "EV_LANDING_PAIN_MUD",
    "EV_LANDING_PAIN_PAPER",
    "EV_LANDING_PAIN_PLASTER",
    "EV_LANDING_PAIN_ROCK",
    "EV_LANDING_PAIN_SAND",
    "EV_LANDING_PAIN_SNOW",
    "EV_LANDING_PAIN_WATER",
    "EV_LANDING_PAIN_WOOD",
    "EV_LANDING_PAIN_ASPHALT",
    "EV_FOLIAGE_SOUND",
    "EV_STANCE_FORCE_STAND",
    "EV_STANCE_FORCE_CROUCH",
    "EV_STANCE_FORCE_PRONE",
    "EV_STEP_VIEW",
    "EV_ITEM_PICKUP",
    "EV_AMMO_PICKUP",
    "EV_NOAMMO",
    "EV_EMPTYCLIP",
    "EV_EMPTY_OFFHAND",
    "EV_RESET_ADS",
    "EV_RELOAD",
    "EV_RELOAD_FROM_EMPTY",
    "EV_RELOAD_START",
    "EV_RELOAD_END",
    "EV_RAISE_WEAPON",
    "EV_PUTAWAY_WEAPON",
    "EV_WEAPON_ALT",
    "EV_PULLBACK_WEAPON",
    "EV_FIRE_WEAPON",
    "EV_FIRE_WEAPONB",
    "EV_FIRE_WEAPON_LASTSHOT",
    "EV_RECHAMBER_WEAPON",
    "EV_EJECT_BRASS",
    "EV_MELEE_SWIPE",
    "EV_FIRE_MELEE",
    "EV_PREP_OFFHAND",
    "EV_USE_OFFHAND",
    "EV_SWITCH_OFFHAND",
    "EV_BINOCULAR_ENTER",
    "EV_BINOCULAR_EXIT",
    "EV_BINOCULAR_FIRE",
    "EV_BINOCULAR_RELEASE",
    "EV_BINOCULAR_DROP",
    "EV_MELEE_HIT",
    "EV_MELEE_MISS",
    "EV_FIRE_WEAPON_MG42",
    "EV_FIRE_QUADBARREL_1",
    "EV_FIRE_QUADBARREL_2",
    "EV_BULLET_TRACER",
    "EV_SOUND_ALIAS",
    "EV_SOUND_ALIAS_AS_MASTER",
    // The binaries' table reads SMALL, LARGE, SHOTGUN here; vcod names
    // 181..183 by what the cgame plays (module docs).
    "EV_SHOTGUN_HIT",
    "EV_BULLET_HIT_SMALL",
    "EV_BULLET_HIT_LARGE",
    "EV_BULLET_HIT_AP",
    "EV_BULLET_HIT_CLIENT_SMALL",
    "EV_BULLET_HIT_CLIENT_LARGE",
    "EV_GRENADE_BOUNCE",
    "EV_GRENADE_EXPLODE",
    "EV_ROCKET_EXPLODE",
    "EV_ROCKET_EXPLODE_NOMARKS",
    "EV_CUSTOM_EXPLODE",
    "EV_CUSTOM_EXPLODE_NOMARKS",
    "EV_BULLET",
    "EV_PLAY_FX",
    "EV_PLAY_FX_ON_TAG",
    "EV_EARTHQUAKE",
    "EV_GRENADE_SUICIDE",
    "EV_OBITUARY",
];

pub const EV_NONE: i32 = 0;
pub const EV_FOLIAGE_SOUND: i32 = 139;
pub const EV_STANCE_FORCE_STAND: i32 = 140;
pub const EV_STANCE_FORCE_CROUCH: i32 = 141;
pub const EV_STANCE_FORCE_PRONE: i32 = 142;
pub const EV_STEP_VIEW: i32 = 143;
pub const EV_ITEM_PICKUP: i32 = 144;
pub const EV_AMMO_PICKUP: i32 = 145;
pub const EV_NOAMMO: i32 = 146;
pub const EV_EMPTYCLIP: i32 = 147;
pub const EV_EMPTY_OFFHAND: i32 = 148;
pub const EV_RESET_ADS: i32 = 149;
pub const EV_RELOAD: i32 = 150;
pub const EV_RELOAD_FROM_EMPTY: i32 = 151;
pub const EV_RELOAD_START: i32 = 152;
pub const EV_RELOAD_END: i32 = 153;
pub const EV_RAISE_WEAPON: i32 = 154;
pub const EV_PUTAWAY_WEAPON: i32 = 155;
pub const EV_WEAPON_ALT: i32 = 156;
pub const EV_PULLBACK_WEAPON: i32 = 157;
pub const EV_FIRE_WEAPON: i32 = 158;
pub const EV_FIRE_WEAPONB: i32 = 159;
pub const EV_FIRE_WEAPON_LASTSHOT: i32 = 160;
pub const EV_RECHAMBER_WEAPON: i32 = 161;
pub const EV_EJECT_BRASS: i32 = 162;
pub const EV_MELEE_SWIPE: i32 = 163;
pub const EV_FIRE_MELEE: i32 = 164;
pub const EV_PREP_OFFHAND: i32 = 165;
pub const EV_USE_OFFHAND: i32 = 166;
pub const EV_SWITCH_OFFHAND: i32 = 167;
pub const EV_BINOCULAR_ENTER: i32 = 168;
pub const EV_BINOCULAR_EXIT: i32 = 169;
pub const EV_BINOCULAR_FIRE: i32 = 170;
pub const EV_BINOCULAR_RELEASE: i32 = 171;
pub const EV_BINOCULAR_DROP: i32 = 172;
pub const EV_MELEE_HIT: i32 = 173;
pub const EV_MELEE_MISS: i32 = 174;
pub const EV_FIRE_WEAPON_MG42: i32 = 175;
pub const EV_FIRE_QUADBARREL_1: i32 = 176;
pub const EV_FIRE_QUADBARREL_2: i32 = 177;
pub const EV_BULLET_TRACER: i32 = 178;
pub const EV_SOUND_ALIAS: i32 = 179;
pub const EV_SOUND_ALIAS_AS_MASTER: i32 = 180;
pub const EV_SHOTGUN_HIT: i32 = 181;
pub const EV_BULLET_HIT_SMALL: i32 = 182;
pub const EV_BULLET_HIT_LARGE: i32 = 183;
pub const EV_BULLET_HIT_AP: i32 = 184;
pub const EV_BULLET_HIT_CLIENT_SMALL: i32 = 185;
pub const EV_BULLET_HIT_CLIENT_LARGE: i32 = 186;
pub const EV_GRENADE_BOUNCE: i32 = 187;
pub const EV_GRENADE_EXPLODE: i32 = 188;
pub const EV_ROCKET_EXPLODE: i32 = 189;
pub const EV_ROCKET_EXPLODE_NOMARKS: i32 = 190;
pub const EV_CUSTOM_EXPLODE: i32 = 191;
pub const EV_CUSTOM_EXPLODE_NOMARKS: i32 = 192;
pub const EV_BULLET: i32 = 193;
pub const EV_PLAY_FX: i32 = 194;
pub const EV_PLAY_FX_ON_TAG: i32 = 195;
pub const EV_EARTHQUAKE: i32 = 196;
pub const EV_GRENADE_SUICIDE: i32 = 197;
pub const EV_OBITUARY: i32 = 198;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_line_up_with_the_names() {
        assert_eq!(event_name(EV_FOOTSTEP_RUN), Some("EV_FOOTSTEP_RUN_DEFAULT"));
        assert_eq!(
            event_name(EV_FOOTSTEP_WALK - 1),
            Some("EV_FOOTSTEP_RUN_ASPHALT")
        );
        assert_eq!(
            event_name(EV_LANDING_PAIN + SURFACE_EVENTS),
            Some("EV_FOLIAGE_SOUND")
        );
        assert_eq!(event_name(EV_JUMP + 22), Some("EV_JUMP_ASPHALT"));
        assert_eq!(event_name(EV_OBITUARY), Some("EV_OBITUARY"));
        assert_eq!(event_name(199), None);
        assert_eq!(event_of(ET_EVENTS + EV_PLAY_FX), Some(EV_PLAY_FX));
        assert_eq!(event_of(ET_TURRET), None);
    }
}
