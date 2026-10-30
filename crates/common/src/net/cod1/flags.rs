//! Playerstate `pm_type` values, entity `eType` values, `eFlags` /
//! `pm_flags` bits and the brush-model `solid`, shared by the server that
//! writes them and the client that reads them so both directions read one
//! definition.

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
/// A player entity whose `pm_type` reads above 5, dead or dead and linked:
/// `BG_PlayerStateToEntityState` sets and clears it (0x2cda6..0x2cdb2).
pub const EF_DEAD: i32 = 0x1;
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
/// A corpse cloned off a live player in the last 250 ms (UO's 0x400 in
/// its own layout). The client plays its death anim from the top, and an
/// entering corpse with it replays its event ring.
pub const EF_CORPSE_FRESH: i32 = 0x800;
/// The mounted-gun bits by the gun's stance (turrets doc, 4.4 and 12.1);
/// `EF_MOUNTED` masks the pair.
pub const EF_MOUNTED: i32 = 0xC000;
pub const EF_MOUNTED_STAND: i32 = 0xC000;
pub const EF_MOUNTED_DUCK: i32 = 0x8000;
pub const EF_MOUNTED_PRONE: i32 = 0x4000;
/// Hides an entity's model. The per-entity runner (game.mp 0x602bc) mirrors
/// `hide()`'s `flags & 0x1000` into it every frame for an entity with no
/// client (movers doc, section 14).
pub const EF_NODRAW: i32 = 0x100;
/// On a turret entity: the server fired it this frame (turrets doc 6.3).
pub const EF_FIRING: i32 = 0x400;
/// `PmoveSingle` sets it on every cmd with the talk bit and clears it on
/// every other (cod15-movement.md 5, "The talk button"); UO's 0x20000.
pub const EF_TALK: i32 = 0x40000;
/// An `ET_PLAYER`'s `pingPlayer` bit (docs/research/cod11-hud-protocol.md,
/// "Compass friendlies").
pub const EF_PING: i32 = 0x80000;
/// The playerstate's copy of it for the `iCompassFriendInfo` teammate.
pub const EF_FRIEND_PING: i32 = 0x100000;

pub const PMF_PRONE: i32 = 0x1;
pub const PMF_DUCKED: i32 = 0x2;
/// The prone dive (`pmove::PlayerState::prone_dive`).
pub const PMF_PRONE_DIVE: i32 = 0x4;
/// A refused prone press or swing (`pmove::PlayerState::prone_blocked`).
pub const PMF_PRONE_BLOCKED: i32 = 0x8000;
/// Held jump, retail's 0x8 (set @0x2ec34, cleared @0x34135); the capture reads
/// `pm_flags` 0x40008 on the first airborne frame.
pub const PMF_JUMP_HELD: i32 = 0x8;
/// On a ladder, retail's 0x10: set and cleared by the ladder probe
/// (game.mp.i386.so 0x336e8); the ladder capture reads `pm_flags` 0x40010 on
/// the rungs (docs/research/cod11-combat.md, 9.7).
pub const PMF_LADDER: i32 = 0x10;
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
/// `pm_flags` on a frame copied from the followed client
/// (docs/research/cod11-spectator-follow.md).
pub const PMF_FOLLOW: i32 = 0x10000;
/// `pm_flags` on a follow the script asked for (`spectatorclient >= 0`).
pub const PMF_FORCED_FOLLOW: i32 = 0x20000;

/// Patch 1.5's jump penalty (docs/research/cod15-movement.md). 1.1 never
/// sets 0x2000, so the bit keeps its 1.5 place in the 1.1 layout, where
/// only a 1.5 movement mode reads it.
pub const PMF_JUMP_PENALTY: i32 = 0x2000;

