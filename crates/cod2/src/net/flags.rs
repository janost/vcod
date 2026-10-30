//! CoD2 1.3's `pm_flags` and `eFlags` bits, in CoD2's own layout (vcod maps
//! nothing onto 1.1's). Meanings and evidence are
//! docs/research/cod2-movement.md 10 and 12; a bit with no name there has
//! none here.

/// `pm_flags` (`ps + 0xc`, 27 bits on the wire).
pub const PMF_PRONE: i32 = 0x1;
pub const PMF_DUCKED: i32 = 0x2;
pub const PMF_MANTLE: i32 = 0x4;
pub const PMF_OFFHAND: i32 = 0x10;
pub const PMF_LADDER: i32 = 0x20;
pub const PMF_SIGHT: i32 = 0x40;
pub const PMF_BACKWARDS_RUN: i32 = 0x80;
pub const PMF_SIGHT_WALK: i32 = 0x100;
pub const PMF_TIME_LAND: i32 = 0x200;
pub const PMF_TIME_KNOCKBACK: i32 = 0x400;
pub const PMF_PRONE_SIGHT: i32 = 0x800;
pub const PMF_SPAWN_FIRE_LATCH: i32 = 0x1000;
pub const PMF_FROZEN: i32 = 0x8000;
pub const PMF_PRONE_REFUSED: i32 = 0x10000;
pub const PMF_BINOCULARS: i32 = 0x20000;
pub const PMF_LADDER_LEFT: i32 = 0x40000;
pub const PMF_TIME_JUMP_SLOWDOWN: i32 = 0x80000;
/// `MSG_SetDefaultUserCmd` reads the stance and lean off the playerstate
/// only while this is set (cod2-protocol.md 4.3).
pub const PMF_CMD_STANCE: i32 = 0x800000;
/// Set on a spectator's playerstate in every live capture
/// (cod2-protocol.md 9); its meaning is not read off a binary.
pub const PMF_SPECTATOR_SEEN: i32 = 0x1000000;

/// `eFlags` (entity and playerstate, 24 bits).
/// Flipped on a teleport, a spawn and a reused body queue slot; the client
/// lerps nothing across a flip (cod2-protocol.md 12.3).
pub const EF_TELEPORT_BIT: i32 = 0x2;
pub const EF_CROUCHING: i32 = 0x4;
pub const EF_PRONE: i32 = 0x8;
pub const EF_FIRING: i32 = 0x40;
/// A turret gunner's stance (both bits a mask).
pub const EF_TURRET_STANCE: i32 = 0x300;
pub const EF_MANTLE: i32 = 0x4000;
pub const EF_TALK: i32 = 0x200000;
