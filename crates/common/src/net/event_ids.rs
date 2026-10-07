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