/// 1.5-only `pm_flags` state a protocol 6 server keeps. vcod holds them
/// above 1.1's 19-bit word, so a protocol 1 wire never carries them;
/// [`pm_flags_v1_to_v6`] puts each on its 1.5 bit
/// (docs/research/cod15-protocol-diff.md, "pm_flags").
/// A spectator that may cycle whom it follows: 1.5's 0x40000.
pub const PMF_MAY_CYCLE: i32 = 0x80000;
/// A following spectator that may leave to free flight: 1.5's 0x80000.
pub const PMF_MAY_FREE: i32 = 0x100000;
/// `disableWeapon`: 1.5's 0x100000.
pub const PMF_WEAPON_DISABLED: i32 = 0x200000;
/// 1.1's `pm_flags` field width.
const V1_PM_FLAGS_MASK: i32 = (1 << 19) - 1;

/// What a protocol 1 wire carries of vcod's `pm_flags`: everything but the
/// 1.5-only bits.
pub fn pm_flags_v1_wire(f: i32) -> i32 {
    f & V1_PM_FLAGS_MASK
}

/// vcod's `pm_flags` in protocol 6's layout, the inverse of
/// [`pm_flags_v6_to_v1`] plus the 1.5-only bits. A follow the script did not
/// force may cycle, as 1.5's `SpectatorClientEndFrame` (game.mp 0x39edd)
/// sets 0x40000 on every such copy. [`PMF_JUMP_PENALTY`] rides as is.
pub fn pm_flags_v1_to_v6(f: i32) -> i32 {
    const V6_OWN_VIEW: i32 = 0x20000;
    const V6_MAY_CYCLE: i32 = 0x40000;
    const V6_MAY_FREE: i32 = 0x80000;
    const V6_WEAPON_DISABLED: i32 = 0x100000;
    let mut out = f & 0xffff;
    if f & PMF_FOLLOW != 0 {
        out |= PMF_FOLLOW;
        if f & PMF_FORCED_FOLLOW == 0 {
            out |= V6_MAY_CYCLE;
        }
    }
    for (bit, v6) in [
        (PMF_OWN_VIEW, V6_OWN_VIEW),
        (PMF_MAY_CYCLE, V6_MAY_CYCLE),
        (PMF_MAY_FREE, V6_MAY_FREE),
        (PMF_WEAPON_DISABLED, V6_WEAPON_DISABLED),
    ] {
        if f & bit != 0 {
            out |= v6;
        }
    }
    out
}

/// Protocol 6 `pm_flags` in 1.1's layout (docs/research/cod15-protocol-diff.md,
/// "pm_flags"). 0x1..0x8000 keep their place, 0x2000 among them (1.5's jump
/// penalty, unused by 1.1); the view bits above them were renumbered.
/// `disableWeapon`'s 0x100000 lands on [`PMF_WEAPON_DISABLED`], which the
/// predictor reads; the follow permissions drop.
pub fn pm_flags_v6_to_v1(f: i32) -> i32 {
    const V6_OWN_VIEW: i32 = 0x20000;
    const V6_MAY_CYCLE: i32 = 0x40000;
    const V6_WEAPON_DISABLED: i32 = 0x100000;
    let mut out = f & 0xffff;
    if f & V6_WEAPON_DISABLED != 0 {
        out |= PMF_WEAPON_DISABLED;
    }
    if f & PMF_FOLLOW != 0 {
        out |= PMF_FOLLOW;
        if f & V6_MAY_CYCLE == 0 {
            out |= PMF_FORCED_FOLLOW;
        }
    }
    if f & V6_OWN_VIEW != 0 {
        out |= PMF_OWN_VIEW;
    }
    out
}

/// Protocol 22's view and permission bits (docs/research/coduo-diff.md,
/// section 11): 1.5's moved up two places past sprinting (0x10000) and
/// winded (0x20000), which have no 1.1 bit.
const V22_FOLLOW: i32 = 0x40000;
const V22_OWN_VIEW: i32 = 0x80000;
const V22_MAY_CYCLE: i32 = 0x100000;
const V22_MAY_FREE: i32 = 0x200000;
const V22_WEAPON_DISABLED: i32 = 0x400000;
const V22_SPRINT: i32 = 0x10000;
const V22_WINDED: i32 = 0x20000;
const V22_OVERHEATED: i32 = 0x800000;

