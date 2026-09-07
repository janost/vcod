//! The weapon half of pmove: retail's `PM_Weapon`
//! (`game.mp.i386.so` 0x390e0, `cgame_mp_x86.dll` 0x30011ab0), read out in
//! docs/research/cod11-combat.md section 1. Section numbers below are that
//! document's; the behaviour the two combat captures measured is in
//! docs/research/player-model-anim-system.md, "The weapon channel".
//!
//! Not modelled yet: the alt-weapon clause of the switch path, retail's
//! `BG_TakePlayerWeapon` on a spent `clipOnly` weapon (the host owns the held
//! bits, so pmove cannot clear one), and the three stops of 1.12, whose
//! `pm_flags`/`pm_type`/`eFlags` this `PlayerState` does not carry.
//!
//! One deliberate divergence from a VERIFIED retail reading: retail's
//! weapon-change check treats a `cmd.weapon` of 0 as a request to holster
//! (1.8), and [`requested_weapon`] reads it as "the cmd asks for nothing" and
//! keeps the current weapon, so a caller that does not thread the byte yet
//! is not disarmed every frame.

use super::{PlayerState, PmEvent, PmInput, Stance};
use crate::weapon::WeaponDef;

/// `ps.weaponstate`, the twelve values of section 1.1.
pub const WEAPON_READY: u8 = 0;
pub const WEAPON_RAISING: u8 = 1;
pub const WEAPON_DROPPING: u8 = 2;
pub const WEAPON_FIRING: u8 = 3;
pub const WEAPON_RECHAMBERING: u8 = 4;
pub const WEAPON_RELOADING: u8 = 5;
pub const WEAPON_RELOADING_INTERUPT: u8 = 6;
pub const WEAPON_RELOAD_START: u8 = 7;
pub const WEAPON_RELOAD_START_INTERUPT: u8 = 8;
pub const WEAPON_RELOAD_END: u8 = 9;
pub const WEAPON_MELEE_WINDUP: u8 = 10;
pub const WEAPON_MELEE_RELAX: u8 = 11;

/// Wire `EV_*` numbering (docs/research/cod11-events-and-fx.md). `EV_EMPTYCLIP`
/// (150) has no constant on purpose: section 1.5 found no site that raises it.
pub const EV_NOAMMO: i32 = 149;
pub const EV_RELOAD: i32 = 151;
pub const EV_RELOAD_FROM_EMPTY: i32 = 152;
pub const EV_RELOAD_START: i32 = 153;
pub const EV_RELOAD_END: i32 = 154;
pub const EV_RAISE_WEAPON: i32 = 155;
pub const EV_PUTAWAY_WEAPON: i32 = 156;
pub const EV_PULLBACK_WEAPON: i32 = 158;
pub const EV_FIRE_WEAPON: i32 = 159;
pub const EV_FIRE_WEAPON_LASTSHOT: i32 = 161;
pub const EV_RECHAMBER_WEAPON: i32 = 162;
pub const EV_EJECT_BRASS: i32 = 163;
pub const EV_MELEE_SWIPE: i32 = 164;
pub const EV_FIRE_MELEE: i32 = 165;

/// `ps.weapAnim` indices, the `WEAP_*` order of section 1.2.
pub const WEAP_IDLE: i32 = 0;
pub const WEAP_ATTACK: i32 = 2;
pub const WEAP_ATTACK_LASTSHOT: i32 = 3;
pub const WEAP_RECHAMBER: i32 = 4;
pub const WEAP_ADS_ATTACK: i32 = 5;
pub const WEAP_ADS_ATTACK_LASTSHOT: i32 = 6;
pub const WEAP_ADS_RECHAMBER: i32 = 7;
pub const WEAP_MELEE_ATTACK: i32 = 8;
pub const WEAP_DROP: i32 = 9;
pub const WEAP_RAISE: i32 = 10;
pub const WEAP_RELOAD: i32 = 11;
pub const WEAP_RELOAD_EMPTY: i32 = 12;
pub const WEAP_RELOAD_START: i32 = 13;
pub const WEAP_RELOAD_END: i32 = 14;
/// The pullback has no name in the binary's printer: it is index 17, past the
/// seventeen the switch there covers (section 1.11).
pub const WEAP_GRENADE_PULLBACK: i32 = 17;
/// Bit 512, the restart toggle, is not part of the index (section 1.2).
const ANIM_TOGGLEBIT: i32 = 512;

/// `ps.ammo` and `ps.ammoclip` are 64 entries each, indexed by the weapon
/// def's ammo and clip index and not by weapon (docs/protocol-1.1.md).
pub const NUM_AMMO: usize = 64;
/// `ps.weaponslots`, eight bytes across two 32-bit netfields.
pub const NUM_SLOTS: usize = 8;

/// `giveWeapon`: hold the weapon and put it in its file's `weaponSlot`. A
/// weapon whose file names no slot is still held; only the slot byte goes
/// unset. Shared with `vcod_server::weapons::PlayerWeapons`.
pub fn give_slot(held: &mut u64, slots: &mut [u8; NUM_SLOTS], index: u8, slot: usize) {
    if (index as u32) < u64::BITS {
        *held |= 1u64 << index;
    }
    if slot > 0 && slot < NUM_SLOTS {
        slots[slot] = index;
    }
}

/// Retail's `COM_BitCheck(ps.weapons, index)`.
pub fn bit_set(held: u64, index: u8) -> bool {
    (index as u32) < u64::BITS && held & (1u64 << index) != 0
}

/// The two 32-bit words `weaponslots[0]` and `weaponslots[4]` carry.
pub fn pack_slots(slots: &[u8; NUM_SLOTS]) -> [i32; 2] {
    let word = |b: &[u8]| i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    [word(&slots[0..4]), word(&slots[4..8])]
}

pub fn give(ps: &mut PlayerState, index: u8, slot: usize) {
    give_slot(&mut ps.weapons_held, &mut ps.weapon_slots, index, slot);
}

pub fn holds(ps: &PlayerState, index: u8) -> bool {
    bit_set(ps.weapons_held, index)
}

pub fn slot_words(ps: &PlayerState) -> [i32; 2] {
    pack_slots(&ps.weapon_slots)
}

fn ms(seconds: f32) -> i32 {
    (seconds * 1000.0).round() as i32
}

fn weapon_def(weapons: &[Option<WeaponDef>], index: u8) -> Option<&WeaponDef> {
    weapons.get(index as usize).and_then(Option::as_ref)
}

/// Every write flips the toggle, `WEAP_IDLE` included, so a repeated shot
/// restarts its clip and a clear is visible as a change (section 1.2; the
/// sustained fire of player-model-anim-system.md ends at 253 and the channel
/// then reads 512).
fn set_anim(ps: &mut PlayerState, index: i32) {
    ps.weap_anim = index | ((ps.weap_anim ^ ANIM_TOGGLEBIT) & ANIM_TOGGLEBIT);
}

/// The index-preserving setter (section 1.2): it writes nothing when the index
/// is already the one asked for, which is how a state holds its anim across
/// frames without restarting the clip.
fn hold_anim(ps: &mut PlayerState, index: i32) {
    if ps.weap_anim & !ANIM_TOGGLEBIT != index {
        set_anim(ps, index);
    }
}

fn push(events: &mut Vec<PmEvent>, event: i32) {
    events.push(PmEvent { event, parm: 0 });
}

/// The usercmd's weapon byte, with 0 read as "the cmd asks for nothing". A
/// retail cmd always carries the client's current weapon and treats a real 0
/// as a switch to no weapon; a vcod caller that does not thread the byte yet
/// sends 0 and must not be read as asking to be disarmed.
fn requested_weapon(ps: &PlayerState, input: &PmInput) -> u8 {
    if input.weapon == 0 {
        ps.weapon
    } else {
        input.weapon
    }
}

fn rechamber_pending(ps: &PlayerState) -> bool {
    bit_set(ps.weapon_rechamber, ps.weapon)
}

fn clear_rechamber(ps: &mut PlayerState) {
    if (ps.weapon as u32) < u64::BITS {
        ps.weapon_rechamber &= !(1u64 << ps.weapon);
    }
}

/// `pm_flags` 0x20, the gated ADS flag. The usercmd's sight bit reaches
/// nothing else: the fraction's ramp and the animscript's `ads` condition
/// both read this instead (combat doc, 1.13).
pub const PMF_ADS: i32 = 0x20;
/// `pm_flags` 0x80, the ADS walk slow-down, which rides on 0x20.
pub const PMF_ADS_WALK: i32 = 0x80;

/// Retail's fallbacks for a weapon file that spells no transition time,
/// derived at load into reciprocal milliseconds (combat doc, 1.13).
const ADS_TRANS_IN_DEFAULT_MS: f32 = 300.0;
const ADS_TRANS_OUT_DEFAULT_MS: f32 = 500.0;
/// What an airborne frame adds to `aimSpreadScale` before the frame-time
/// scaling: 1.28 twice, in two separate adds (combat doc, 2.1).
const AIR_SPREAD_ADD: f32 = 1.28 + 1.28;

/// A transition time in milliseconds, or retail's fallback when the file
/// spells none.
fn trans_ms(seconds: f32, fallback: f32) -> f32 {
    let ms = ms(seconds) as f32;
    if ms > 0.0 {
        ms
    } else {
        fallback
    }
}

/// `PM_UpdateAimDownSightFlag` (`game.mp.i386.so` 0x37230), combat doc 1.13.
/// Every clause is a way to lose the sight: no `aimDownSight` on the weapon,
/// a weapon being raised, holstered or swung, and being off the ground. The
/// dead clear is `pmove::dead_move`'s, since no pmove step runs for a dead
/// player at all.
pub fn update_ads_flag(ps: &mut PlayerState, input: &PmInput, def: Option<&WeaponDef>) {
    let keep = input.ads
        && def.is_some_and(|d| d.aim_down_sight)
        && !matches!(
            ps.weaponstate,
            WEAPON_RAISING | WEAPON_DROPPING | WEAPON_MELEE_WINDUP | WEAPON_MELEE_RELAX
        )
        && ps.on_ground;
    if !keep {
        ps.ads_active = false;
    } else if ps.stance != Stance::Prone {
        ps.ads_active = true;
    } else if !(ps.last_cmd_ads && (input.forward != 0.0 || input.right != 0.0)) {
        // A prone player crawling with the sight already held on the previous
        // cmd keeps the flag as it is; every other prone frame sets it.
        ps.ads_active = true;
    }
}

/// The `pm_flags` bits the ADS path owns. 0x80 is
/// `PM_UpdatePlayerWalkingFlag`'s (`0x33694`): it rides on 0x20 but drops for
/// a prone player and for the whole of a reload, while 0x20 stays on.
/// `pm_flags` 0x400, which the prone arm ORs in beside 0x20, is unnamed and
/// unmodelled (combat doc, section 10).
pub fn ads_pm_flags(ps: &PlayerState) -> i32 {
    if !ps.ads_active {
        return 0;
    }
    let walking = ps.stance != Stance::Prone
        && !matches!(ps.weaponstate, WEAPON_RELOADING..=WEAPON_RELOAD_END);
    PMF_ADS | if walking { PMF_ADS_WALK } else { 0 }
}

