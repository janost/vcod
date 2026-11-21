//! CoD4 1.7's state values as the wire words carry them (`u32`): the
//! `pm_type`s of [`crate::pmove::pm_type`], `eFlags` bits, `team_t`,
//! `solid` and the weapon def's type and class numbers. Each user cites
//! where it reads them.

use crate::pmove::pm_type;

pub const PM_SPECTATOR: u32 = pm_type::SPECTATOR as u32;
pub const PM_INTERMISSION: u32 = pm_type::INTERMISSION as u32;
pub const PM_DEAD: u32 = pm_type::DEAD as u32;

/// `eFlags`: crouched, prone, and on a turret (both stance bits).
pub const EF_CROUCH: u32 = 0x4;
pub const EF_PRONE: u32 = 0x8;
pub const EF_TURRET: u32 = 0x300;

/// `team_t`'s spectator.
pub const TEAM_SPECTATOR: u32 = 3;

/// `entityState_t.solid` of a brush model.
pub const SOLID_BMODEL: u32 = 0xff_ffff;

/// `WeaponDef.weapType` 1, a grenade.
pub const WEAPTYPE_GRENADE: i32 = 1;
/// `WeaponDef.weapClass` 7, a turret.
pub const WEAPCLASS_TURRET: i32 = 7;