/// United Offensive's sprinting, winded and gunner-overheated `pm_flags`
/// (coduo-diff.md section 11), which have no 1.1 bit: vcod keeps them above
/// protocol 22's 24 bits.
pub const PMF_SPRINT_V22: i32 = 0x0100_0000;
pub const PMF_WINDED_V22: i32 = 0x0200_0000;
pub const PMF_OVERHEATED_V22: i32 = 0x0400_0000;

/// Protocol 22 `pm_flags` in vcod's layout: 0x1..0x8000 as on 1.5, the
/// view bits as [`pm_flags_v6_to_v1`] reads 1.5's.
pub fn pm_flags_v22_to_v1(f: i32) -> i32 {
    const FOLLOW: i32 = V22_FOLLOW;
    const OWN_VIEW: i32 = V22_OWN_VIEW;
    const MAY_CYCLE: i32 = V22_MAY_CYCLE;
    let mut out = f & 0xffff;
    if f & V22_WEAPON_DISABLED != 0 {
        out |= PMF_WEAPON_DISABLED;
    }
    if f & FOLLOW != 0 {
        out |= PMF_FOLLOW;
        if f & MAY_CYCLE == 0 {
            out |= PMF_FORCED_FOLLOW;
        }
    }
    if f & OWN_VIEW != 0 {
        out |= PMF_OWN_VIEW;
    }
    for (v22, bit) in [
        (V22_SPRINT, PMF_SPRINT_V22),
        (V22_WINDED, PMF_WINDED_V22),
        (V22_OVERHEATED, PMF_OVERHEATED_V22),
    ] {
        if f & v22 != 0 {
            out |= bit;
        }
    }
    out
}

/// The inverse of [`pm_flags_v22_to_v1`], as [`pm_flags_v1_to_v6`] is of
/// 1.5's.
pub fn pm_flags_v1_to_v22(f: i32) -> i32 {
    let mut out = f & 0xffff;
    if f & PMF_FOLLOW != 0 {
        out |= V22_FOLLOW;
        if f & PMF_FORCED_FOLLOW == 0 {
            out |= V22_MAY_CYCLE;
        }
    }
    for (bit, v22) in [
        (PMF_OWN_VIEW, V22_OWN_VIEW),
        (PMF_MAY_CYCLE, V22_MAY_CYCLE),
        (PMF_MAY_FREE, V22_MAY_FREE),
        (PMF_WEAPON_DISABLED, V22_WEAPON_DISABLED),
        (PMF_SPRINT_V22, V22_SPRINT),
        (PMF_WINDED_V22, V22_WINDED),
        (PMF_OVERHEATED_V22, V22_OVERHEATED),
    ] {
        if f & bit != 0 {
            out |= v22;
        }
    }
    out
}

/// Protocol 22 `eFlags` in 1.1's layout. United Offensive dropped one bit
/// below 1.1's `EF_NODRAW` and moved everything from it up down one place:
/// `hide()` mirrors into 0x80 (game.mp.uo.i386.so, Ghidra `FUN_00069a35`,
/// where 1.1's 0x602bc writes 0x100), firing reads 0x200, the mount pair
/// 0x6000 and the ping bits 0x40000 and 0x80000 (coduo-diff.md, section
/// 11). The vehicle bits 0x100000..0x400000 land above 1.1's highest.
pub fn eflags_v22_to_v1(f: i32) -> i32 {
    (f & 0x7f) | ((f & !0x7f) << 1)
}

/// The inverse of [`eflags_v22_to_v1`]. 1.1's 0x80, the bit UO dropped,
/// has no place to go and nothing in vcod sets it.
pub fn eflags_v1_to_v22(f: i32) -> i32 {
    (f & 0x7f) | ((f >> 1) & !0x7f)
}