/// The ADS fraction, `ps.fWeaponPosFrac`: `PM_UpdateAimDownSightLerp`
/// (`game.mp.i386.so` 0x372fc, `cgame_mp_x86.dll` 0x3000fc80), the first
/// thing `PM_Weapon` does (combat doc, 1.13). A linear ramp by
/// `msec / adsTransInTime` toward the sight while `pm_flags` 0x20 is held and
/// by `msec / adsTransOutTime` back to the hip when it is not, pinned at 0
/// for all of a reload but its last `adsReloadTransTime`.
///
/// The client predicts the same function off each snapshot's playerstate, so
/// a fraction that is merely plausible here is re-based into its prediction
/// twenty times a second and reads as the weapon twitching.
fn advance_ads(ps: &mut PlayerState, def: &WeaponDef, dt_ms: i32) {
    if !def.aim_down_sight {
        ps.weapon_pos_frac = 0.0;
        return;
    }
    // The sight may start coming back up once the reload has less than
    // `adsReloadTransTime` left to run.
    let tail = ps.weapon_time_ms - ms(def.ads_reload_trans_time) <= 0;
    let reloading = if def.segmented_reload {
        match ps.weaponstate {
            WEAPON_RELOADING..=WEAPON_RELOAD_START_INTERUPT => true,
            WEAPON_RELOAD_END => !tail,
            _ => false,
        }
    } else {
        ps.weaponstate == WEAPON_RELOADING && !tail
    };
    // An `adsFire` weapon aims itself: the shot in flight forces the sight up
    // whatever the button says.
    let want = (ps.ads_active && !reloading)
        || (def.ads_fire && ps.weapon_delay_ms != 0 && ps.weaponstate == WEAPON_FIRING);
    let step = dt_ms as f32
        / if want {
            trans_ms(def.ads_trans_in, ADS_TRANS_IN_DEFAULT_MS)
        } else {
            -trans_ms(def.ads_trans_out, ADS_TRANS_OUT_DEFAULT_MS)
        };
    ps.weapon_pos_frac = (ps.weapon_pos_frac + step).clamp(0.0, 1.0);
}

/// `PM_AdjustAimSpreadScale` (`game.mp.i386.so` 0x385e8, dll 0x30011050),
/// combat doc 2.1: the hip cone opens while the player moves, turns or falls
/// and decays at the weapon's own rate whatever else happens. Both halves
/// carry the same `* 255.0`, so a standing frame of the carbine's decay is 51
/// of the field's 0..255 range rather than 0.2.
///
/// `def` is `None` for a weapon index with no file, which reads the same as
/// `hipSpreadDecayRate 0`: a flat -255 a frame, so the cone shuts within one.
pub fn adjust_aim_spread_scale(
    ps: &mut PlayerState,
    input: &PmInput,
    def: Option<&WeaponDef>,
    dt: f32,
) {
    // A swing holds the cone wide open: every sample of `weaponstate` 10 and
    // 11 in the capture's `melee_tap` reads 255.00 and the decay starts only
    // once the swing is over. Measured from the capture; no store in the
    // binary is located for it.
    if matches!(ps.weaponstate, WEAPON_MELEE_WINDUP | WEAPON_MELEE_RELAX) {
        ps.aim_spread_scale = 255.0;
        return;
    }
    let (add, decay) = match def {
        Some(def) if def.hip_spread_decay_rate != 0.0 => {
            let mut decay = def.hip_spread_decay_rate;
            // The three stance arms are exclusive and airborne wins.
            if !ps.on_ground {
                decay *= 0.5;
            } else if ps.stance == Stance::Prone {
                decay *= def.hip_spread_prone_decay;
            } else if ps.stance == Stance::Crouch {
                decay *= def.hip_spread_ducked_decay;
            }
            let (mut add, mut turn) = (0.0, 0.0);
            // A settled sight takes none of the additions; the decay runs
            // whatever the fraction is.
            if ps.weapon_pos_frac != 1.0 {
                if def.hip_spread_turn_add != 0.0 {
                    for axis in 0..2 {
                        let d = angle_delta_deg(input.angles[axis], ps.last_cmd_angles[axis]);
                        // Retail divides this term by the frame time and
                        // multiplies the whole add by it again, so turning
                        // opens the cone by the angle alone at any frame
                        // rate; written as the product the two reduce to.
                        turn += d.abs() * 0.01 * def.hip_spread_turn_add;
                    }
                }
                if def.hip_spread_move_add != 0.0 && (input.forward != 0.0 || input.right != 0.0) {
                    add += def.hip_spread_move_add;
                }
                if !ps.on_ground {
                    add += AIR_SPREAD_ADD;
                }
            }
            (add * dt + turn, decay * dt)
        }
        // `hipSpreadDecayRate 0` short-circuits to a decay of 1.0 with no
        // frame-time scaling at all, which empties the counter in one frame.
        _ => (0.0, 1.0),
    };
    ps.aim_spread_scale = (ps.aim_spread_scale + (add - decay) * 255.0).clamp(0.0, 255.0);
}

/// `AngleSubtract(SHORT2ANGLE(a), SHORT2ANGLE(b))`: the shorter way round
/// between two wire angles, in degrees.
fn angle_delta_deg(a: i32, b: i32) -> f32 {
    const SHORT2ANGLE: f32 = 360.0 / 65536.0;
    let d = (a - b) as f32 * SHORT2ANGLE;
    (d + 180.0).rem_euclid(360.0) - 180.0
}

/// One frame of the weapon machine. `dt_ms` is retail's `pml.msec`.
pub fn pm_weapon(
    ps: &mut PlayerState,
    input: &PmInput,
    weapons: &[Option<WeaponDef>],
    dt_ms: i32,
    events: &mut Vec<PmEvent>,
) {
    // The timers run whether or not the current weapon resolves: an index with
    // no def, weapon 0 on a ladder among them, must not freeze them.
    let delay_expired = advance_timers(ps, weapon_def(weapons, ps.weapon), input, dt_ms);
    // The fraction rides on the held weapon's own transition times, so it
    // stands still while the weapon has no def to read them from.
    if let Some(def) = weapon_def(weapons, ps.weapon) {
        advance_ads(ps, def, dt_ms);
    }

    // "finish a raise": the state ends when `raiseTime` does. A held trigger
    // keeps it going, the semi-automatic latch pinning `weaponTime` at 1
    // (section 1.4): the capture's `cancel` step raises the carbine under a
    // held bit and reads `weaponstate` 1 for 2.7 s, where `to_frag` leaves it
    // after `raiseTime`.
    if ps.weaponstate == WEAPON_RAISING && ps.weapon_time_ms == 0 {
        ps.weaponstate = WEAPON_READY;
        set_anim(ps, WEAP_IDLE);
    }
    if leave_ladder(ps, input, weapons, events) {
        return;
    }
    let Some(def) = weapon_def(weapons, ps.weapon) else {
        // Any index the table cannot resolve, which with the stock table is
        // only weapon 0: nothing to read a melee, a reload or a shot from.
        // The switch path still runs, which is what lets a player whose last
        // grenade `BG_TakePlayerWeapon` took ask for another weapon and get
        // it (section 1.8, and 1.12 for the three stops that really do end
        // `PM_Weapon`). Weapon 0 takes `putaway`'s short branch, so the
        // pickup lands on this frame; a held index with no def would take the
        // ordinary path and write `WEAP_DROP` with a drop time of 0.
        if begin_change(ps, input, None, events)
            && ps.weaponstate == WEAPON_DROPPING
            && ps.weapon_time_ms == 0
        {
            pickup(ps, weapons, events);
        }
        return;
    };
    melee_finish(ps, def, delay_expired, events);
    if melee_check(ps, input, def, delay_expired, events) {
        return;
    }
    if begin_change(ps, input, Some(def), events) {
        // The pickup half runs later in the same `PM_Weapon` (section 1.8), so
        // a putaway that left no drop time -- the cancelled pullback, a weapon
        // gone from under the player -- raises on this frame and never shows
        // `weaponstate` 2.
        if ps.weaponstate == WEAPON_DROPPING && ps.weapon_time_ms == 0 {
            pickup(ps, weapons, events);
        }
        return;
    }
    if reload_check(ps, input, def, events) {
        return;
    }
    if rechamber_check(ps, def, delay_expired, events) {
        return;
    }
    if reload_machine(ps, def, delay_expired, events) {
        return;
    }
    // "finish a weapon change": the pickup half runs only in state 2.
    if ps.weaponstate == WEAPON_DROPPING {
        if ps.weapon_time_ms == 0 {
            pickup(ps, weapons, events);
        }
        return;
    }
    if ps.weapon_time_ms != 0 {
        return;
    }
    if grenade_hold(ps, input, def, events) {
        return;
    }
    // Releasing the trigger (section 1.6).
    if !input.attack && !delay_expired {
        if ps.weaponstate == WEAPON_FIRING {
            ps.weaponstate = WEAPON_READY;
            hold_anim(ps, WEAP_IDLE);
        }
        return;
    }
    fire(ps, def, events);
}

/// Sections 1.3 and 1.4. Returns whether `weaponDelay` reached 0 on this
/// frame, the single edge the rest of the machine is written around.
fn advance_timers(
    ps: &mut PlayerState,
    def: Option<&WeaponDef>,
    input: &PmInput,
    dt_ms: i32,
) -> bool {
    let delay_expired = ps.weapon_delay_ms > 0 && ps.weapon_delay_ms - dt_ms < 1;
    ps.weapon_delay_ms = (ps.weapon_delay_ms - dt_ms).max(0);
    if ps.weapon_time_ms == 0 {
        return delay_expired;
    }
    ps.weapon_time_ms -= dt_ms;
    if ps.weapon_time_ms >= 1 {
        return delay_expired;
    }
    // The semi-automatic latch: a held trigger pins `weaponTime` at 1, so the
    // weapon never reaches the 0 the fire path is gated on (section 1.4).
    let latched = def.is_some_and(|def| {
        def.semi_auto
            && input.attack
            && requested_weapon(ps, input) == ps.weapon
            && ps.ammoclip[def.clip_index] != 0
    });
    ps.weapon_time_ms = i32::from(latched);
    // The anim leaves the shot on this frame even when the state does not.
    // A rechamber ending writes no anim at all: pavlov's capture drops from
    // `weaponstate` 4 to 0 with `weapAnim` still reading the rechamber index,
    // and its sustained fire never shows the idle index between two shots.
    //
    // A shot ends its state here too, and not in the trigger-release check
    // below: pavlov's mosin goes from `weaponstate` 3 straight to 4 with one
    // server frame between the samples, which it cannot do if the rechamber
    // has to wait for a later frame to see the state ready. The ordering
    // section 1.6 gives is INFERRED; this is measured. A latched trigger is
    // the exception, and is what keeps a held semi-automatic in state 3 with
    // the idle pose (section 1.4).
    match ps.weaponstate {
        WEAPON_RECHAMBERING => ps.weaponstate = WEAPON_READY,
        // The swing relaxing into the idle pose (section 1.10). Retail's
        // `melee_tap` reads `weapAnim` 520 through the swing and a bare 0
        // after, so the write flips the toggle like every other one.
        WEAPON_MELEE_RELAX => {
            ps.weaponstate = WEAPON_READY;
            set_anim(ps, WEAP_IDLE);
        }
        WEAPON_FIRING => {
            hold_anim(ps, WEAP_IDLE);
            if !latched {
                ps.weaponstate = WEAPON_READY;
            }
        }
        _ => {}
    }
    delay_expired
}

