//! Playerstate `pm_type` values and `eFlags` / `pm_flags` bits, shared by the
//! server's `ClientSim::to_wire` and the client's `predict::from_wire` so both
//! directions read one definition.

/// `pm_type`. `PM_NORMAL_LINKED` and `PM_DEAD_LINKED` are CoDExtended's names
/// for a linked live player and a linked dead one; the two values and the
/// decrement `G_RunClient` applies without a link record are read out of the
/// module (docs/research/cod11-gsc-object-model.md, 23.2).
pub const PM_NORMAL: i32 = 0;
pub const PM_NORMAL_LINKED: i32 = 1;
/// What `SpectatorThink` writes.
pub const PM_SPECTATOR: i32 = 4;
/// What `ClientEndFrame`'s third arm writes (map-cycle doc, 6.2); the dm
/// map-change capture's post-end traces read it.
pub const PM_INTERMISSION: i32 = 5;
/// Read off both retail deaths (combat doc, section 8).
pub const PM_DEAD: i32 = 6;
pub const PM_DEAD_LINKED: i32 = 7;

/// Stance bits in `eFlags` and `pm_flags`, measured off the retail server
/// under each input (`crates/server/tests/fixtures/playerstate/*-motion.txt`):
/// standing reads `eFlags` 16 / `pm_flags` 0x40000 and crouched 48 / 0x40002.
/// The two `eFlags` bits are exclusive. `pm_flags` 0x1 marks prone and 0x2 is
/// a crouch latch rather than a stance bit: prone entered from a crouch reads
/// 0x40003 and prone entered from standing 0x40001, which is why
/// `PlayerState::ducked` carries it.
pub const EF_CROUCH: i32 = 0x20;
pub const EF_PRONE: i32 = 0x40;
/// The per-spawn toggle: every spawn flips it, a spectator's and the
/// intermission camera's included, and a level boundary clears it
/// (docs/research/cod11-map-cycle.md, 8.2). Not per life --
/// `mp_carentan-sd-roundrestart-target.txt` reads 24 on two consecutive
/// lives (`!trace ms=28607` and `ms=118532`) because the spectator frame
/// between them took the intervening flip. The `dm` hit capture's near-even
/// split (115 samples at 16 against 101 at 24) is that same alternation seen
/// from a run whose spawns happened to pair off.
/// INFERRED: a client breaks interpolation on the
/// changed word, so without it a respawn smears from the corpse to the spawn.
/// The same bit `bodies::EFLAGS_ANIM_TOGGLE` inverts per body-queue push, for
/// the same reason: a changed `eFlags` is what makes a client stop carrying
/// the previous occupant of that entity number forward.
pub const EF_TELEPORT_BIT: i32 = 0x8;
/// The mounted-gun bits by the gun's stance (turrets doc, 4.4 and 12.1);
/// `EF_MOUNTED` masks the pair.
pub const EF_MOUNTED: i32 = 0xC000;
pub const EF_MOUNTED_STAND: i32 = 0xC000;
pub const EF_MOUNTED_DUCK: i32 = 0x8000;
pub const EF_MOUNTED_PRONE: i32 = 0x4000;

pub const PMF_PRONE: i32 = 0x1;
pub const PMF_DUCKED: i32 = 0x2;
/// The prone dive (`pmove::PlayerState::prone_dive`).
pub const PMF_PRONE_DIVE: i32 = 0x4;
/// Held jump, retail's 0x8 (set @0x2ec34, cleared @0x34135); the capture reads
/// `pm_flags` 0x40008 on the first airborne frame.
pub const PMF_JUMP_HELD: i32 = 0x8;
/// Backpedalling, retail's 0x40. The anim selection reads this bit rather than
/// the usercmd, and both captures carry it at `run_back` and nowhere else.
pub const PMF_BACKWARDS_RUN: i32 = 0x40;
/// `PMF_RESPAWNED`: set by every spawn, cleared by the first `PmoveSingle`
/// at `pm_type` 5 or below without attack held
/// (docs/research/cod11-spectator-follow.md, 13).
pub const PMF_RESPAWNED: i32 = 0x800;
/// The own-body bit, third of the view-source group: a live client looking
/// out of its own body carries it and neither spectator view does
/// (docs/research/cod11-gsc-object-model.md, section 20).
pub const PMF_OWN_VIEW: i32 = 0x40000;