/// `eFlags` on a protocol 22 player in a vehicle seat, in 1.1's layout:
/// `eflags_v22_to_v1` of the 0x100000 the mount sets (coduo-diff.md 9). On
/// a projectile's explode it marks a vehicle hit.
pub const EF_IN_VEHICLE_V22: i32 = 0x200000;
/// UO's 0x200000 on a seated player, in 1.1's layout: the mount sets it,
/// a tank's think clears it, and a four-wheel's sets it again unless
/// `upmove` is held. The four-wheel's riders turn with it while it is set
/// (uo cgame 0x30040580; coduo-vehicles.md 5).
pub const EF_HULL_VIEW_V22: i32 = 0x400000;
/// UO's 0x400000, a rider popped out of the hatch (`G_VehiclePopOut`), in
/// 1.1's layout.
pub const EF_POPPED_OUT_V22: i32 = 0x800000;

/// `eType` values (CoDExtended's `entityType_t`; `ET_EVENTS` is in
/// `net::events`).
pub const ET_GENERAL: i32 = 0;
pub const ET_PLAYER: i32 = 1;
/// The client resolves the body model through `clientNum` on the roster
/// (`docs/research/clientstate-wire-format.md`).
pub const ET_CORPSE: i32 = 2;
/// An item, `index` its `bg_itemlist` row (a weapon's is its configstring 7
/// index).
pub const ET_ITEM: i32 = 3;
pub const ET_MISSILE: i32 = 4;
pub const ET_MOVER: i32 = 5;
pub const ET_PORTAL: i32 = 6;
pub const ET_INVISIBLE: i32 = 7;
/// A script model, `index` a model configstring index, or a
/// `script_brushmodel`, `index` its inline model number.
pub const ET_SCRIPTMOVER: i32 = 8;
/// A `playLoopedFx` effect (game.mp 0x5b536): `scale` the effect id,
/// `origin2` the forward, `angles2` the cull distance and the period in ms.
/// cgame's `CG_AddCEntity` case 10 plays it (0x3001cf20,
/// docs/research/cod11-events-and-fx.md 2).
pub const ET_FX: i32 = 10;
/// A mounted MG (`misc_mg42` / `misc_turret`). Not in CoDExtended's
/// `entityType_t`, read off the traces: carentan's and pavlov's `misc_mg42`s
/// arrive as 11 with the `mg42_bipod` model index.
pub const ET_TURRET: i32 = 11;

/// United Offensive's own entity types (docs/research/coduo-diff.md,
/// section 8), which 1.1 spends on event entities: a crossfading engine
/// sound, a vehicle (sent in the second entity table), a destroyed one, a
/// vehicle's collision brush and its owner icon.
pub const ET_SOUND_BLEND_V22: i32 = 9;
pub const ET_VEHICLE_V22: i32 = 12;
pub const ET_VEHICLE_CORPSE_V22: i32 = 13;
pub const ET_VEHICLE_COLLMAP_V22: i32 = 14;
pub const ET_VEHICLE_OWNER_ICON_V22: i32 = 15;

/// `solid` of an entity linked as a brush model: `SV_LinkEntity` stores it
/// for `r.bmodel` (cod_lnxded 0x80908da) whatever the entity's contents, so a
/// `notSolid()`ed brush model keeps it (docs/research/cod11-movers.md 14).
pub const SOLID_BMODEL: i32 = 0xff_ffff;