/// Section 1.10, the swing. The one edge latch in the machine: a held bit
/// swings once, and the latch clears only when the bit comes back up.
fn melee_check(
    ps: &mut PlayerState,
    input: &PmInput,
    def: &WeaponDef,
    delay_expired: bool,
    events: &mut Vec<PmEvent>,
) -> bool {
    // The clear is unconditional (section 1.10): a bit that came back up
    // re-arms the latch whatever the rest of the check would have said.
    if !input.melee {
        ps.melee_latched = false;
        return false;
    }
    if def.melee_damage == 0 || delay_expired {
        return false;
    }
    // A reload is the one busy state a swing interrupts.
    if ps.weapon_delay_ms != 0 && !matches!(ps.weaponstate, WEAPON_RELOADING..=WEAPON_RELOAD_END) {
        return false;
    }
    if ps.melee_latched {
        return false;
    }
    ps.melee_latched = true;
    if matches!(
        ps.weaponstate,
        WEAPON_RAISING | WEAPON_DROPPING | WEAPON_MELEE_WINDUP | WEAPON_MELEE_RELAX
    ) {
        return false;
    }
    set_anim(ps, WEAP_MELEE_ATTACK);
    push(events, EV_MELEE_SWIPE);
    // The timers and the state are the `meleeDelay` arm's; a weapon that
    // spells none swings its anim and nothing else. No stock weapon does.
    if ms(def.melee_delay) != 0 {
        ps.weapon_time_ms = ms(def.melee_time);
        ps.weapon_delay_ms = ms(def.melee_delay);
        ps.weaponstate = WEAPON_MELEE_WINDUP;
    }
    true
}

/// Section 1.10, the frame the swing connects: the hit event, and the rest of
/// `meleeTime` to relax in.
fn melee_finish(
    ps: &mut PlayerState,
    def: &WeaponDef,
    delay_expired: bool,
    events: &mut Vec<PmEvent>,
) {
    if ps.weaponstate != WEAPON_MELEE_WINDUP || !delay_expired {
        return;
    }
    ps.weapon_time_ms = ps
        .weapon_time_ms
        .max(ms(def.melee_time) - ms(def.melee_delay));
    push(events, EV_FIRE_MELEE);
    ps.weaponstate = WEAPON_MELEE_RELAX;
}

/// The weapon-change check of section 1.8. A jump and a stance change do
/// nothing here: the captures that read `weaponstate` 2 at both were taken
/// with `cmd.weapon` 0, which this check reads as a request to holster.
fn begin_change(
    ps: &mut PlayerState,
    input: &PmInput,
    def: Option<&WeaponDef>,
    events: &mut Vec<PmEvent>,
) -> bool {
    // A pullback is the one `weaponstate` 3 a switch may interrupt, and it
    // interrupts it whatever `weaponDelay` reads: the capture's `cancel` step
    // switches 300 ms into a hold and the raise lands 60 ms later
    // (section 1.14).
    if ps.grenade_time_left_ms == 0 {
        if matches!(
            ps.weaponstate,
            WEAPON_FIRING | WEAPON_MELEE_WINDUP | WEAPON_MELEE_RELAX
        ) || ps.weapon_delay_ms != 0
        {
            return false;
        }
        // Busy, unless the state is one a switch may interrupt: a reload or a
        // rechamber can be cut short, a shot cannot.
        if ps.weapon_time_ms != 0
            && !matches!(ps.weaponstate, WEAPON_RECHAMBERING..=WEAPON_RELOAD_END)
        {
            return false;
        }
    }
    // The ladder forces weapon 0 (section 1.8's `pm_flags & 0x10`; 1.12 reads
    // that bit as the ladder).
    if ps.on_ladder && ps.weapon != 0 {
        ps.stowed_weapon = ps.weapon;
        return putaway(ps, def, 0, events);
    }
    if ps.weapon != 0 && !holds(ps, ps.weapon) {
        return putaway(ps, def, 0, events);
    }
    let wanted = requested_weapon(ps, input);
    if wanted != ps.weapon && holds(ps, wanted) {
        return putaway(ps, def, wanted, events);
    }
    false
}

/// The putaway a caller outside the machine starts: `switchToWeapon` asks for
/// the same change a usercmd's weapon byte does, and goes through the drop
/// and the raise rather than swapping the weapon in place (section 1.8).
pub fn begin_switch(
    ps: &mut PlayerState,
    def: &WeaponDef,
    target: u8,
    events: &mut Vec<PmEvent>,
) -> bool {
    putaway(ps, Some(def), target, events)
}

fn putaway(
    ps: &mut PlayerState,
    def: Option<&WeaponDef>,
    target: u8,
    events: &mut Vec<PmEvent>,
) -> bool {
    if ps.weaponstate == WEAPON_DROPPING {
        return false;
    }
    ps.weapon_delay_ms = 0;
    ps.pending_weapon = target;
    ps.weaponstate = WEAPON_DROPPING;
    // The short branch of section 1.8: a weapon that is gone from under the
    // player, and a grenade still on its pin, go without a drop time, an anim
    // or an event. Retail ORs `pm_flags` 0x400 in only for a prone player;
    // this flag is set on every cancel, since nothing reads it either way.
    if ps.weapon == 0 || !holds(ps, ps.weapon) || ps.grenade_time_left_ms > 0 {
        ps.weapon_time_ms = 0;
        if ps.grenade_time_left_ms > 0 {
            ps.grenade_time_left_ms = 0;
            ps.grenade_cancelled = true;
        }
        return true;
    }
    ps.weapon_time_ms = ms(def.map_or(0.0, |d| d.drop_time));
    // The capture's `to_frag` reads `weapAnim` 521 through the whole putaway,
    // `WEAP_DROP` with the toggle flipped. The superseded captures read the
    // anim unchanged because they sent `cmd.weapon` 0 every frame, which is
    // the one input the setter refuses to write on (section 1.2).
    set_anim(ps, WEAP_DROP);
    events.push(PmEvent {
        event: EV_PUTAWAY_WEAPON,
        parm: WEAP_DROP,
    });
    true
}

/// The pickup half of section 1.8. Retail re-reads `cmd.weapon` here; vcod
/// raises the weapon the putaway latched, so the putaway a jump forces comes
/// back to the same weapon even when the caller threads no weapon byte. The
/// ladder forces 0 the same way retail's `pm_flags & 0x10` does: without it
/// this raises a weapon the next frame's ladder clause holsters again.
fn pickup(ps: &mut PlayerState, weapons: &[Option<WeaponDef>], events: &mut Vec<PmEvent>) {
    let target = ps.pending_weapon;
    ps.pending_weapon = 0;
    let old = ps.weapon;
    ps.weapon = if ps.on_ladder || !holds(ps, target) {
        0
    } else {
        target
    };
    // The two arms are exclusive and each writes `weapAnim` once: the same
    // weapon back in hand goes idle, a different one raises. The capture's
    // `to_frag` counts the toggle flips that prove it (section 1.14).
    let raising = ps.weapon != old;
    if let (true, Some(def)) = (raising, weapon_def(weapons, ps.weapon)) {
        ps.weaponstate = WEAPON_RAISING;
        ps.weapon_time_ms = ms(def.raise_time);
        // A weapon coming up starts with the cone wide open (section 1.8).
        ps.aim_spread_scale = 255.0;
        set_anim(ps, WEAP_RAISE);
        push(events, EV_RAISE_WEAPON);
        return;
    }
    ps.weaponstate = WEAPON_READY;
    set_anim(ps, WEAP_IDLE);
}

/// The other half of the ladder rule. Retail's pickup takes `cmd.weapon`, so a
/// client that keeps sending the byte re-arms itself off the ladder; a vcod
/// caller that sends no byte would stay disarmed, so the weapon the ladder
/// holstered comes back instead.
fn leave_ladder(
    ps: &mut PlayerState,
    input: &PmInput,
    weapons: &[Option<WeaponDef>],
    events: &mut Vec<PmEvent>,
) -> bool {
    if ps.on_ladder
        || ps.weapon != 0
        || ps.stowed_weapon == 0
        || ps.weaponstate != WEAPON_READY
        || ps.weapon_time_ms != 0
    {
        return false;
    }
    ps.pending_weapon = if input.weapon != 0 {
        input.weapon
    } else {
        ps.stowed_weapon
    };
    ps.stowed_weapon = 0;
    pickup(ps, weapons, events);
    true
}

/// "Can this weapon reload" (section 1.7).
fn can_reload(ps: &PlayerState, def: &WeaponDef) -> bool {
    let clip = ps.ammoclip[def.clip_index];
    if ps.ammo[def.ammo_index] == 0 || clip >= def.clip_size as i16 {
        return false;
    }
    if !def.no_partial_reload {
        return true;
    }
    if def.reload_ammo_add == 0 || def.reload_ammo_add >= def.clip_size {
        clip == 0
    } else {
        def.clip_size as i16 - clip >= def.reload_ammo_add as i16
    }
}

/// The reload check of section 1.7: the key, the keyless reload on a dry clip,
/// and the attack bit interrupting a segmented one.
fn reload_check(
    ps: &mut PlayerState,
    input: &PmInput,
    def: &WeaponDef,
    events: &mut Vec<PmEvent>,
) -> bool {
    if def.segmented_reload && input.attack {
        match ps.weaponstate {
            WEAPON_RELOAD_START => ps.weaponstate = WEAPON_RELOAD_START_INTERUPT,
            WEAPON_RELOADING => ps.weaponstate = WEAPON_RELOADING_INTERUPT,
            _ => {}
        }
    }
    if !matches!(
        ps.weaponstate,
        WEAPON_READY | WEAPON_FIRING | WEAPON_RECHAMBERING
    ) || ps.weapon_time_ms != 0
    {
        return false;
    }
    if input.reload && can_reload(ps, def) {
        return begin_reload(ps, def, events);
    }
    // The keyless one, which refuses to run for a prone player who is moving.
    let prone_moving = ps.stance == Stance::Prone && (input.forward != 0.0 || input.right != 0.0);
    if ps.ammoclip[def.clip_index] == 0
        && ps.ammo[def.ammo_index] != 0
        && ps.weaponstate != WEAPON_FIRING
        && !prone_moving
    {
        return begin_reload(ps, def, events);
    }
    false
}