/// The restart toggle on `legsAnim`, `torsoAnim` and `ps.weapAnim`: every
/// `set_anim` flips it, a repeat of the same clip included, so the index is
/// the low 9 bits (player-model-anim-system.md, "Animation indices"; combat
/// doc, 1.2).
pub const ANIM_TOGGLEBIT: i32 = 512;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v6_pm_flags_land_on_v1_bits() {
        let map = pm_flags_v6_to_v1;
        // A live player, standing and sighted: 1.5's own-view bit is 0x20000.
        assert_eq!(map(0x20000 | 0x20), PMF_OWN_VIEW | 0x20);
        // Crouched after a jump: the jump-penalty bit keeps its place.
        assert_eq!(
            map(0x20000 | 0x2000 | PMF_DUCKED),
            PMF_OWN_VIEW | PMF_JUMP_PENALTY | PMF_DUCKED
        );
        // Following, free to cycle and to fly: plain follow.
        assert_eq!(map(PMF_FOLLOW | 0x40000 | 0x80000), PMF_FOLLOW);
        // Following where the script holds the view.
        assert_eq!(map(PMF_FOLLOW), PMF_FOLLOW | PMF_FORCED_FOLLOW);
        // A free spectator's permissions drop; `disableWeapon` is kept.
        assert_eq!(map(0x40000), 0);
        assert_eq!(map(0x20000 | 0x100000), PMF_OWN_VIEW | PMF_WEAPON_DISABLED);
    }

    #[test]
    fn v1_pm_flags_land_on_v6_bits() {
        let map = pm_flags_v1_to_v6;
        assert_eq!(map(PMF_OWN_VIEW | PMF_DUCKED), 0x20000 | PMF_DUCKED);
        assert_eq!(map(PMF_FOLLOW | PMF_FORCED_FOLLOW), PMF_FOLLOW);
        assert_eq!(
            map(PMF_FOLLOW | PMF_MAY_FREE),
            PMF_FOLLOW | 0x40000 | 0x80000
        );
        assert_eq!(map(PMF_MAY_CYCLE), 0x40000);
        assert_eq!(map(PMF_OWN_VIEW | PMF_WEAPON_DISABLED), 0x20000 | 0x100000);
        // Every 1.1 value survives the round trip.
        for f in [0x40000, 0x40003, 0x10000, 0x30000, 0x40800, 0x4c010, 0x8000] {
            assert_eq!(pm_flags_v6_to_v1(map(f)), f, "{f:#x}");
        }
    }

    #[test]
    fn the_v1_wire_drops_the_v6_only_bits() {
        let f = PMF_OWN_VIEW | PMF_MAY_CYCLE | PMF_MAY_FREE | PMF_WEAPON_DISABLED | 0x8;
        assert_eq!(pm_flags_v1_wire(f), PMF_OWN_VIEW | 0x8);
    }

    #[test]
    fn v22_flags_land_on_v1_bits() {
        // The live captures: a free spectator 0x100000, a spawned player's
        // own view 0x80000 (coduo-diff.md 11).
        assert_eq!(pm_flags_v22_to_v1(0x100000), 0);
        assert_eq!(
            pm_flags_v22_to_v1(0x80000 | PMF_DUCKED),
            PMF_OWN_VIEW | PMF_DUCKED
        );
        // Sprinting, winded and overheated have no 1.1 bit; vcod keeps
        // them above the wire's 24.
        assert_eq!(
            pm_flags_v22_to_v1(0x80000 | 0x10000 | 0x20000 | 0x800000),
            PMF_OWN_VIEW | PMF_SPRINT_V22 | PMF_WINDED_V22 | PMF_OVERHEATED_V22
        );
        assert_eq!(pm_flags_v22_to_v1(0x40000), PMF_FOLLOW | PMF_FORCED_FOLLOW);
        assert_eq!(pm_flags_v22_to_v1(0x40000 | 0x100000), PMF_FOLLOW);
        for f in [
            PMF_FOLLOW,
            PMF_FOLLOW | PMF_FORCED_FOLLOW,
            PMF_OWN_VIEW | PMF_DUCKED,
            PMF_OWN_VIEW | PMF_SPRINT_V22 | PMF_WINDED_V22,
        ] {
            assert_eq!(pm_flags_v22_to_v1(pm_flags_v1_to_v22(f)), f, "{f:#x}");
        }

        let map = eflags_v22_to_v1;
        // A standing player reads 16 on both, crouched and prone keep their place.
        assert_eq!(
            map(0x10 | EF_CROUCH | EF_PRONE | EF_TELEPORT_BIT),
            0x10 | EF_CROUCH | EF_PRONE | EF_TELEPORT_BIT
        );
        assert_eq!(map(0x80), EF_NODRAW);
        assert_eq!(map(0x200), EF_FIRING);
        assert_eq!(map(0x6000), EF_MOUNTED);
        assert_eq!(map(0x20000), EF_TALK);
        assert_eq!(map(0x40000), EF_PING);
        assert_eq!(map(0x80000), EF_FRIEND_PING);
        assert_eq!(map(0x100000), EF_IN_VEHICLE_V22);
    }
}