fn begin_reload(ps: &mut PlayerState, def: &WeaponDef, events: &mut Vec<PmEvent>) -> bool {
    if def.segmented_reload && def.reload_start_time > 0.0 {
        ps.weaponstate = WEAPON_RELOAD_START;
        ps.weapon_time_ms = ms(def.reload_start_time);
        ps.weapon_delay_ms = reload_delay(ps, def, ms(def.reload_start_add_time));
        set_anim(ps, WEAP_RELOAD_START);
        push(events, EV_RELOAD_START);
        return true;
    }
    begin_reload_proper(ps, def, events)
}

/// One reload of the clip, or one segment of a segmented reload.
fn begin_reload_proper(ps: &mut PlayerState, def: &WeaponDef, events: &mut Vec<PmEvent>) -> bool {
    let empty = ps.ammoclip[def.clip_index] == 0;
    let (anim, time, event) = if empty {
        (
            WEAP_RELOAD_EMPTY,
            def.reload_empty_time,
            EV_RELOAD_FROM_EMPTY,
        )
    } else {
        (WEAP_RELOAD, def.reload_time, EV_RELOAD)
    };
    ps.weaponstate = if ps.weaponstate == WEAPON_RELOAD_START_INTERUPT {
        WEAPON_RELOADING_INTERUPT
    } else {
        WEAPON_RELOADING
    };
    ps.weapon_time_ms = ms(time);
    ps.weapon_delay_ms = reload_delay(ps, def, ms(def.reload_add_time));
    set_anim(ps, anim);
    push(events, event);
    true
}

/// A reload's `weaponDelay` (section 1.3): the smaller of the add time and the
/// state's own time, or 1 while a bolt-action still holds its spent case, so
/// the brass leaves before the rounds land. An add time of 0 would never fire
/// the edge that loads the clip at all, so it lands at the end of the state.
fn reload_delay(ps: &PlayerState, def: &WeaponDef, add_ms: i32) -> i32 {
    if def.bolt_action && rechamber_pending(ps) {
        let bolt = ms(def.rechamber_bolt_time);
        if bolt > 0 && bolt < ps.weapon_time_ms {
            return bolt;
        }
        return 1;
    }
    if add_ms == 0 {
        return ps.weapon_time_ms;
    }
    add_ms.min(ps.weapon_time_ms)
}

/// The reload state machine of section 1.7: the rounds landing on the delay
/// edge, and the state ending when its time runs out.
fn reload_machine(
    ps: &mut PlayerState,
    def: &WeaponDef,
    delay_expired: bool,
    events: &mut Vec<PmEvent>,
) -> bool {
    if !matches!(ps.weaponstate, WEAPON_RELOADING..=WEAPON_RELOAD_END) {
        return false;
    }
    if delay_expired {
        reload_delay_edge(ps, def, events);
    }
    if ps.weapon_time_ms != 0 {
        return true;
    }
    match ps.weaponstate {
        WEAPON_RELOAD_START | WEAPON_RELOAD_START_INTERUPT => {
            // The start's own add can fill the clip (kar98k_sniper after one
            // shot), in which case no loop segment runs at all.
            let interrupted =
                ps.weaponstate == WEAPON_RELOAD_START_INTERUPT && ps.ammoclip[def.clip_index] != 0;
            if !interrupted && can_reload(ps, def) {
                begin_reload_proper(ps, def, events);
            } else {
                end_reload(ps, def, events);
            }
        }
        WEAPON_RELOADING | WEAPON_RELOADING_INTERUPT => {
            let interrupted = ps.weaponstate == WEAPON_RELOADING_INTERUPT;
            if def.segmented_reload && !interrupted && can_reload(ps, def) {
                clear_rechamber(ps);
                begin_reload_proper(ps, def, events);
            } else {
                end_reload(ps, def, events);
            }
        }
        _ => {
            ps.weaponstate = WEAPON_READY;
            set_anim(ps, WEAP_IDLE);
        }
    }
    true
}

/// A reload's last segment is over: a segmented weapon with a `reloadEndTime`
/// runs its end state, anything else is ready (section 1.7).
fn end_reload(ps: &mut PlayerState, def: &WeaponDef, events: &mut Vec<PmEvent>) {
    clear_rechamber(ps);
    if def.segmented_reload && def.reload_end_time > 0.0 {
        ps.weaponstate = WEAPON_RELOAD_END;
        ps.weapon_time_ms = ms(def.reload_end_time);
        set_anim(ps, WEAP_RELOAD_END);
        push(events, EV_RELOAD_END);
    } else {
        ps.weaponstate = WEAPON_READY;
        set_anim(ps, WEAP_IDLE);
    }
}

/// The frame a reload's `weaponDelay` expires: a bolt-action's spent case
/// leaves first and the delay re-arms, otherwise the rounds land.
fn reload_delay_edge(ps: &mut PlayerState, def: &WeaponDef, events: &mut Vec<PmEvent>) {
    if def.bolt_action && rechamber_pending(ps) {
        clear_rechamber(ps);
        push(events, EV_EJECT_BRASS);
        ps.weapon_delay_ms = reload_delay(ps, def, ms(def.reload_add_time));
        return;
    }
    let (ci, ai) = (def.clip_index, def.ammo_index);
    // The start segment loads `reloadStartAdd` rounds and a loop segment
    // `reloadAmmoAdd`; a 0 start loads nothing, a 0 loop fills the clip.
    let in_start = matches!(
        ps.weaponstate,
        WEAPON_RELOAD_START | WEAPON_RELOAD_START_INTERUPT
    );
    let want = match (in_start, def.reload_start_add, def.reload_ammo_add) {
        (true, 0, _) => return,
        (true, add, _) | (false, _, add) if add > 0 && add < def.clip_size => add as i16,
        _ => def.clip_size as i16,
    };
    let room = (def.clip_size as i16 - ps.ammoclip[ci]).min(want);
    let take = room.min(ps.ammo[ai]).max(0);
    ps.ammoclip[ci] += take;
    ps.ammo[ai] -= take;
}

/// The rechamber check of section 1.9, which runs off the weapon's bit in
/// `ps.weaponrechamber`.
fn rechamber_check(
    ps: &mut PlayerState,
    def: &WeaponDef,
    delay_expired: bool,
    events: &mut Vec<PmEvent>,
) -> bool {
    if !def.bolt_action || !rechamber_pending(ps) {
        return false;
    }
    if ps.weaponstate == WEAPON_RECHAMBERING {
        if delay_expired {
            clear_rechamber(ps);
            push(events, EV_EJECT_BRASS);
        }
        return true;
    }
    if ps.weaponstate != WEAPON_READY || ps.weapon_time_ms != 0 {
        return false;
    }
    ps.weaponstate = WEAPON_RECHAMBERING;
    ps.weapon_time_ms = ms(def.rechamber_time);
    let bolt = ms(def.rechamber_bolt_time);
    // The literal 1 stands in for a bolt time that is 0 or longer than the
    // rechamber itself (section 1.3).
    ps.weapon_delay_ms = if bolt > 0 && bolt < ps.weapon_time_ms {
        bolt
    } else {
        1
    };
    // The rechamber picks its aimed form off the same threshold the shot
    // does (section 1.9).
    set_anim(
        ps,
        if ps.weapon_pos_frac > 0.75 {
            WEAP_ADS_RECHAMBER
        } else {
            WEAP_RECHAMBER
        },
    );
    push(events, EV_RECHAMBER_WEAPON);
    true
}

/// `weaponType 1`, the one type with a pullback between the trigger and the
/// shot (section 1.11).
const WEAPON_TYPE_GRENADE: &str = "grenade";

/// Section 1.11, the pullback: arm the fuse and hold the pin for
/// `holdFireTime`.
fn pullback(ps: &mut PlayerState, def: &WeaponDef, events: &mut Vec<PmEvent>) {
    ps.grenade_time_left_ms = ms(def.fuse_time);
    ps.grenade_cancelled = false;
    set_anim(ps, WEAP_GRENADE_PULLBACK);
    push(events, EV_PULLBACK_WEAPON);
    ps.weapon_delay_ms = ms(def.hold_fire_time);
    ps.weapon_time_ms = 0;
    ps.weaponstate = WEAPON_FIRING;
}

/// Sections 1.11 and 1.14, the frames an armed grenade owns. Nothing counts
/// the fuse down: the throw is the trigger coming up once `holdFireTime` has
/// run out, and while the bit is held the capture reads `weaponDelay` pinned
/// at 1, the same shape the semi-automatic latch gives `weaponTime`
/// (section 1.4). Returns whether the frame is spent.
fn grenade_hold(
    ps: &mut PlayerState,
    input: &PmInput,
    def: &WeaponDef,
    events: &mut Vec<PmEvent>,
) -> bool {
    if ps.grenade_time_left_ms == 0 {
        return false;
    }
    if input.attack {
        ps.weapon_delay_ms = ps.weapon_delay_ms.max(1);
        return true;
    }
    // Released before the pin ran out: the throw waits for it.
    if ps.weapon_delay_ms > 1 {
        return true;
    }
    ps.weapon_delay_ms = 0;
    fire(ps, def, events);
    true
}

/// Section 1.5, and the anim pick of 1.2, which reads the ADS fraction
/// rather than the usercmd's sight bit.
fn fire(ps: &mut PlayerState, def: &WeaponDef, events: &mut Vec<PmEvent>) {
    if ps.weapon_delay_ms != 0 {
        return;
    }
    let grenade = def.weapon_type == WEAPON_TYPE_GRENADE;
    // A grenade's first trigger pulls the pin; the throw is a later frame,
    // through the rest of this function ([`grenade_hold`]).
    if grenade && ps.grenade_time_left_ms == 0 {
        if ps.ammoclip[def.clip_index] > 0 {
            pullback(ps, def, events);
        }
        return;
    }
    let (ci, ai) = (def.clip_index, def.ammo_index);
    if ps.ammoclip[ci] < 1 {
        if ps.ammo[ai] > 0 {
            begin_reload(ps, def, events);
        } else {
            // A grenade raises nothing here (section 1.5 step 2).
            if !grenade {
                push(events, EV_NOAMMO);
            }
            set_anim(ps, WEAP_IDLE);
            ps.weapon_time_ms += 500;
        }
        return;
    }
    ps.ammoclip[ci] -= 1;
    if def.bolt_action && (ps.weapon as u32) < u64::BITS {
        ps.weapon_rechamber |= 1u64 << ps.weapon;
    }
    ps.weaponstate = WEAPON_FIRING;
    ps.weapon_time_ms = ms(def.fire_time);
    // A grenade keeps `weaponDelay` at the 0 the release left it (step 1),
    // which is what the capture's three throw frames read.
    if !grenade {
        ps.weapon_delay_ms = ms(def.fire_delay);
    }
    // An `adsFire` weapon's shot waits out whatever is left of the raise
    // (`.so` 0x38b23, section 1.5 step 1).
    if def.ads_fire {
        ps.weapon_delay_ms = ((1.0 - ps.weapon_pos_frac)
            * trans_ms(def.ads_trans_in, ADS_TRANS_IN_DEFAULT_MS))
            as i32;
    }
    // The shot's own spread add (step 8), skipped from a settled sight.
    if ps.weapon_pos_frac != 1.0 {
        ps.aim_spread_scale = (ps.aim_spread_scale + def.hip_spread_fire_add * 255.0).min(255.0);
    }
    let last = ps.ammoclip[ci] == 0;
    set_anim(
        ps,
        match (ps.weapon_pos_frac > 0.75, last) {
            (false, false) => WEAP_ATTACK,
            (false, true) => WEAP_ATTACK_LASTSHOT,
            (true, false) => WEAP_ADS_ATTACK,
            (true, true) => WEAP_ADS_ATTACK_LASTSHOT,
        },
    );
    // A throw carries what is left of the fuse, so the server never has to
    // read `grenadeTimeLeft` back off a step that already cleared it.
    let parm = ps.grenade_time_left_ms;
    ps.grenade_time_left_ms = 0;
    events.push(PmEvent {
        event: if last {
            EV_FIRE_WEAPON_LASTSHOT
        } else {
            EV_FIRE_WEAPON
        },
        parm,
    });
    // Section 1.5 step 9: a `clipOnly` weapon with nothing left tells the
    // client so. Retail also takes the weapon away here, which pmove cannot:
    // the host owns `ps.weapons`.
    if def.clip_only && last && ps.ammo[ai] == 0 {
        push(events, EV_NOAMMO);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pmove::PlayerState;

    /// The stock files' shape: `reloadEmptyTime` matches `reloadTime` on the
    /// mosin, and `reloadAddTime` lands the rounds midway.
    fn def(fire: f32, rechamber: f32, reload: f32, drop: f32, clip: u32, bolt: bool) -> WeaponDef {
        WeaponDef {
            clip_size: clip,
            fire_time: fire,
            rechamber_time: rechamber,
            reload_time: reload,
            reload_empty_time: reload,
            reload_add_time: reload * 0.5,
            drop_time: drop,
            raise_time: 0.5,
            semi_auto: true,
            bolt_action: bolt,
            ammo_index: 1,
            clip_index: 1,
            ..WeaponDef::default()
        }
    }

    fn carbine() -> WeaponDef {
        def(0.135, 0.1, 2.65, 0.67, 15, false)
    }

    fn mosin() -> WeaponDef {
        def(0.33, 1.0, 2.4, 0.4, 5, true)
    }

    fn armed(d: &WeaponDef) -> (PlayerState, Vec<Option<WeaponDef>>) {
        let mut ps = PlayerState::spawn(glam::Vec3::ZERO, 0.0);
        give(&mut ps, 1, 1);
        ps.weapon = 1;
        ps.ammoclip[1] = d.clip_size as i16;
        ps.ammo[1] = 30;
        // The ADS flag drops for an airborne player, and every test here but
        // the airborne one is about a player standing on something.
        ps.on_ground = true;
        (ps, vec![None, Some(d.clone())])
    }

    /// One frame in the order `pmove` runs these in: the spread, then the ADS
    /// flag, then the machine, then the cmd is remembered as the next frame's
    /// `oldcmd`.
    fn step(
        ps: &mut PlayerState,
        weapons: &[Option<WeaponDef>],
        input: &PmInput,
        frames: usize,
    ) -> Vec<i32> {
        let mut out = Vec::new();
        for _ in 0..frames {
            let mut ev = Vec::new();
            let def = weapon_def(weapons, ps.weapon).cloned();
            adjust_aim_spread_scale(ps, input, def.as_ref(), 0.05);
            update_ads_flag(ps, input, def.as_ref());
            pm_weapon(ps, input, weapons, 50, &mut ev);
            ps.last_cmd_angles = input.angles;
            ps.last_cmd_ads = input.ads;
            out.extend(ev.iter().map(|e| e.event));
        }
        out
    }

    /// The carbine's own ADS and spread numbers, off `weapons/mp/m1carbine_mp`
    /// in `pak0.pk3` (combat doc, section 0).
    fn carbine_ads() -> WeaponDef {
        WeaponDef {
            aim_down_sight: true,
            ads_trans_in: 0.3,
            ads_trans_out: 0.4,
            ads_reload_trans_time: 0.6,
            hip_spread_max: 5.0,
            hip_spread_fire_add: 0.7,
            hip_spread_decay_rate: 4.0,
            hip_spread_turn_add: 0.0,
            hip_spread_move_add: 8.0,
            hip_spread_ducked_decay: 1.0,
            hip_spread_prone_decay: 1.1,
            ..carbine()
        }
    }

    fn held(ads: bool) -> PmInput {
        PmInput {
            ads,
            ..Default::default()
        }
    }

    fn run(
        ps: &mut PlayerState,
        weapons: &[Option<WeaponDef>],
        attack: bool,
        frames: usize,
    ) -> Vec<i32> {
        let input = PmInput {
            attack,
            ..Default::default()
        };
        step(ps, weapons, &input, frames)
    }

    /// The frag's own numbers, off `weapons/mp/fraggrenade_mp` in `pak0.pk3`.
    fn frag() -> WeaponDef {
        let mut d = def(1.0, 0.0, 2.0, 0.25, 3, false);
        d.weapon_type = "grenade".into();
        d.fuse_time = 4.0;
        d.hold_fire_time = 0.6;
        d.clip_only = true;
        d
    }

    /// The frag's melee numbers; the carbine's own file spells 0.15 and 0.65.
    fn with_melee(mut d: WeaponDef) -> WeaponDef {
        d.melee_damage = 50;
        d.melee_delay = 0.1;
        d.melee_time = 0.66;
        d
    }

    /// 1.11: the pullback arms the fuse for `fuseTime` and holds the pin for
    /// `holdFireTime`; the release after that throws, taking one from the clip
    /// and carrying what is left of the fuse as the event's parm.
    #[test]
    fn a_grenade_pulls_back_and_throws_on_release() {
        let (mut ps, w) = armed(&frag());
        let held = PmInput {
            attack: true,
            ..Default::default()
        };
        let ev = step(&mut ps, &w, &held, 1);
        assert_eq!(ev, vec![EV_PULLBACK_WEAPON]);
        assert_eq!(ps.grenade_time_left_ms, 4000);
        assert_eq!(ps.weap_anim & 511, WEAP_GRENADE_PULLBACK);
        assert_eq!(ps.weapon_delay_ms, 600);
        assert_eq!(ps.weaponstate, WEAPON_FIRING);
        // The fuse does not run down while the trigger is held (the
        // `pin_out` step of `mp_carentan-tdm-grenade.txt`).
        let ev = step(&mut ps, &w, &held, 20);
        assert_eq!(ev, Vec::<i32>::new());
        assert_eq!(ps.grenade_time_left_ms, 4000);
        let mut throw = Vec::new();
        pm_weapon(&mut ps, &PmInput::default(), &w, 50, &mut throw);
        assert_eq!(
            throw,
            vec![PmEvent {
                event: EV_FIRE_WEAPON,
                parm: 4000,
            }]
        );
        assert_eq!(ps.grenade_time_left_ms, 0);
        assert_eq!(ps.weapon_time_ms, 1000);
        assert_eq!(ps.weapon_delay_ms, 0);
        assert_eq!(ps.weap_anim & 511, WEAP_ATTACK);
        assert_eq!(ps.ammoclip[1], 2);
    }

    /// The `pin_out` step of `mp_carentan-tdm-grenade.txt`: retail 1.1 MP
    /// never counts `grenadeTimeLeft` down, so a held trigger cooks forever
    /// and the grenade leaves only on the release.
    #[test]
    fn a_held_grenade_does_not_throw_itself() {
        let (mut ps, w) = armed(&frag());
        let held = PmInput {
            attack: true,
            ..Default::default()
        };
        let ev = step(&mut ps, &w, &held, 120);
        assert_eq!(ev, vec![EV_PULLBACK_WEAPON]);
        assert_eq!(ps.grenade_time_left_ms, 4000);
        assert_eq!(ps.weaponstate, WEAPON_FIRING);
        assert_eq!(ps.ammoclip[1], 3);
    }

    /// An empty clip has nothing to pull: no event, no fuse, no state change.
    #[test]
    fn a_grenade_with_an_empty_clip_does_nothing() {
        let (mut ps, w) = armed(&frag());
        ps.ammoclip[1] = 0;
        ps.ammo[1] = 0;
        let held = PmInput {
            attack: true,
            ..Default::default()
        };
        let ev = step(&mut ps, &w, &held, 4);
        assert_eq!(ev, Vec::<i32>::new());
        assert_eq!(ps.grenade_time_left_ms, 0);
        assert_eq!(ps.weaponstate, WEAPON_READY);
    }

    /// The `cancel` step of `mp_carentan-tdm-grenade.txt`: a weapon change
    /// during a pullback drops the grenade with no putaway at all -- no
    /// `EV_PUTAWAY_WEAPON`, no drop time, no round spent -- and the raise
    /// lands on the same frame.
    #[test]
    fn a_weapon_change_cancels_a_cooking_grenade() {
        let (mut ps, mut w) = armed(&frag());
        w.push(Some(carbine()));
        give(&mut ps, 2, 2);
        let held = PmInput {
            attack: true,
            ..Default::default()
        };
        step(&mut ps, &w, &held, 5);
        assert_eq!(ps.grenade_time_left_ms, 4000);
        let ev = step(
            &mut ps,
            &w,
            &PmInput {
                attack: true,
                weapon: 2,
                ..Default::default()
            },
            1,
        );
        assert_eq!(ev, vec![EV_RAISE_WEAPON]);
        assert_eq!(ps.grenade_time_left_ms, 0);
        assert!(ps.grenade_cancelled);
        assert_eq!(ps.weaponstate, WEAPON_RAISING);
        assert_eq!(ps.weapon, 2);
        assert_eq!(ps.ammoclip[1], 3);
    }

    /// The last frag leaves the player holding nothing, and the machine has
    /// to keep answering the usercmd's weapon byte from there. Retail's
    /// `BG_TakePlayerWeapon` clears the held bit on the last round of a
    /// `clipOnly` weapon (section 1.5, step 9), the change check then begins
    /// a change to 0 because the player no longer owns `ps.weapon`
    /// (section 1.8), and the next weapon the client asks for still has to
    /// arrive: only the three stops of 1.12 end `PM_Weapon` outright.
    #[test]
    fn a_player_who_lost_its_last_grenade_can_switch_back() {
        let (mut ps, mut w) = armed(&frag());
        // The carbine keeps its own clip: the frag's is about to run dry.
        let mut rifle = carbine();
        rifle.clip_index = 2;
        rifle.ammo_index = 2;
        w.push(Some(rifle.clone()));
        give(&mut ps, 2, 2);
        ps.ammoclip[1] = 1;
        ps.ammo[1] = 0;
        ps.ammoclip[2] = rifle.clip_size as i16;
        ps.ammo[2] = 30;
        let held = PmInput {
            attack: true,
            ..Default::default()
        };
        assert_eq!(step(&mut ps, &w, &held, 1), vec![EV_PULLBACK_WEAPON]);
        // The pin holds for `holdFireTime`; the release after it throws.
        assert!(step(&mut ps, &w, &held, 13).is_empty());
        // Section 1.5, step 9: the spent `clipOnly` weapon raises `EV_NOAMMO`
        // beside the shot, and it is that pair the host takes the weapon on.
        assert_eq!(
            step(&mut ps, &w, &PmInput::default(), 1),
            vec![EV_FIRE_WEAPON_LASTSHOT, EV_NOAMMO]
        );
        assert_eq!(ps.ammoclip[1], 0);

        // The host owns the held bits, so this is the take the server mirrors
        // back into the playerstate on the frame after the last shot.
        ps.weapons_held &= !(1u64 << 1);
        let asks_for_the_frag = PmInput {
            weapon: 1,
            ..Default::default()
        };
        step(&mut ps, &w, &asks_for_the_frag, 40);
        // A change to 0 happens and stays there while the client keeps asking
        // for a weapon it does not own: the retail grenade capture's
        // `idle_after` reads `weapon` 0 under exactly that input.
        assert_eq!(ps.weapon, 0);
        assert_eq!(ps.weaponstate, WEAPON_READY);

        let asks_for_the_carbine = PmInput {
            weapon: 2,
            ..Default::default()
        };
        let ev = step(&mut ps, &w, &asks_for_the_carbine, 1);
        // Weapon 0 has no drop time, so the raise lands on the same frame.
        assert_eq!(ev, vec![EV_RAISE_WEAPON]);
        assert_eq!(ps.weapon, 2);
        step(&mut ps, &w, &asks_for_the_carbine, 20);
        assert_eq!(ps.weaponstate, WEAPON_READY);
        assert_eq!(
            step(
                &mut ps,
                &w,
                &PmInput {
                    weapon: 2,
                    attack: true,
                    ..Default::default()
                },
                1
            ),
            vec![EV_FIRE_WEAPON]
        );
    }

    /// The ladder forces weapon 0 in the pickup half too (1.8, dll
    /// 0x300107c0: the new weapon is `cmd.weapon` forced to 0 when
    /// `pm_flags & 0x10` is set). Without it a climber with nothing in hand
    /// raises whatever its cmd byte names and the next frame's ladder clause
    /// holsters it again, once per `raiseTime` for the whole climb.
    #[test]
    fn a_climber_with_nothing_in_hand_raises_nothing() {
        let (mut ps, w) = armed(&carbine());
        ps.weapon = 0;
        ps.on_ladder = true;
        let asks = PmInput {
            weapon: 1,
            ..Default::default()
        };
        assert_eq!(step(&mut ps, &w, &asks, 40), Vec::<i32>::new());
        assert_eq!(ps.weapon, 0);
        assert_eq!(ps.weaponstate, WEAPON_READY);
    }

    /// 1.10: the melee bit swings once per press -- the swipe, the hit event
    /// when `meleeDelay` runs out, and idle when `meleeTime` does.
    #[test]
    fn the_melee_bit_swings_once_per_press() {
        let (mut ps, w) = armed(&with_melee(carbine()));
        let held = PmInput {
            melee: true,
            ..Default::default()
        };
        let ev = step(&mut ps, &w, &held, 1);
        assert_eq!(ev, vec![EV_MELEE_SWIPE]);
        assert_eq!(ps.weaponstate, WEAPON_MELEE_WINDUP);
        assert_eq!(ps.weap_anim & 511, WEAP_MELEE_ATTACK);
        assert_eq!((ps.weapon_time_ms, ps.weapon_delay_ms), (660, 100));
        let ev = step(&mut ps, &w, &held, 2);
        assert_eq!(ev, vec![EV_FIRE_MELEE]);
        assert_eq!(ps.weaponstate, WEAPON_MELEE_RELAX);
        let ev = step(&mut ps, &w, &held, 12);
        assert_eq!(ev, Vec::<i32>::new());
        assert_eq!(ps.weaponstate, WEAPON_READY);
        // The relax clears the index and the toggle with it: the capture's
        // `melee_tap` reads `weapAnim` 520 through the swing and 0 after.
        assert_eq!(ps.weap_anim, WEAP_IDLE);
        assert_eq!(ps.ammoclip[1], 15);
    }

    /// A weapon with no `meleeDamage` ignores the bit.
    #[test]
    fn a_weapon_without_melee_damage_ignores_the_bit() {
        let (mut ps, w) = armed(&carbine());
        let ev = step(
            &mut ps,
            &w,
            &PmInput {
                melee: true,
                ..Default::default()
            },
            3,
        );
        assert_eq!(ev, Vec::<i32>::new());
        assert_eq!(ps.weaponstate, WEAPON_READY);
    }

    /// Held down, the carbine fires once: the latch pins `weaponTime` at 1 and
    /// the fire path never sees the 0 it needs (section 1.4).
    #[test]
    fn a_held_attack_fires_a_semi_auto_once() {
        let (mut ps, w) = armed(&carbine());
        let events = run(&mut ps, &w, true, 20);
        assert_eq!(events.iter().filter(|e| **e == EV_FIRE_WEAPON).count(), 1);
        assert_eq!(ps.ammoclip[1], 14);
        assert_eq!(ps.weapon_time_ms, 1);
    }

    /// Two frames on, six off: one shot per pulse, the capture's cadence.
    #[test]
    fn a_pulsed_attack_fires_every_pulse() {
        let (mut ps, w) = armed(&carbine());
        let mut shots = 0;
        for _ in 0..6 {
            shots += run(&mut ps, &w, true, 2)
                .iter()
                .filter(|e| **e == EV_FIRE_WEAPON)
                .count();
            run(&mut ps, &w, false, 6);
        }
        assert_eq!(shots, 6);
        assert_eq!(ps.ammoclip[1], 9);
    }

    /// The carbine reads `weaponstate` 3 for `fireTime` then ready again:
    /// 135 ms is under three 50 ms frames, and the capture ran 3 samples.
    #[test]
    fn firing_runs_for_fire_time() {
        let (mut ps, w) = armed(&carbine());
        run(&mut ps, &w, true, 1);
        assert_eq!(ps.weaponstate, WEAPON_FIRING);
        run(&mut ps, &w, false, 2);
        assert_eq!(ps.weaponstate, WEAPON_FIRING);
        run(&mut ps, &w, false, 1);
        assert_eq!(ps.weaponstate, WEAPON_READY);
    }

    /// A mosin shot is 159, then 162 with `weaponstate` 4 for `rechamberTime`,
    /// then 163 when the bolt closes (pavlov's three events per shot).
    #[test]
    fn a_bolt_action_rechambers_after_every_shot() {
        let (mut ps, w) = armed(&mosin());
        let mut events = run(&mut ps, &w, true, 1);
        events.extend(run(&mut ps, &w, false, 40));
        assert_eq!(
            events,
            vec![EV_FIRE_WEAPON, EV_RECHAMBER_WEAPON, EV_EJECT_BRASS]
        );
        assert_eq!(ps.weaponstate, WEAPON_READY);
        assert_eq!(ps.weapon_rechamber, 0);
    }

    /// The last round raises 161, and a dry clip with reserve reloads with no
    /// key at all (section 1.7's keyless clause).
    #[test]
    fn the_last_round_is_lastshot_and_the_dry_clip_reloads_itself() {
        let (mut ps, w) = armed(&carbine());
        ps.ammoclip[1] = 1;
        let mut events = run(&mut ps, &w, true, 1);
        assert_eq!(events, vec![EV_FIRE_WEAPON_LASTSHOT]);
        events.extend(run(&mut ps, &w, false, 4));
        assert_eq!(events[1], EV_RELOAD_FROM_EMPTY);
        assert_eq!(ps.weaponstate, WEAPON_RELOADING);
        run(&mut ps, &w, false, 60);
        assert_eq!(ps.weaponstate, WEAPON_READY);
        assert_eq!(ps.ammoclip[1], 15);
        assert_eq!(ps.ammo[1], 15);
    }

    /// The reload key on a partly full clip is an ordinary reload, 151, since
    /// neither fixture weapon sets `noPartialReload` (section 1.7).
    #[test]
    fn the_reload_key_on_a_partial_clip_raises_reload() {
        let (mut ps, w) = armed(&carbine());
        ps.ammoclip[1] = 10;
        let input = PmInput {
            reload: true,
            ..Default::default()
        };
        let events = step(&mut ps, &w, &input, 1);
        assert_eq!(events, vec![EV_RELOAD]);
        assert_eq!(ps.weaponstate, WEAPON_RELOADING);
        run(&mut ps, &w, false, 60);
        assert_eq!(ps.ammoclip[1], 15);
        assert_eq!(ps.ammo[1], 25);
    }

    /// `noPartialReload` with a `reloadAmmoAdd` at or above the clip size
    /// refuses every reload but the one from empty (section 1.7).
    #[test]
    fn no_partial_reload_refuses_a_partly_full_clip() {
        let mut d = carbine();
        d.no_partial_reload = true;
        let (mut ps, w) = armed(&d);
        ps.ammoclip[1] = 10;
        let input = PmInput {
            reload: true,
            ..Default::default()
        };
        assert!(step(&mut ps, &w, &input, 1).is_empty());
        ps.ammoclip[1] = 0;
        assert_eq!(step(&mut ps, &w, &input, 1), vec![EV_RELOAD_FROM_EMPTY]);
    }

    /// No reserve: the tap clicks (149) and costs half a second (section 1.5).
    #[test]
    fn an_empty_weapon_with_no_reserve_clicks() {
        let (mut ps, w) = armed(&carbine());
        ps.ammoclip[1] = 0;
        ps.ammo[1] = 0;
        let e = run(&mut ps, &w, true, 1);
        assert_eq!(e, vec![EV_NOAMMO]);
        assert_eq!(ps.weaponstate, WEAPON_READY);
        assert_eq!(ps.weapon_time_ms, 500);
    }

    /// `weaponDelay` skips the shot outright even with `weaponTime` at 0
    /// (section 1.5, step 3).
    #[test]
    fn a_weapon_delay_holds_the_next_shot_off() {
        let mut d = def(0.0, 0.0, 2.0, 0.5, 15, false);
        d.semi_auto = false;
        d.fire_delay = 0.2;
        let (mut ps, w) = armed(&d);
        let events = run(&mut ps, &w, true, 4);
        assert_eq!(events, vec![EV_FIRE_WEAPON], "one shot inside fireDelay");
        assert_eq!(ps.ammoclip[1], 14);
        // The delay expires on the fifth frame and the shot runs again.
        assert_eq!(run(&mut ps, &w, true, 1), vec![EV_FIRE_WEAPON]);
    }

    /// The ramp rate is the file's: 300 ms up and 400 ms down on the carbine,
    /// at `pml.msec / adsTransInTime` a frame, clamped at both ends rather
    /// than overshooting (combat doc, 1.13).
    #[test]
    fn the_ads_fraction_ramps_at_the_weapon_files_rate() {
        let (mut ps, w) = armed(&carbine_ads());
        // 300 ms in at 50 ms a frame: six frames to the sight.
        step(&mut ps, &w, &held(true), 3);
        assert!(
            (ps.weapon_pos_frac - 0.5).abs() < 1e-5,
            "{}",
            ps.weapon_pos_frac
        );
        step(&mut ps, &w, &held(true), 3);
        assert_eq!(ps.weapon_pos_frac, 1.0);
        step(&mut ps, &w, &held(true), 4);
        assert_eq!(ps.weapon_pos_frac, 1.0, "held at the sight, not past it");
        // 400 ms out: eight frames back to the hip.
        step(&mut ps, &w, &held(false), 4);
        assert!(
            (ps.weapon_pos_frac - 0.5).abs() < 1e-5,
            "{}",
            ps.weapon_pos_frac
        );
        step(&mut ps, &w, &held(false), 4);
        assert_eq!(ps.weapon_pos_frac, 0.0);
        step(&mut ps, &w, &held(false), 2);
        assert_eq!(ps.weapon_pos_frac, 0.0, "held at the hip, not below it");
    }

    /// A file with no transition times falls back to 300 ms in and 500 ms
    /// out, the two reciprocals `BG_SetupWeaponInfo` derives at load.
    #[test]
    fn a_weapon_with_no_transition_times_takes_the_retail_defaults() {
        let mut d = carbine_ads();
        d.ads_trans_in = 0.0;
        d.ads_trans_out = 0.0;
        let (mut ps, w) = armed(&d);
        step(&mut ps, &w, &held(true), 6);
        assert_eq!(ps.weapon_pos_frac, 1.0, "300 ms up");
        step(&mut ps, &w, &held(false), 9);
        assert!(
            (ps.weapon_pos_frac - 0.1).abs() < 1e-5,
            "{}",
            ps.weapon_pos_frac
        );
        step(&mut ps, &w, &held(false), 1);
        assert_eq!(ps.weapon_pos_frac, 0.0, "500 ms down");
    }

    /// A weapon with `aimDownSight 0` -- the grenades and the mounted MGs --
    /// holds the fraction at 0 whatever the button does.
    #[test]
    fn a_weapon_with_no_sight_never_leaves_the_hip() {
        let mut d = carbine_ads();
        d.aim_down_sight = false;
        let (mut ps, w) = armed(&d);
        step(&mut ps, &w, &held(true), 10);
        assert_eq!(ps.weapon_pos_frac, 0.0);
        assert!(!ps.ads_active, "and the flag never takes either");
    }

    /// The sight stays down for all of a reload but its last
    /// `adsReloadTransTime`: 600 ms of the carbine's 2650, so the fraction
    /// leaves 0 only once `weaponTime` is inside that window.
    #[test]
    fn the_reload_pins_the_sight_down_until_its_last_ads_reload_trans_time() {
        let (mut ps, w) = armed(&carbine_ads());
        ps.ammoclip[1] = 10;
        let reload = PmInput {
            reload: true,
            ..Default::default()
        };
        assert_eq!(step(&mut ps, &w, &reload, 1), vec![EV_RELOAD]);
        assert_eq!(ps.weapon_time_ms, 2650);
        // Forty frames on, 650 ms left: still outside the window.
        let ads = held(true);
        step(&mut ps, &w, &ads, 40);
        assert_eq!(ps.weapon_time_ms, 650);
        assert_eq!(ps.weapon_pos_frac, 0.0);
        assert!(ps.ads_active, "the flag is set for the whole reload");
        // The next frame reaches 600 and the sight starts up.
        step(&mut ps, &w, &ads, 1);
        assert_eq!(ps.weapon_time_ms, 600);
        assert!(ps.weapon_pos_frac > 0.0);
    }

    /// Off the ground the flag drops, so the fraction ramps back down for the
    /// whole jump and starts again on landing.
    #[test]
    fn an_airborne_player_loses_the_sight() {
        let (mut ps, w) = armed(&carbine_ads());
        step(&mut ps, &w, &held(true), 6);
        assert_eq!(ps.weapon_pos_frac, 1.0);
        ps.on_ground = false;
        step(&mut ps, &w, &held(true), 1);
        assert!(!ps.ads_active);
        assert!(
            (ps.weapon_pos_frac - 0.875).abs() < 1e-5,
            "{}",
            ps.weapon_pos_frac
        );
        ps.on_ground = true;
        step(&mut ps, &w, &held(true), 1);
        assert!(ps.ads_active);
        assert!(ps.weapon_pos_frac > 0.875);
    }

    /// A raise, a holster and a melee each drop the flag while they last.
    #[test]
    fn a_busy_weapon_refuses_the_sight() {
        let (mut ps, w) = armed(&carbine_ads());
        for state in [
            WEAPON_RAISING,
            WEAPON_DROPPING,
            WEAPON_MELEE_WINDUP,
            WEAPON_MELEE_RELAX,
        ] {
            ps.weaponstate = state;
            ps.ads_active = true;
            let def = weapon_def(&w, ps.weapon).cloned();
            update_ads_flag(&mut ps, &held(true), def.as_ref());
            assert!(!ps.ads_active, "weaponstate {state}");
        }
    }

    /// The prone arm holds the flag where it is rather than setting it, for
    /// as long as the sight was held on the previous cmd too and the body is
    /// crawling. So a flag something else cleared does not come back until
    /// the crawl stops.
    #[test]
    fn a_crawling_prone_player_does_not_take_the_flag_back() {
        let (mut ps, w) = armed(&carbine_ads());
        ps.stance = Stance::Prone;
        let crawl = PmInput {
            ads: true,
            forward: 1.0,
            ..Default::default()
        };
        // The frame the sight goes down on is a set: the previous cmd held
        // no sight, so the hold does not apply to it.
        step(&mut ps, &w, &crawl, 1);
        assert!(ps.ads_active);
        // A gap in the flag -- a jump, a weapon raise -- is not recovered
        // while it crawls with the sight already held.
        ps.ads_active = false;
        step(&mut ps, &w, &crawl, 3);
        assert!(!ps.ads_active);
        // Stopping restores it.
        step(&mut ps, &w, &held(true), 1);
        assert!(ps.ads_active);
    }

    /// `pm_flags` 0x80 rides on 0x20 but drops for a prone player and for the
    /// whole of a reload, which 0x20 sits through.
    #[test]
    fn the_ads_walk_bit_drops_where_retails_does() {
        let (mut ps, w) = armed(&carbine_ads());
        step(&mut ps, &w, &held(true), 6);
        assert_eq!(ads_pm_flags(&ps), PMF_ADS | PMF_ADS_WALK);
        ps.weaponstate = WEAPON_RELOADING;
        assert_eq!(ads_pm_flags(&ps), PMF_ADS);
        ps.weaponstate = WEAPON_READY;
        ps.stance = Stance::Prone;
        assert_eq!(ads_pm_flags(&ps), PMF_ADS);
        ps.ads_active = false;
        assert_eq!(ads_pm_flags(&ps), 0);
    }

    /// A held movement axis opens the cone faster than the carbine decays it
    /// (8 against 4), so it saturates and stays there; standing still it
    /// empties at `hipSpreadDecayRate * frametime * 255`, 51 a frame.
    #[test]
    fn the_move_term_saturates_the_spread_and_standing_still_empties_it() {
        let (mut ps, w) = armed(&carbine_ads());
        let walk = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        // (8 - 4) * 0.05 * 255 = 51 on the first frame.
        step(&mut ps, &w, &walk, 1);
        assert!(
            (ps.aim_spread_scale - 51.0).abs() < 1e-3,
            "{}",
            ps.aim_spread_scale
        );
        step(&mut ps, &w, &walk, 20);
        assert_eq!(ps.aim_spread_scale, 255.0);
        step(&mut ps, &w, &PmInput::default(), 1);
        assert!(
            (ps.aim_spread_scale - 204.0).abs() < 1e-3,
            "{}",
            ps.aim_spread_scale
        );
        step(&mut ps, &w, &PmInput::default(), 4);
        assert_eq!(ps.aim_spread_scale, 0.0);
    }

    /// The stance multipliers scale the decay: the carbine's prone 1.1
    /// against its ducked 1.0. Airborne halves it and adds 2.56 on top, which
    /// is the only way the counter climbs with no input at all.
    #[test]
    fn the_stance_and_airborne_terms_scale_the_decay() {
        let (mut ps, w) = armed(&carbine_ads());
        let idle = PmInput::default();
        for (stance, rate) in [(Stance::Crouch, 4.0 * 1.0), (Stance::Prone, 4.0 * 1.1)] {
            ps.stance = stance;
            ps.aim_spread_scale = 255.0;
            step(&mut ps, &w, &idle, 1);
            let want = 255.0 - rate * 0.05 * 255.0;
            assert!(
                (ps.aim_spread_scale - want).abs() < 1e-3,
                "{stance:?}: {}",
                ps.aim_spread_scale
            );
        }
        // Airborne: (2.56 - 4 * 0.5) * 0.05 * 255 = +7.14 a frame from empty.
        ps.stance = Stance::Stand;
        ps.on_ground = false;
        ps.aim_spread_scale = 0.0;
        step(&mut ps, &w, &idle, 1);
        assert!(
            (ps.aim_spread_scale - 7.14).abs() < 1e-3,
            "{}",
            ps.aim_spread_scale
        );
    }

    /// The turn term is the usercmd angle delta, frame-rate independent. It
    /// is the one term the two fixture weapons cannot show: both spell
    /// `hipSpreadTurnAdd 0`, and the BAR is the only stock MP file that does
    /// not.
    #[test]
    fn the_turn_term_reads_the_usercmd_angle_delta() {
        let mut d = carbine_ads();
        d.hip_spread_turn_add = 0.8;
        let (mut ps, w) = armed(&d);
        // 90 degrees of yaw in one frame: 90 * 0.01 * 0.8 * 255 = 183.6 in,
        // against the frame's 51 of decay.
        let turn = PmInput {
            angles: [0, 16384],
            ..Default::default()
        };
        step(&mut ps, &w, &turn, 1);
        assert!(
            (ps.aim_spread_scale - 132.6).abs() < 0.1,
            "{}",
            ps.aim_spread_scale
        );
        // The same angle held is no turn at all: the delta is against the
        // previous cmd, not against zero.
        step(&mut ps, &w, &turn, 1);
        assert!(
            (ps.aim_spread_scale - 81.6).abs() < 0.1,
            "{}",
            ps.aim_spread_scale
        );
    }

    /// A settled sight takes none of the additions and all of the decay, so
    /// walking with the sight up still closes the cone.
    #[test]
    fn a_settled_sight_skips_the_additions_but_not_the_decay() {
        let (mut ps, w) = armed(&carbine_ads());
        let walk_ads = PmInput {
            forward: 1.0,
            ads: true,
            ..Default::default()
        };
        step(&mut ps, &w, &walk_ads, 6);
        assert_eq!(ps.weapon_pos_frac, 1.0);
        ps.aim_spread_scale = 255.0;
        step(&mut ps, &w, &walk_ads, 1);
        assert!(
            (ps.aim_spread_scale - 204.0).abs() < 1e-3,
            "{}",
            ps.aim_spread_scale
        );
    }

    /// A weapon with no decay rate at all -- and a weapon index with no file
    /// behind it -- empties the counter in one frame, which is what retail's
    /// `decay = 1.0` with no frame-time scaling does.
    #[test]
    fn a_weapon_with_no_decay_rate_slams_the_spread_shut() {
        let mut d = carbine_ads();
        d.hip_spread_decay_rate = 0.0;
        let (mut ps, w) = armed(&d);
        ps.aim_spread_scale = 255.0;
        step(&mut ps, &w, &PmInput::default(), 1);
        assert_eq!(ps.aim_spread_scale, 0.0);
        ps.aim_spread_scale = 255.0;
        adjust_aim_spread_scale(&mut ps, &PmInput::default(), None, 0.05);
        assert_eq!(ps.aim_spread_scale, 0.0);
    }

    /// The shot's own add is `hipSpreadFireAdd * 255`, 178.5 on the carbine,
    /// and it is skipped from a settled sight (section 1.5, step 8).
    #[test]
    fn a_shot_adds_its_fire_add_unless_the_sight_is_up() {
        let (mut ps, w) = armed(&carbine_ads());
        // The decay runs first and takes an empty counter nowhere, so the
        // shot's frame reads the add alone; the next one is 51 lower.
        run(&mut ps, &w, true, 1);
        assert!(
            (ps.aim_spread_scale - 178.5).abs() < 0.1,
            "{}",
            ps.aim_spread_scale
        );
        run(&mut ps, &w, false, 1);
        assert!(
            (ps.aim_spread_scale - 127.5).abs() < 0.1,
            "{}",
            ps.aim_spread_scale
        );
        let (mut ps, w) = armed(&carbine_ads());
        step(&mut ps, &w, &held(true), 6);
        assert_eq!(ps.weapon_pos_frac, 1.0);
        let ads_fire = PmInput {
            attack: true,
            ads: true,
            ..Default::default()
        };
        step(&mut ps, &w, &ads_fire, 1);
        assert_eq!(ps.aim_spread_scale, 0.0, "no add from the sight");
        assert_eq!(ps.weap_anim & !ANIM_TOGGLEBIT, WEAP_ADS_ATTACK);
    }

    /// Every `weapAnim` write flips the toggle, `WEAP_IDLE` included, so each
    /// transition is a different word from the one before it. At 50 ms frames
    /// the shot's anim is always cleared before the next tap can fire, so
    /// there is no pair of consecutive shot writes here to compare; the
    /// capture's sustained fire, which has them, sees the same flip.
    #[test]
    fn every_weapon_anim_write_changes_the_word() {
        let (mut ps, w) = armed(&carbine());
        run(&mut ps, &w, true, 1);
        assert_eq!(ps.weap_anim, WEAP_ATTACK | ANIM_TOGGLEBIT);
        run(&mut ps, &w, false, 3);
        assert_eq!(ps.weap_anim, WEAP_IDLE, "clearing to idle flips it back");
        run(&mut ps, &w, false, 3);
        run(&mut ps, &w, true, 1);
        assert_eq!(ps.weap_anim, WEAP_ATTACK | ANIM_TOGGLEBIT);
    }

    /// A ladder forces weapon 0 through the putaway, and the weapon comes back
    /// through a raise at the top (section 1.8).
    #[test]
    fn the_ladder_holsters_the_weapon_and_gives_it_back() {
        let (mut ps, w) = armed(&carbine());
        ps.on_ladder = true;
        assert_eq!(run(&mut ps, &w, false, 1), vec![EV_PUTAWAY_WEAPON]);
        assert_eq!(ps.weaponstate, WEAPON_DROPPING);
        run(&mut ps, &w, false, 14);
        assert_eq!(ps.weapon, 0, "0.67 s of dropTime and the hands are empty");
        assert_eq!(ps.weaponstate, WEAPON_READY);
        // Nothing happens for as long as the climb lasts.
        assert!(run(&mut ps, &w, false, 20).is_empty());
        ps.on_ladder = false;
        assert_eq!(run(&mut ps, &w, false, 1), vec![EV_RAISE_WEAPON]);
        assert_eq!(ps.weapon, 1);
        assert_eq!(ps.weaponstate, WEAPON_RAISING);
    }

    /// A weapon index the table has no def for must not freeze the timers.
    #[test]
    fn the_timers_run_for_a_weapon_with_no_def() {
        let (mut ps, w) = armed(&carbine());
        ps.weapon = 3;
        ps.weapon_time_ms = 200;
        ps.weapon_delay_ms = 120;
        run(&mut ps, &w, false, 1);
        assert_eq!((ps.weapon_time_ms, ps.weapon_delay_ms), (150, 70));
    }

    /// The usercmd weapon byte switches: putaway, raise, then ready on the new
    /// weapon (section 1.8). Retail's cmd carries the byte every frame.
    #[test]
    fn a_weapon_request_switches_through_putaway_and_raise() {
        let (mut ps, mut w) = armed(&carbine());
        w.push(Some(def(0.2, 0.0, 1.0, 0.3, 8, false)));
        give(&mut ps, 2, 3);
        let input = PmInput {
            weapon: 2,
            ..Default::default()
        };
        let events = step(&mut ps, &w, &input, 1);
        assert_eq!(events, vec![EV_PUTAWAY_WEAPON]);
        assert_eq!(ps.weaponstate, WEAPON_DROPPING);
        assert_eq!(ps.pending_weapon, 2);

        let events = step(&mut ps, &w, &input, 14);
        assert_eq!(events, vec![EV_RAISE_WEAPON]);
        assert_eq!(ps.weapon, 2);
        assert_eq!(ps.weaponstate, WEAPON_RAISING);

        // The raise runs for `raiseTime` and not for a frame: the capture's
        // `to_frag` holds `weaponstate` 1 for 295 ms of the frag's 250.
        step(&mut ps, &w, &input, 9);
        assert_eq!(ps.weaponstate, WEAPON_RAISING);
        step(&mut ps, &w, &input, 1);
        assert_eq!(ps.weaponstate, WEAPON_READY);
        assert_eq!(ps.weap_anim & !ANIM_TOGGLEBIT, WEAP_IDLE);
    }

    /// A segmented reload (enfield, kar98k_sniper, springfield) runs
    /// start, one segment per `reloadAmmoAdd` rounds, then end (section 1.7).
    #[test]
    fn a_segmented_reload_loads_one_segment_at_a_time() {
        let mut d = def(0.33, 0.95, 0.6, 0.35, 5, true);
        d.segmented_reload = true;
        d.no_partial_reload = true;
        d.reload_ammo_add = 1;
        d.reload_start_add = 1;
        d.reload_add_time = 0.2;
        d.reload_start_time = 1.8;
        d.reload_start_add_time = 1.3;
        d.reload_end_time = 0.77;
        let (mut ps, w) = armed(&d);
        ps.ammoclip[1] = 2;
        let input = PmInput {
            reload: true,
            ..Default::default()
        };
        let events = step(&mut ps, &w, &input, 1);
        assert_eq!(events, vec![EV_RELOAD_START]);
        assert_eq!(ps.weaponstate, WEAPON_RELOAD_START);
        let events = step(&mut ps, &w, &PmInput::default(), 120);
        assert_eq!(
            events.iter().filter(|e| **e == EV_RELOAD).count(),
            2,
            "three rounds missing, and the start state's own add-time edge \
             loads the first of them"
        );
        assert_eq!(events.last(), Some(&EV_RELOAD_END));
        assert_eq!(ps.weaponstate, WEAPON_READY);
        assert_eq!(ps.ammoclip[1], 5);
        assert_eq!(ps.ammo[1], 27);
    }

    /// The start segment's own add can fill the clip, and then no loop
    /// segment runs: the retail `kar98k_sniper_mp` capture reads
    /// `weaponstate` 7 then 9, never 5, after a one-round reload
    /// (cod11-combat.md, 9.2).
    #[test]
    fn a_start_segment_that_fills_the_clip_skips_the_loop() {
        let mut d = def(0.33, 1.0, 0.6, 0.35, 5, true);
        d.segmented_reload = true;
        d.no_partial_reload = true;
        d.reload_ammo_add = 1;
        d.reload_start_add = 1;
        d.reload_add_time = 0.2;
        d.reload_start_time = 1.8;
        d.reload_start_add_time = 1.4;
        d.reload_end_time = 0.77;
        let (mut ps, w) = armed(&d);
        ps.ammoclip[1] = 4;
        let input = PmInput {
            reload: true,
            ..Default::default()
        };
        let events = step(&mut ps, &w, &input, 1);
        assert_eq!(events, vec![EV_RELOAD_START]);
        let mut states = Vec::new();
        let mut events = Vec::new();
        for _ in 0..60 {
            events.extend(step(&mut ps, &w, &PmInput::default(), 50));
            states.push(ps.weaponstate);
        }
        assert_eq!(events, vec![EV_RELOAD_END]);
        assert!(!states.contains(&WEAPON_RELOADING), "{states:?}");
        assert!(states.contains(&WEAPON_RELOAD_END), "{states:?}");
        assert_eq!(ps.weaponstate, WEAPON_READY);
        assert_eq!(ps.ammoclip[1], 5);
    }

    /// The attack bit cuts a segmented reload short at the end of its segment.
    #[test]
    fn the_attack_bit_interrupts_a_segmented_reload() {
        let mut d = def(0.33, 0.95, 0.6, 0.35, 5, false);
        d.segmented_reload = true;
        d.reload_ammo_add = 1;
        d.reload_add_time = 0.2;
        d.reload_end_time = 0.77;
        let (mut ps, w) = armed(&d);
        ps.ammoclip[1] = 0;
        step(&mut ps, &w, &PmInput::default(), 1);
        assert_eq!(ps.weaponstate, WEAPON_RELOADING);
        let input = PmInput {
            attack: true,
            ..Default::default()
        };
        step(&mut ps, &w, &input, 1);
        assert_eq!(ps.weaponstate, WEAPON_RELOADING_INTERUPT);
        step(&mut ps, &w, &PmInput::default(), 40);
        assert_eq!(ps.weaponstate, WEAPON_READY);
        assert_eq!(ps.ammoclip[1], 1, "the segment in flight still landed");
    }

    #[test]
    fn slots_pack_into_the_two_wire_words() {
        let mut ps = PlayerState::spawn(glam::Vec3::ZERO, 0.0);
        give(&mut ps, 3, 1);
        give(&mut ps, 9, 5);
        assert!(holds(&ps, 3) && holds(&ps, 9) && !holds(&ps, 4));
        assert_eq!(slot_words(&ps), [3 << 8, 9 << 8]);
    }
}
