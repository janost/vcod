//! The weapon half of pmove: retail's `PM_Weapon`
//! (`game.mp.i386.so` 0x390e0, `cgame_mp_x86.dll` 0x30011ab0), read out in
//! docs/research/cod11-combat.md section 1. Section numbers below are that
//! document's; the behaviour the two combat captures measured is in
//! docs/research/player-model-anim-system.md, "The weapon channel".
//!
//! Not modelled yet: retail's `BG_TakePlayerWeapon` on a spent `clipOnly`
//! weapon (the host owns the held bits, so pmove cannot clear one), and the
//! three stops of 1.12, whose `pm_flags`/`pm_type`/`eFlags` this
//! `PlayerState` does not carry.
//!
//! `cmd.weapon` is read as retail reads it: a 0 under a held weapon is a
//! request to holster (1.8), so every caller sends the weapon it holds or
//! selects, never a neutral 0.

use super::{MoveRules, PlayerState, PmEvent, PmInput, Stance};
use crate::net::event_ids::{
    EV_UO_BREAKDOWN_WEAPON, EV_UO_DEPLOY_WEAPON, EV_UO_GRENADE_SPOON, EV_UO_GRENADE_SUICIDE,
    EV_UO_REMOVE_WEAPON_ATTACHMENTS,
};
use crate::net::flags::ANIM_TOGGLEBIT;
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
/// United Offensive's bipod going down and coming up (`weaponstate` 12 and
/// 13, docs/research/coduo-movement.md 7).
pub const WEAPON_DEPLOY: u8 = 12;
pub const WEAPON_BREAKDOWN: u8 = 13;

/// The wire `EV_*` ids pmove raises. `EV_EMPTYCLIP` (150) is not among them:
/// docs/research/cod11-events-and-fx.md section 1.5 found no site that raises it.
pub use crate::net::event_ids::{
    EV_EJECT_BRASS, EV_FIRE_MELEE, EV_FIRE_WEAPON, EV_FIRE_WEAPON_LASTSHOT, EV_MELEE_SWIPE,
    EV_NOAMMO, EV_PULLBACK_WEAPON, EV_PUTAWAY_WEAPON, EV_RAISE_WEAPON, EV_RECHAMBER_WEAPON,
    EV_RELOAD, EV_RELOAD_END, EV_RELOAD_FROM_EMPTY, EV_RELOAD_START, EV_WEAPON_ALT,
};

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
pub const WEAP_ALTSWITCHFROM: i32 = 15;
pub const WEAP_ALTSWITCHTO: i32 = 16;
/// The pullback has no name in the binary's printer: it is index 17, past the
/// seventeen the switch there covers (section 1.11).
pub const WEAP_GRENADE_PULLBACK: i32 = 17;
/// UO's `WEAP_DEPLOY` and `WEAP_BREAKDOWN` (0x9d81c).
pub const WEAP_DEPLOY: i32 = 19;
pub const WEAP_BREAKDOWN: i32 = 20;

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
    crate::weapon::stored_ms(seconds)
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
    if ms > 0.0 { ms } else { fallback }
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
        // A link passes for ground here (0x3727e), though its arm clears it.
        && (ps.sight_ground() || ps.linked);
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
    // 1.5 lowers the sight through a rechamber the weapon file forbids it
    // in (0x2d2f2).
    let rechambering =
        ps.rules.v15() && def.rechamber_drops_ads && ps.weaponstate == WEAPON_RECHAMBERING;
    // UO keeps a deployed LMG's sight up through its reload (0x3349e).
    let reloading = reloading && !(ps.rules == MoveRules::Uo && def.is_lmg());
    let want = (ps.ads_active && !reloading && !rechambering)
        || (def.ads_fire && ps.weapon_delay_ms != 0 && ps.weaponstate == WEAPON_FIRING);
    let step = dt_ms as f32
        / if want {
            trans_ms(def.ads_trans_in, ADS_TRANS_IN_DEFAULT_MS)
        } else {
            -trans_ms(def.ads_trans_out, ADS_TRANS_OUT_DEFAULT_MS)
        };
    ps.weapon_pos_frac = (ps.weapon_pos_frac + step).clamp(0.0, 1.0);
}

/// UO's `PM_UpdateAimDownSightLerp` (game 0x3349e, cgame 0x30011f50) in
/// a seat that fires no weapon of its own: the button alone wants the
/// sight, and the driver's seat snaps to it where the others ramp at the
/// held weapon's rates (coduo-vehicles.md 3).
pub fn seat_ads_lerp(
    ps: &mut PlayerState,
    seat: super::VehicleSeat,
    def: Option<&WeaponDef>,
    dt_ms: i32,
) {
    let want = ps.ads_active;
    if seat.pos == 1 {
        ps.weapon_pos_frac = if want { 1.0 } else { 0.0 };
        return;
    }
    let Some(def) = def else {
        return;
    };
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
    // once the swing is over. `PM_Weapon`'s store (0x39254,
    // `open_cone_on_prone_move`) puts it back every frame of the swing.
    if matches!(ps.weaponstate, WEAPON_MELEE_WINDUP | WEAPON_MELEE_RELAX) {
        ps.aim_spread_scale = 255.0;
        return;
    }
    let (add, decay) = match def {
        Some(def) if def.hip_spread_decay_rate != 0.0 => {
            let mut decay = def.hip_spread_decay_rate;
            // The three stance arms are exclusive and airborne wins; a link
            // is not airborne (0x38622). The stance is the `eFlags` one
            // (1.1d ps+0x80 at 0x38632, UO ps+0x84 at 0x35e56), so on 1.5 and UO a
            // body standing up from prone decays at the crouch rate until
            // its eye has risen.
            let airborne = !ps.on_ground && !ps.linked;
            let stance = ps.eflags_stance();
            if airborne {
                decay *= 0.5;
            } else if stance == Stance::Prone {
                decay *= def.hip_spread_prone_decay;
            } else if stance == Stance::Crouch {
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
                if airborne {
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

/// What `BG_GetMinSpreadForWeapon` reads off a playerstate: the stance flags
/// and the eye's leg, the leg timed against the caller's clock. The default
/// is a settled stand.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpreadStance {
    /// `pm_flags` 0x1 and 0x2.
    pub prone: bool,
    pub ducked: bool,
    /// `viewHeightCurrent`, `viewHeightLerpTarget`, `viewHeightLerpDown`.
    pub view_height: f32,
    pub lerp_target: f32,
    pub lerp_down: bool,
    /// `pm_flags` 0x4, which shortens the dive's legs.
    pub dive: bool,
    /// `time - viewHeightLerpTime`; `None` while `viewHeightLerpTime` is 0.
    pub into_leg_ms: Option<i32>,
}

impl SpreadStance {
    /// `ps` as it stands at `command_time`, read at `time`.
    pub fn of(ps: &PlayerState, command_time: i32, time: i32) -> Self {
        SpreadStance {
            prone: ps.stance == Stance::Prone,
            ducked: ps.ducked,
            view_height: ps.view_height_cur,
            lerp_target: ps.view_lerp_target,
            lerp_down: ps.view_lerp_down,
            dive: ps.prone_dive,
            into_leg_ms: ps
                .view_lerp_ms
                .map(|_| time - ps.view_lerp_stamp(command_time)),
        }
    }
}

/// `BG_GetMinSpreadForWeapon` (`game.mp.i386.so` 0x37114, dll 0x3000fa50),
/// combat doc 2.1: the hip cone's minimum, blended linearly in time between
/// the two ends of a running leg of the eye, not along its curve.
pub fn hip_spread_min(def: &WeaponDef, s: &SpreadStance) -> f32 {
    let (stand, ducked, prone) = (
        def.hip_spread_stand_min,
        def.hip_spread_ducked_min,
        def.hip_spread_prone_min,
    );
    let into = match s.into_leg_ms {
        Some(ms) if s.view_height != s.lerp_target => ms,
        _ if s.prone => return prone,
        _ if s.ducked => return ducked,
        _ => return stand,
    };
    let length = super::view_lerp_length(s.lerp_target, s.lerp_down, s.dive);
    let f = (f64::from(into) / f64::from(length)).clamp(0.0, 1.0);
    let (from, to) = if s.lerp_target == super::VIEW_PRONE {
        (ducked, prone)
    } else if s.lerp_target == super::VIEW_STAND {
        (ducked, stand)
    } else if s.lerp_down {
        (stand, ducked)
    } else {
        (prone, ducked)
    };
    (f64::from(from) + (f64::from(to) - f64::from(from)) * f) as f32
}

/// One frame of the weapon machine. `dt_ms` is retail's `pml.msec`.
pub fn pm_weapon(
    ps: &mut PlayerState,
    input: &PmInput,
    weapons: &[Option<WeaponDef>],
    dt_ms: i32,
    events: &mut Vec<PmEvent>,
) {
    // Both `weapAnim` setters write nothing on a cmd whose weapon byte is 0
    // (1.1 dll 0x3000ffe0 and 0x30010010, 1.5b 0x2d9b8 and 0x2da19, UO
    // 0x33f9c and 0x34024; combat doc 1.2): a raise of empty hands keeps
    // `WEAP_RAISE` past its finish. UO's writes a `gas` weapon's index
    // bare, the toggle cleared.
    let anim = ps.weap_anim;
    let gas = ps.rules == MoveRules::Uo
        && weapon_def(weapons, ps.weapon).is_some_and(|d| d.weapon_type == WEAPON_TYPE_GAS);
    pm_weapon_step(ps, input, weapons, dt_ms, events);
    if input.weapon == 0 {
        ps.weap_anim = anim;
    } else if gas && ps.weap_anim != anim {
        ps.weap_anim &= !ANIM_TOGGLEBIT;
    }
}

fn pm_weapon_step(
    ps: &mut PlayerState,
    input: &PmInput,
    weapons: &[Option<WeaponDef>],
    dt_ms: i32,
    events: &mut Vec<PmEvent>,
) {
    // `PM_UpdateAimDownSightLerp` opens the machine, ahead of the grenade
    // and the timers (1.1 0x39143, 1.5 0x30467, UO `PM_Weapon`'s first
    // call), so it reads the timers and the state the frame starts with. The
    // fraction rides on the held weapon's own transition times, so it stands
    // still while the weapon has no def to read them from.
    if let Some(def) = weapon_def(weapons, ps.weapon) {
        advance_ads(ps, def, dt_ms);
        if cook(ps, input, def, dt_ms, events) {
            return;
        }
    }
    // The timers run whether or not the current weapon resolves: an index with
    // no def, weapon 0 on a ladder among them, must not freeze them.
    let delay_expired = advance_timers(ps, weapon_def(weapons, ps.weapon), input, dt_ms);
    if let Some(def) = weapon_def(weapons, ps.weapon) {
        deploy_driver(ps, def, events);
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
    let Some(def) = weapon_def(weapons, ps.weapon) else {
        // Any index the table cannot resolve, which with the stock table is
        // only weapon 0: nothing to read a melee, a reload or a shot from.
        // The switch path still runs, which is what lets a player whose last
        // grenade `BG_TakePlayerWeapon` took ask for another weapon and get
        // it (section 1.8, and 1.12 for the three stops that really do end
        // `PM_Weapon`). Weapon 0 takes `putaway`'s short branch, so the
        // pickup lands on this frame; a held index with no def would take the
        // ordinary path and write `WEAP_DROP` with a drop time of 0.
        if begin_change(ps, input, weapons, events)
            && ps.weaponstate == WEAPON_DROPPING
            && ps.weapon_time_ms == 0
        {
            pickup(ps, input, weapons, events);
        }
        open_cone_on_prone_move(ps, input);
        return;
    };
    melee_finish(ps, def, delay_expired, events);
    if melee_check(ps, input, def, delay_expired, events) {
        open_cone_on_prone_move(ps, input);
        return;
    }
    if begin_change(ps, input, weapons, events) {
        // The pickup half runs later in the same `PM_Weapon` (section 1.8), so
        // a putaway that left no drop time -- the cancelled pullback, a weapon
        // gone from under the player -- raises on this frame and never shows
        // `weaponstate` 2.
        if ps.weaponstate == WEAPON_DROPPING && ps.weapon_time_ms == 0 {
            pickup(ps, input, weapons, events);
        }
        open_cone_on_prone_move(ps, input);
        return;
    }
    if reload_check(ps, input, def, events) {
        open_cone_on_prone_move(ps, input);
        return;
    }
    if rechamber_check(ps, def, delay_expired, events) {
        return;
    }
    // A rechamber that starts or runs still opens the cone (0x375d0
    // returns 0 for both, combat doc 1.9).
    open_cone_on_prone_move(ps, input);
    if reload_machine(ps, def, delay_expired, events) {
        return;
    }
    // "finish a weapon change": the pickup half runs only in state 2.
    if ps.weaponstate == WEAPON_DROPPING {
        if ps.weapon_time_ms == 0 {
            pickup(ps, input, weapons, events);
        }
        return;
    }
    // Retail's gate (0x3925a): a running `weaponTime` or `weaponDelay` ends
    // the frame unless the delay ran out on it. The swing's and the bolt's
    // delay edges are spent above, so the one that gets past here is a shot
    // waiting out its delay (section 1.5).
    let shot_due = delay_expired && ps.weaponstate == WEAPON_FIRING;
    if ps.weapon_time_ms != 0 && !shot_due {
        return;
    }
    if grenade_hold(ps, input, def, events) {
        return;
    }
    if ps.weapon_delay_ms != 0 && !delay_expired {
        return;
    }
    // The melee finish's second arm (0x38f15): the swing relaxes into the
    // idle pose on the first frame both timers are out, and that frame
    // fires nothing (section 1.10).
    if ps.weaponstate == WEAPON_MELEE_RELAX {
        hold_anim(ps, WEAP_IDLE);
        ps.weaponstate = WEAPON_READY;
        return;
    }
    // UO's deploy and breakdown end once both timers are out (0x350c2,
    // 0x35120).
    if matches!(ps.weaponstate, WEAPON_DEPLOY | WEAPON_BREAKDOWN) {
        ps.weaponstate = WEAPON_READY;
        set_anim(ps, WEAP_IDLE);
        return;
    }
    // Releasing the trigger (section 1.6): whatever state got this far,
    // a rechamber the timer left standing among them, ends here.
    if !input.attack && !delay_expired {
        if ps.weaponstate == WEAPON_FIRING {
            hold_anim(ps, WEAP_IDLE);
        }
        ps.weaponstate = WEAPON_READY;
        return;
    }
    if !uo_may_fire(ps, def) {
        return;
    }
    fire(ps, def, events);
}

/// UO's grenade cook, `PM_Weapon` (0x3726c) ahead of its timers, for a
/// `cookOffHold` grenade: an edge of the sight button on a pulled grenade
/// whose fuse is still whole spoons it (10 ms off, `EV_GRENADE_SPOON`),
/// and from then the fuse runs by the frame. Under 51 ms the grenade goes
/// by itself with 50 left: the throw's event, `EV_GRENADE_SUICIDE` for one
/// with `explosionInnerDamage` (the server kills the thrower), a round
/// off the clip and 1600 ms of `weaponTime`. Returns whether that ended
/// the frame (coduo-movement.md, 12).
fn cook(
    ps: &mut PlayerState,
    input: &PmInput,
    def: &WeaponDef,
    dt_ms: i32,
    events: &mut Vec<PmEvent>,
) -> bool {
    if ps.rules != MoveRules::Uo || def.weapon_type != WEAPON_TYPE_GRENADE || !def.cook_off_hold {
        return false;
    }
    let fuse = ms(def.fuse_time);
    if input.ads != ps.last_cmd_ads && fuse <= ps.grenade_time_left_ms {
        ps.grenade_time_left_ms -= 10;
        push(events, EV_UO_GRENADE_SPOON);
    }
    if ps.grenade_time_left_ms <= 0 {
        return false;
    }
    if ps.grenade_time_left_ms < fuse {
        ps.grenade_time_left_ms -= dt_ms;
    }
    if ps.grenade_time_left_ms >= COOKED_OUT_MS {
        return false;
    }
    // The fuse rides the event's parm, as [`fire`]'s does.
    events.push(PmEvent {
        event: EV_FIRE_WEAPON,
        parm: 50,
    });
    ps.grenade_time_left_ms = 0;
    if def.explosion_inner_damage != 0 {
        push(events, EV_UO_GRENADE_SUICIDE);
    }
    let (ci, ai) = (def.clip_index, def.ammo_index);
    ps.ammoclip[ci] = ps.ammoclip[ci].saturating_sub(1);
    ps.weapon_delay_ms = 0;
    ps.weapon_time_ms = 1600;
    if def.clip_only && ps.ammoclip[ci] == 0 && ps.ammo[ai] == 0 {
        push(events, EV_NOAMMO);
    }
    true
}

/// A spooned grenade in hand, its fuse running: what
/// `PM_UpdatePlayerSprintingFlag` (0x2c74a) refuses a sprint on.
pub fn cooking(ps: &PlayerState, def: &WeaponDef) -> bool {
    def.cook_off_hold
        && ps.grenade_time_left_ms != 0
        && ps.grenade_time_left_ms < ms(def.fuse_time)
        && holds(ps, ps.weapon)
}

/// `grenadeTimeLeft` under which a cooked grenade goes by itself (`PM_Weapon`
/// compares against 0x33).
const COOKED_OUT_MS: i32 = 51;

/// UO's gate on the shot (0x36c0e): never while sprinting, a spotter only
/// down an `adsFire` sight, an LMG only deployed and settled.
fn uo_may_fire(ps: &PlayerState, def: &WeaponDef) -> bool {
    if ps.rules != MoveRules::Uo {
        return true;
    }
    !ps.sprinting
        && (def.weapon_class != "spotter" || ps.ads_active && def.ads_fire)
        && (!def.is_lmg() || ps.ads_active && ps.weapon_pos_frac >= LMG_SETTLED)
}

/// The fraction a deployed LMG fires and reloads from (UO 0x9d58c,
/// 0x9d4e8).
const LMG_SETTLED: f32 = 0.99;

/// UO's deploy driver (0x35b66), for `lmg` and `spotter` weapons, after
/// the sight's lerp: a sight flag on a weapon short of the sight starts the
/// deploy, its loss off the hip starts the breakdown, each `adsTransInTime`
/// or `adsTransOutTime` long. A raise, a drop, a swing and (sighted) a
/// reload leave it alone.
fn deploy_driver(ps: &mut PlayerState, def: &WeaponDef, events: &mut Vec<PmEvent>) {
    if ps.rules != MoveRules::Uo || !def.deploys() {
        return;
    }
    // State 13's arm (0x35bed) raises `LMG_BREAKDOWN` again, forced and as
    // a continue, on the move whose `PM_CheckDuck` took the eye into or out
    // of prone (`pmove_t` +0x100, `PlayerState::prone_flip`): the same
    // clause's anim is left alone, the other stance's clause restarts the
    // torso on its own timer (player-model-anim-system.md, "The takeoff
    // and the landing").
    if ps.weaponstate == WEAPON_BREAKDOWN && ps.prone_flip {
        ps.breakdown_continue = true;
    }
    match ps.weaponstate {
        WEAPON_RAISING | WEAPON_DROPPING | WEAPON_MELEE_WINDUP | WEAPON_MELEE_RELAX => return,
        WEAPON_RELOADING..=WEAPON_RELOAD_END if ps.ads_active => return,
        _ => {}
    }
    if ps.ads_active && ps.weapon_pos_frac != 1.0 && ps.weaponstate != WEAPON_DEPLOY {
        ps.weaponstate = WEAPON_DEPLOY;
        ps.weapon_time_ms = ms(def.ads_trans_in);
        set_anim(ps, WEAP_DEPLOY);
        push(events, EV_UO_DEPLOY_WEAPON);
    } else if !ps.ads_active && ps.weapon_pos_frac != 0.0 && ps.weaponstate != WEAPON_BREAKDOWN {
        ps.weaponstate = WEAPON_BREAKDOWN;
        ps.weapon_time_ms = ms(def.ads_trans_out);
        set_anim(ps, WEAP_BREAKDOWN);
        push(events, EV_UO_BREAKDOWN_WEAPON);
    }
}

/// UO's `PM_InteruptWeaponWithSprintMove` (0x33a44), on the cmd after a
/// sprinting one: a deploy or a breakdown is dropped with its timers and
/// the gun comes up.
pub fn interrupt_with_sprint(ps: &mut PlayerState) {
    if matches!(ps.weaponstate, WEAPON_DEPLOY | WEAPON_BREAKDOWN) {
        ps.weapon_time_ms = 0;
        ps.weapon_delay_ms = 0;
        ps.weaponstate = WEAPON_READY;
        if ps.weapon != 0 {
            hold_anim(ps, WEAP_RAISE);
        }
    }
}

/// `PmoveSingle`'s prone move key, the first thing a cmd runs (1.1d
/// 0x33e75, 1.5b 0x27fde, UO 0x2d67b; cod11-combat.md 1.13): on a prone
/// body (`pm_flags` 1) whose forward or right move grew in magnitude since
/// the last cmd, `PM_InteruptWeaponWithProneMove` (1.1d 0x37488, 1.5b
/// 0x2d4bc, UO 0x338b0) drops a busy state, and the sight goes with it
/// unless the state is one the press leaves alone.
pub fn prone_move_key(
    ps: &mut PlayerState,
    cmd: &crate::net::msg::UserCmd,
    weapons: &[Option<WeaponDef>],
) {
    if ps.stance != Stance::Prone {
        return;
    }
    let grew = |old: i8, new: i8| old != new && i16::from(old).abs() < i16::from(new).abs();
    if !grew(ps.last_cmd_moves[0], cmd.forward) && !grew(ps.last_cmd_moves[1], cmd.right) {
        return;
    }
    let def = weapons.get(ps.weapon as usize).and_then(Option::as_ref);
    if interrupt_with_prone_move(ps, def, cmd.weapon) {
        ps.ads_active = false;
    }
}

/// `PM_InteruptWeaponWithProneMove`: whether the sight drops. A shot and a
/// swing's relax keep both; the reloads, a raise, a drop and (on UO) a
/// rechamber keep the state; anything else (1.1's and 1.5's rechamber, a
/// swing's windup, UO's deploy and breakdown) goes back to ready with its
/// timers cleared and the idle anim, and the next `PM_Weapon` starts over
/// from there. UO keeps a sighted LMG first of all.
fn interrupt_with_prone_move(
    ps: &mut PlayerState,
    def: Option<&WeaponDef>,
    cmd_weapon: u8,
) -> bool {
    let uo = ps.rules == MoveRules::Uo;
    if uo && ps.ads_active && def.is_some_and(WeaponDef::is_lmg) {
        return false;
    }
    match ps.weaponstate {
        WEAPON_READY | WEAPON_RAISING | WEAPON_DROPPING => true,
        WEAPON_RELOADING..=WEAPON_RELOAD_END => true,
        WEAPON_RECHAMBERING if uo => true,
        WEAPON_FIRING | WEAPON_MELEE_RELAX => false,
        _ => {
            ps.weapon_time_ms = 0;
            ps.weapon_delay_ms = 0;
            ps.weaponstate = WEAPON_READY;
            // The anim helper reads the cmd's weapon byte (`pm->cmd + 6`).
            if cmd_weapon != 0 {
                hold_anim(ps, WEAP_IDLE);
            }
            true
        }
    }
}

/// `PM_Weapon` 0x39228-0x39254 (1.5 0x30643): unless the rechamber check
/// stopped it, a body with `pm_flags` 0x1 and a move key, or a swing
/// (states 10 and 11), holds the hip cone at 255. Retail's helpers before it
/// all run, so every early return of [`pm_weapon`] but the rechamber's calls
/// this.
fn open_cone_on_prone_move(ps: &mut PlayerState, input: &PmInput) {
    let moving = input.forward != 0.0 || input.right != 0.0;
    // UO holds it open on a sprint with a move key too.
    let prone_move = (ps.stance == Stance::Prone || ps.sprinting) && moving;
    if prone_move || matches!(ps.weaponstate, WEAPON_MELEE_WINDUP | WEAPON_MELEE_RELAX) {
        ps.aim_spread_scale = 255.0;
    }
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
            && input.weapon == ps.weapon
            && ps.ammoclip[def.clip_index] != 0
    });
    ps.weapon_time_ms = i32::from(latched);
    // The anim leaves the shot on this frame even when the state does not.
    // A rechamber ending writes no anim at all: pavlov's capture drops from
    // `weaponstate` 4 to 0 with `weapAnim` still reading the rechamber index,
    // and its sustained fire never shows the idle index between two shots.
    //
    // An unlatched shot leaves state 3 standing: the trigger-release check
    // ends it later in the frame, after the rechamber check has passed, so a
    // bolt-action rechambers on the next cmd (UO's jeep passenger capture,
    // coduo-vehicles.md 7; the timer is 0x387c0 on 1.1, 0x2f5ce on 1.5 and
    // 0x360fa on UO, all three alike). A latched trigger ends states 3, 4, 10
    // and 11 here with the idle pose, so a held bolt-action rechambers on
    // this very cmd (section 1.4).
    if latched
        && matches!(
            ps.weaponstate,
            WEAPON_FIRING | WEAPON_RECHAMBERING | WEAPON_MELEE_WINDUP | WEAPON_MELEE_RELAX
        )
    {
        hold_anim(ps, WEAP_IDLE);
        ps.weaponstate = WEAPON_READY;
        return delay_expired;
    }
    // Unlatched, the timer only zeroes `weaponTime` (0x388d0): a rechamber
    // (4) ends in the trigger-release check or the next shot, and a swing's
    // relax (11) in the melee finish, both later in `PM_Weapon`.
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
    // UO swings neither a deployed bipod nor one coming up (0x36fb7).
    let uo = ps.rules == MoveRules::Uo;
    if uo && (ps.deployed() || ps.weaponstate == WEAPON_BREAKDOWN) {
        return false;
    }
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
    if ps.melee_latched || uo && ps.sprinting {
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
    weapons: &[Option<WeaponDef>],
    events: &mut Vec<PmEvent>,
) -> bool {
    // Only a running `weaponTime` guards the check: then a reload or a
    // rechamber may be cut short, and a shot, a swing or a running
    // `weaponDelay` may not (dll 0x300112e0, UO 0x362ea). A pullback holds
    // `weaponTime` at 0, so a switch interrupts it whatever `weaponDelay`
    // reads (the capture's `cancel` step, section 1.14), and so does the
    // cmd a released shot's time runs out on, while its state 3 still stands.
    if ps.weapon_time_ms != 0
        && !matches!(ps.weaponstate, WEAPON_RECHAMBERING..=WEAPON_RELOAD_END)
        && (matches!(
            ps.weaponstate,
            WEAPON_FIRING | WEAPON_MELEE_WINDUP | WEAPON_MELEE_RELAX
        ) || ps.weapon_delay_ms != 0)
    {
        return false;
    }
    // The ladder forces weapon 0 (section 1.8's `pm_flags & 0x10`; 1.12 reads
    // that bit as the ladder), and with empty hands it ends the check before
    // the cmd byte is read (section 9.7).
    if hands_forced_empty(ps) {
        if ps.weapon == 0 {
            return false;
        }
        // Off the ladder or after `enableWeapon` only the cmd byte brings a
        // weapon back, so a cmd that echoes the holstered 0 stays
        // empty-handed (retail UO, `gametypes/mp_foy-basend-*-uo.txt`:
        // weapon 0 from the plant on; 1.1 ladder, section 9.7).
        return putaway(ps, weapons, 0, events);
    }
    if ps.weapon != 0 && !holds(ps, ps.weapon) {
        return putaway(ps, weapons, 0, events);
    }
    // A byte of 0 needs no held bit: it is a switch to empty hands.
    let wanted = input.weapon;
    if wanted != ps.weapon && (wanted == 0 || holds(ps, wanted)) {
        return putaway(ps, weapons, wanted, events);
    }
    false
}

/// The ladder, or 1.5's `disableWeapon`: 1.5's pmove tests `pm_flags`
/// 0x100000 beside the ladder's 0x10 at each site (game.mp 0x2e43d,
/// 0x2f7be) and holsters the same way.
fn hands_forced_empty(ps: &PlayerState) -> bool {
    ps.on_ladder || ps.weapon_disabled
}

/// Whether `to` is `from`'s `altWeapon`, the select-fire mode on the same gun
/// (def `+0x2fc`): the switch between the two is the alt arm of the putaway
/// and the pickup.
fn is_alt_of(weapons: &[Option<WeaponDef>], from: u8, to: u8) -> bool {
    to != 0 && weapon_def(weapons, from).is_some_and(|d| d.alt_index == usize::from(to))
}

/// The putaway, `.so` `0x37a9c` (section 1.8).
fn putaway(
    ps: &mut PlayerState,
    weapons: &[Option<WeaponDef>],
    target: u8,
    events: &mut Vec<PmEvent>,
) -> bool {
    if ps.weaponstate == WEAPON_DROPPING {
        return false;
    }
    // UO keeps a sighted or still-lowering bipod in hand (0x348ee).
    if ps.rules == MoveRules::Uo
        && weapon_def(weapons, ps.weapon).is_some_and(WeaponDef::deploys)
        && (ps.ads_active || ps.weapon_pos_frac > 0.0001)
    {
        return false;
    }
    ps.weapon_delay_ms = 0;
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
        // UO's short branch raises `EV_REMOVE_WEAPON_ATTACHMENTS` with the
        // weapon going down (game.mp.uo 0x448ee; coduo-diff.md 21).
        if ps.rules == MoveRules::Uo {
            events.push(PmEvent {
                event: EV_UO_REMOVE_WEAPON_ATTACHMENTS,
                parm: i32::from(ps.weapon),
            });
        }
        return true;
    }
    let def = weapon_def(weapons, ps.weapon);
    let alt = is_alt_of(weapons, ps.weapon, target);
    // The event is `PM_AddEvent`'s, whose parm is always 0 (`.so` 0x37bb5,
    // 0x37bee; section 1.8).
    if alt {
        // `WEAP_ALTSWITCHFROM` and `EV_WEAPON_ALT` on a select-fire switch.
        set_anim(ps, WEAP_ALTSWITCHFROM);
        push(events, EV_WEAPON_ALT);
    } else if !def.is_some_and(|d| d.clip_only && ps.ammoclip[d.clip_index] == 0) {
        // The capture's `to_frag` reads `weapAnim` 521 through the whole
        // putaway, `WEAP_DROP` with the toggle flipped. A spent `clipOnly`
        // weapon goes down with neither the anim nor the event.
        set_anim(ps, WEAP_DROP);
        push(events, EV_PUTAWAY_WEAPON);
    }
    ps.weapon_time_ms = ms(def.map_or(0.0, |d| if alt { d.alt_drop_time } else { d.drop_time }));
    if alt {
        alt_slot_swap(ps, weapons, target);
    }
    true
}

/// The tail of the putaway's alt arm (`.so` 0x37c94..0x37d7a): the slot the
/// held weapon, or a mode along its `altWeapon` chain, stands in takes the
/// mode switched to, so `weaponslots` names the mode in hand. Slot types 1
/// and 2 (`primary`, `primaryb`) share the two primary slots.
fn alt_slot_swap(ps: &mut PlayerState, weapons: &[Option<WeaponDef>], target: u8) {
    let slot_type = |w: u8| weapon_def(weapons, w).map_or(0, WeaponDef::slot_type);
    let from = ps.weapon;
    let mut w = from;
    let slot = loop {
        match slot_type(w) {
            0 => return,
            1 | 2 => {
                if let Some(s) = [1, 2].into_iter().find(|&s| ps.weapon_slots[s] == w) {
                    break s;
                }
            }
            t @ 3..=5 => {
                if ps.weapon_slots[t] == w {
                    break t;
                }
            }
            _ => return,
        }
        let next = weapon_def(weapons, w).map_or(0, |d| d.alt_index as u8);
        if next != 0 {
            w = next;
        }
        if w == from {
            return;
        }
    };
    if !holds(ps, target) {
        return;
    }
    match slot_type(target) {
        1 | 2 if slot <= 2 => ps.weapon_slots[slot] = target,
        t @ 3..=5 if t == slot => ps.weapon_slots[slot] = target,
        _ => {}
    }
}

/// The pickup half of section 1.8 (`.so` 0x37d84): the weapon raised is
/// the cmd's byte, forced to 0 on the ladder (retail's `pm_flags & 0x10`;
/// 1.5's `disableWeapon` too, 0x2e4bc), when not held or past the table. A
/// putaway whose cmds go back to the old byte raises the old weapon.
fn pickup(
    ps: &mut PlayerState,
    input: &PmInput,
    weapons: &[Option<WeaponDef>],
    events: &mut Vec<PmEvent>,
) {
    let target = input.weapon;
    let old = ps.weapon;
    ps.weapon =
        if hands_forced_empty(ps) || !holds(ps, target) || usize::from(target) >= weapons.len() {
            0
        } else {
            target
        };
    // The arms are exclusive and each writes `weapAnim` once: the same
    // weapon back in hand goes idle, a different one raises. The capture's
    // `to_frag` counts the toggle flips that prove it (section 1.14).
    if ps.weapon == old {
        ps.weaponstate = WEAPON_READY;
        set_anim(ps, WEAP_IDLE);
        return;
    }
    ps.weaponstate = WEAPON_RAISING;
    let def = weapon_def(weapons, ps.weapon);
    // The select-fire mode comes up on `altRaiseTime` with no raise event,
    // and only opens the cone to 128 (`.so` 0x37ef4, 0x37f81).
    if old != 0 && is_alt_of(weapons, old, ps.weapon) {
        ps.weapon_time_ms = def.map_or(0, |d| ms(d.alt_raise_time));
        ps.aim_spread_scale = ps.aim_spread_scale.max(128.0);
        set_anim(ps, WEAP_ALTSWITCHTO);
        return;
    }
    // Empty hands raise too, on weapon 0's `raiseTime` of 0. The event keys
    // off the weapon that went down, not the one coming up (section 1.8).
    ps.weapon_time_ms = def.map_or(0, |d| ms(d.raise_time));
    // A weapon coming up starts with the cone wide open (section 1.8).
    ps.aim_spread_scale = 255.0;
    set_anim(ps, WEAP_RAISE);
    if old != 0 {
        push(events, EV_RAISE_WEAPON);
    }
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
    // UO starts none while sprinting, nor on an LMG off its settled bipod
    // (0x358b2).
    if ps.rules == MoveRules::Uo
        && (ps.sprinting || def.is_lmg() && !(ps.ads_active && ps.weapon_pos_frac >= LMG_SETTLED))
    {
        return false;
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
    // Only a bullet weapon (`weaponType` 0) reloads from empty: a rocket's
    // dry reload takes `reloadTime` (combat doc 1.7; 1.1 cgame 0x30010330,
    // 1.5b and UO alike).
    let empty = ps.ammoclip[def.clip_index] == 0 && def.is_bullet();
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
        if ps.rules.v15() {
            // 1.5 falls back to the reload's own delay, not to 1 (0x2deee).
            let own = if add_ms == 0 {
                ps.weapon_time_ms
            } else {
                add_ms.min(ps.weapon_time_ms)
            };
            return (if bolt < own { bolt } else { own }).max(1);
        }
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
/// `ps.weaponrechamber`. True, ending `PM_Weapon` ahead of the prone cone,
/// only on the bolt's delay edge with the rechamber still running, as
/// `.so` 0x375d0 returns.
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
            return ps.weapon_time_ms != 0;
        }
        return false;
    }
    // A running `weaponTime` stops it only beside a running `weaponDelay`:
    // the latch leaves state 0 with `weaponTime` 1 (0x37670, section 1.4).
    if ps.weaponstate != WEAPON_READY || ps.weapon_time_ms != 0 && ps.weapon_delay_ms != 0 {
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
    false
}

/// `weaponType 1`, the one type with a pullback between the trigger and the
/// shot (section 1.11).
const WEAPON_TYPE_GRENADE: &str = "grenade";
/// United Offensive's flamethrower type, `weaponType` 4.
const WEAPON_TYPE_GAS: &str = "gas";

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
    // `.so` 0x38a44 arms the shot's timers and state ahead of the ammo check
    // (step 1), so a dry trigger holds `weaponstate` 3 for `fireTime` plus
    // step 2's 500 ms. A grenade keeps `weaponDelay` at the 0 the release
    // left it, which is what the capture's three throw frames read.
    if !grenade {
        ps.weapon_delay_ms = ms(def.fire_delay);
        ps.weapon_time_ms = ms(def.fire_time);
        // An `adsFire` weapon's shot waits out whatever is left of the raise
        // (`.so` 0x38b23, section 1.5 step 1).
        if def.ads_fire {
            ps.weapon_delay_ms = ((1.0 - ps.weapon_pos_frac)
                * trans_ms(def.ads_trans_in, ADS_TRANS_IN_DEFAULT_MS))
                as i32;
        }
        if def.bolt_action && (ps.weapon as u32) < u64::BITS {
            ps.weapon_rechamber |= 1u64 << ps.weapon;
        }
        ps.weaponstate = WEAPON_FIRING;
    }
    if ps.ammoclip[ci] < 1 {
        if ps.ammo[ai] > 0 {
            begin_reload(ps, def, events);
        } else {
            // A grenade raises nothing here (section 1.5 step 2). The idle
            // write is the index-preserving one (`.so` 0x38c46).
            if !grenade {
                push(events, EV_NOAMMO);
            }
            hold_anim(ps, WEAP_IDLE);
            ps.weapon_time_ms += 500;
        }
        return;
    }
    // Step 3: a delay the arm left stops the shot. It fires on the frame the
    // delay runs out, through `pm_weapon`'s expired-delay path, which arms
    // again: by then the sight is up and an `adsFire` delay comes out 0.
    if ps.weapon_delay_ms != 0 {
        return;
    }
    ps.ammoclip[ci] -= 1;
    if grenade {
        ps.weaponstate = WEAPON_FIRING;
        ps.weapon_time_ms = ms(def.fire_time);
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
    // the host owns `ps.weapons`. UO keeps a spent spotter (game.mp.uo
    // 0x35ce8 skips `weaponClass` 8; coduo-diff.md 21).
    if def.clip_only && last && ps.ammo[ai] == 0 && def.weapon_class != "spotter" {
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

    /// The cmds below carry the weapon in hand, as a retail client's do: a
    /// byte of 0 would leave every `weapAnim` write undone (section 1.2).
    fn held(ads: bool) -> PmInput {
        PmInput {
            weapon: 1,
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
            weapon: ps.weapon,
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
            weapon: 1,
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
        pm_weapon(
            &mut ps,
            &PmInput {
                weapon: 1,
                ..Default::default()
            },
            &w,
            50,
            &mut throw,
        );
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
            weapon: 1,
            attack: true,
            ..Default::default()
        };
        let ev = step(&mut ps, &w, &held, 120);
        assert_eq!(ev, vec![EV_PULLBACK_WEAPON]);
        assert_eq!(ps.grenade_time_left_ms, 4000);
        assert_eq!(ps.weaponstate, WEAPON_FIRING);
        assert_eq!(ps.ammoclip[1], 3);
    }

    /// UO's setter writes a `gas` weapon's index without the toggle
    /// (game.mp.uo 0x33fcd..0x33ff2): the `uo-flame` capture's flamethrower
    /// reads `weapAnim` 2 firing and 0 between bursts, never 0x200. 1.1's
    /// rules flip the toggle on the same weapon.
    #[test]
    fn a_uo_gas_weapon_fires_without_the_toggle() {
        let mut d = carbine();
        d.weapon_type = "gas".into();
        d.semi_auto = false;
        let fire = PmInput {
            weapon: 1,
            attack: true,
            ..Default::default()
        };
        for (rules, toggled) in [(MoveRules::Uo, false), (MoveRules::V1_1, true)] {
            let (mut ps, w) = armed(&d);
            ps.rules = rules;
            let mut seen = Vec::new();
            for _ in 0..8 {
                step(&mut ps, &w, &fire, 1);
                seen.push(ps.weap_anim);
            }
            assert!(seen.contains(&WEAP_ATTACK) || seen.contains(&(WEAP_ATTACK | ANIM_TOGGLEBIT)));
            assert_eq!(
                seen.iter().any(|a| a & ANIM_TOGGLEBIT != 0),
                toggled,
                "{rules:?} {seen:?}"
            );
        }
    }

    /// UO's cook (coduo-movement.md 12, the `uofoot` captures): the sight
    /// pressed on a pulled frag spoons it once, the fuse then runs by the
    /// frame, and under 51 ms the frag goes by itself with 50 left and the
    /// suicide event. Without the press the fuse holds as on 1.1.
    #[test]
    fn a_uo_frag_cooks_once_spooned_and_goes_by_itself() {
        let mut d = frag();
        d.cook_off_hold = true;
        d.explosion_inner_damage = 120;
        let (mut ps, w) = armed(&d);
        ps.rules = MoveRules::Uo;
        let pull = PmInput {
            weapon: 1,
            attack: true,
            ..Default::default()
        };
        let spoon = PmInput { ads: true, ..pull };
        assert_eq!(step(&mut ps, &w, &pull, 20), vec![EV_PULLBACK_WEAPON]);
        assert_eq!(ps.grenade_time_left_ms, 4000);
        assert_eq!(step(&mut ps, &w, &spoon, 1), vec![EV_UO_GRENADE_SPOON]);
        assert_eq!(ps.grenade_time_left_ms, 3940);
        // The release of the sight is an edge too, but the fuse is no longer
        // whole.
        assert!(step(&mut ps, &w, &pull, 1).is_empty());
        assert_eq!(ps.grenade_time_left_ms, 3890);
        assert!(cooking(&ps, &d));
        assert!(step(&mut ps, &w, &pull, 76).is_empty());
        assert_eq!(ps.grenade_time_left_ms, 90);
        let mut ev = Vec::new();
        pm_weapon(&mut ps, &pull, &w, 50, &mut ev);
        assert_eq!(
            ev,
            vec![
                PmEvent {
                    event: EV_FIRE_WEAPON,
                    parm: 50,
                },
                PmEvent {
                    event: EV_UO_GRENADE_SUICIDE,
                    parm: 0,
                },
            ]
        );
        assert_eq!(ps.grenade_time_left_ms, 0);
        assert_eq!(ps.weapon_time_ms, 1600);
        assert_eq!(ps.ammoclip[1], 2);
    }

    /// An empty clip has nothing to pull: no event, no fuse, no state change.
    #[test]
    fn a_grenade_with_an_empty_clip_does_nothing() {
        let (mut ps, w) = armed(&frag());
        ps.ammoclip[1] = 0;
        ps.ammo[1] = 0;
        let held = PmInput {
            weapon: 1,
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
            weapon: 1,
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
            weapon: 1,
            attack: true,
            ..Default::default()
        };
        assert_eq!(step(&mut ps, &w, &held, 1), vec![EV_PULLBACK_WEAPON]);
        // The pin holds for `holdFireTime`; the release after it throws.
        assert!(step(&mut ps, &w, &held, 13).is_empty());
        // Section 1.5, step 9: the spent `clipOnly` weapon raises `EV_NOAMMO`
        // beside the shot, and it is that pair the host takes the weapon on.
        let released = PmInput {
            weapon: 1,
            ..Default::default()
        };
        assert_eq!(
            step(&mut ps, &w, &released, 1),
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
        // The change to 0 raises empty hands, and the event with it: the
        // retail grenade capture's `throw_down` moves `eventSequence` by one
        // 155 on the frame `weapon` reads 0.
        let mut ev = Vec::new();
        while ps.weapon != 0 {
            ev.extend(step(&mut ps, &w, &asks_for_the_frag, 1));
        }
        assert_eq!(ev, vec![EV_RAISE_WEAPON]);
        step(&mut ps, &w, &asks_for_the_frag, 40);
        // It stays there while the client keeps asking for a weapon it does
        // not own: the retail grenade capture's `idle_after` reads `weapon` 0
        // under exactly that input.
        assert_eq!(ps.weapon, 0);
        assert_eq!(ps.weaponstate, WEAPON_READY);

        let asks_for_the_carbine = PmInput {
            weapon: 2,
            ..Default::default()
        };
        let ev = step(&mut ps, &w, &asks_for_the_carbine, 1);
        // Weapon 0 has no drop time, so the raise lands on the same frame,
        // and a raise out of empty hands raises no event: both retail swaps
        // of the pickup capture read `weapon` 0 to the new one with
        // `eventSequence` unmoved.
        assert!(ev.is_empty(), "{ev:?}");
        assert_eq!(ps.weapon, 2);
        assert_eq!(ps.weaponstate, WEAPON_RAISING);
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
        let anim = ps.weap_anim;
        for _ in 0..40 {
            assert_eq!(step(&mut ps, &w, &asks, 1), Vec::<i32>::new());
            // No change starts at all (`begin_change`'s ladder arm returns
            // on weapon 0), so the idle toggle never flips: the retail
            // ladder capture holds `weapAnim` 0x200 for the whole climb.
            assert_eq!(ps.weap_anim, anim);
        }
        assert_eq!(ps.weapon, 0);
        assert_eq!(ps.weaponstate, WEAPON_READY);
    }

    /// 1.10: the melee bit swings once per press -- the swipe, the hit event
    /// when `meleeDelay` runs out, and idle when `meleeTime` does.
    #[test]
    fn the_melee_bit_swings_once_per_press() {
        let (mut ps, w) = armed(&with_melee(carbine()));
        let held = PmInput {
            weapon: 1,
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
                weapon: 1,
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

    /// A rocket launcher's dry clip reloads as a partial one, over
    /// `reloadTime` with 151: the empty branch wants `weaponType` 0, and UO's
    /// `bazooka_mp` sets no `reloadEmptyTime`, which read as a 0 ms reload
    /// that never loaded (section 1.7).
    #[test]
    fn a_dry_rocket_launcher_reloads_over_reload_time() {
        let mut d = def(0.63, 0.0, 6.0, 0.4, 1, false);
        d.weapon_type = "projectile".into();
        d.reload_empty_time = 0.0;
        d.reload_add_time = 0.0;
        let (mut ps, w) = armed(&d);
        ps.ammo[1] = 2;
        let mut events = run(&mut ps, &w, true, 1);
        events.extend(run(&mut ps, &w, false, 14));
        assert_eq!(events[1..], [EV_RELOAD]);
        assert_eq!(ps.weaponstate, WEAPON_RELOADING);
        assert_eq!(ps.ammoclip[1], 0);
        run(&mut ps, &w, false, 120);
        assert_eq!(ps.weaponstate, WEAPON_READY);
        assert_eq!((ps.ammoclip[1], ps.ammo[1]), (1, 1));
    }

    /// The reload key on a partly full clip is an ordinary reload, 151, since
    /// neither fixture weapon sets `noPartialReload` (section 1.7).
    #[test]
    fn the_reload_key_on_a_partial_clip_raises_reload() {
        let (mut ps, w) = armed(&carbine());
        ps.ammoclip[1] = 10;
        let input = PmInput {
            weapon: 1,
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
            weapon: 1,
            reload: true,
            ..Default::default()
        };
        assert!(step(&mut ps, &w, &input, 1).is_empty());
        ps.ammoclip[1] = 0;
        assert_eq!(step(&mut ps, &w, &input, 1), vec![EV_RELOAD_FROM_EMPTY]);
    }

    /// No reserve: the tap clicks (149) and holds `weaponstate` 3 for
    /// `fireTime` plus half a second, the idle anim left unwritten (section
    /// 1.5; retail's `EV_NOAMMO` snapshot reads 3, 618, 0 a frame in).
    #[test]
    fn an_empty_weapon_with_no_reserve_clicks() {
        let (mut ps, w) = armed(&carbine());
        ps.ammoclip[1] = 0;
        ps.ammo[1] = 0;
        let e = run(&mut ps, &w, true, 1);
        assert_eq!(e, vec![EV_NOAMMO]);
        assert_eq!(ps.weaponstate, WEAPON_FIRING);
        assert_eq!(ps.weapon_time_ms, 635);
        assert_eq!(ps.weap_anim, 0);
        // Released, it is ready again once the time runs out, and the next
        // pull clicks again.
        assert!(run(&mut ps, &w, false, 13).is_empty());
        assert_eq!(ps.weaponstate, WEAPON_READY);
        assert_eq!(run(&mut ps, &w, true, 1), vec![EV_NOAMMO]);
    }

    /// The arm stores `fireDelay` before step 3 tests it, and arms again on
    /// the frame it runs out, so a gun with one never fires (section 1.5).
    /// Only grenades, which skip the arm, carry one in the stock files.
    #[test]
    fn a_fire_delay_rearms_on_every_pull_and_never_fires() {
        let mut d = def(0.0, 0.0, 2.0, 0.5, 15, false);
        d.semi_auto = false;
        d.fire_delay = 0.2;
        let (mut ps, w) = armed(&d);
        assert!(run(&mut ps, &w, true, 10).is_empty());
        assert_eq!(ps.ammoclip[1], 15);
        assert_eq!(ps.weaponstate, WEAPON_FIRING);
    }

    /// An `adsFire` weapon pulled from the hip arms `weaponDelay` for the
    /// rest of the sight's raise and fires nothing; the delay forces the
    /// sight up, and the shot goes on the frame it runs out, trigger held or
    /// not (section 1.5, step 1 and 3; 0x3925a's gate).
    #[test]
    fn an_ads_fire_shot_waits_for_the_sight() {
        let mut d = carbine_ads();
        d.ads_fire = true;
        d.fire_time = 1.0;
        let (mut ps, w) = armed(&d);
        assert!(run(&mut ps, &w, true, 1).is_empty());
        assert_eq!((ps.weapon_delay_ms, ps.weaponstate), (300, WEAPON_FIRING));
        assert_eq!(ps.ammoclip[1], 15);
        assert!(run(&mut ps, &w, false, 5).is_empty());
        assert_eq!(run(&mut ps, &w, false, 1), vec![EV_FIRE_WEAPON]);
        assert_eq!(ps.weapon_pos_frac, 1.0);
        assert_eq!(ps.weap_anim & !ANIM_TOGGLEBIT, WEAP_ADS_ATTACK);
        assert_eq!((ps.weapon_delay_ms, ps.ammoclip[1]), (0, 14));
        // From a settled sight the delay comes out 0 and the pull fires.
        let (mut ps, w) = armed(&d);
        step(&mut ps, &w, &held(true), 6);
        let pull = PmInput {
            weapon: 1,
            attack: true,
            ads: true,
            ..Default::default()
        };
        assert_eq!(step(&mut ps, &w, &pull, 1), vec![EV_FIRE_WEAPON]);
    }

    /// Unlatched, the timer leaves a swing's relax standing; the melee
    /// finish ends it on the next pass and that frame fires nothing, so a
    /// held trigger on an automatic shoots a frame later (0x388d0, 0x38f15).
    #[test]
    fn a_held_trigger_fires_the_frame_after_the_relax_ends() {
        let mut d = with_melee(carbine());
        d.semi_auto = false;
        let (mut ps, w) = armed(&d);
        let swing = PmInput {
            melee: true,
            ..Default::default()
        };
        assert_eq!(
            step(&mut ps, &w, &swing, 3),
            vec![EV_MELEE_SWIPE, EV_FIRE_MELEE]
        );
        assert!(run(&mut ps, &w, true, 11).is_empty());
        assert_eq!(ps.weaponstate, WEAPON_MELEE_RELAX);
        assert!(run(&mut ps, &w, true, 1).is_empty());
        assert_eq!((ps.weaponstate, ps.weapon_time_ms), (WEAPON_READY, 0));
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
    /// leaves 0 only once `weaponTime` is inside that window. The lerp runs
    /// ahead of the timers, so it reads the time the frame starts with.
    #[test]
    fn the_reload_pins_the_sight_down_until_its_last_ads_reload_trans_time() {
        let (mut ps, w) = armed(&carbine_ads());
        ps.ammoclip[1] = 10;
        let reload = PmInput {
            weapon: 1,
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
        // The next frame reaches 600 behind the lerp; the one after starts
        // the sight up.
        step(&mut ps, &w, &ads, 1);
        assert_eq!(ps.weapon_time_ms, 600);
        assert_eq!(ps.weapon_pos_frac, 0.0);
        step(&mut ps, &w, &ads, 1);
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
            weapon: 1,
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
            weapon: 1,
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
            weapon: 1,
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
            weapon: 1,
            attack: true,
            ads: true,
            ..Default::default()
        };
        step(&mut ps, &w, &ads_fire, 1);
        assert_eq!(ps.aim_spread_scale, 0.0, "no add from the sight");
        assert_eq!(ps.weap_anim & !ANIM_TOGGLEBIT, WEAP_ADS_ATTACK);
    }

    /// Both `weapAnim` setters refuse a cmd whose weapon byte is 0 (section
    /// 1.2): retail's grenade capture's last throw raises empty hands and
    /// `throw_down` and `idle_after` read `WEAP_RAISE` from then on, where
    /// the probe sent 0 for the weapon it no longer held. The raise still
    /// finishes.
    #[test]
    fn a_cmd_weapon_byte_of_zero_writes_no_weapon_anim() {
        for (byte, anim) in [(0, WEAP_RAISE | ANIM_TOGGLEBIT), (1, WEAP_IDLE)] {
            let (mut ps, w) = armed(&carbine());
            ps.weapon = byte;
            ps.weaponstate = WEAPON_RAISING;
            ps.weap_anim = WEAP_RAISE | ANIM_TOGGLEBIT;
            let input = PmInput {
                weapon: byte,
                ..Default::default()
            };
            step(&mut ps, &w, &input, 1);
            assert_eq!(ps.weaponstate, WEAPON_READY, "byte {byte}");
            assert_eq!(ps.weap_anim, anim, "byte {byte}");
        }
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

    /// A ladder forces weapon 0 through the putaway, and the client's byte,
    /// still the weapon it climbed with, raises it at the top (sections 1.8,
    /// 9.7).
    #[test]
    fn the_ladder_holsters_the_weapon_and_gives_it_back() {
        let (mut ps, w) = armed(&carbine());
        let climb = held(false);
        ps.on_ladder = true;
        assert_eq!(step(&mut ps, &w, &climb, 1), vec![EV_PUTAWAY_WEAPON]);
        assert_eq!(ps.weaponstate, WEAPON_DROPPING);
        assert_eq!(step(&mut ps, &w, &climb, 14), vec![EV_RAISE_WEAPON]);
        assert_eq!(ps.weapon, 0, "0.67 s of dropTime and the hands are empty");
        step(&mut ps, &w, &climb, 1);
        assert_eq!(ps.weaponstate, WEAPON_READY);
        // Nothing happens for as long as the climb lasts.
        assert!(step(&mut ps, &w, &climb, 20).is_empty());
        ps.on_ladder = false;
        // Out of empty hands: no event.
        assert!(step(&mut ps, &w, &climb, 1).is_empty());
        assert_eq!(ps.weapon, 1);
        assert_eq!(ps.weaponstate, WEAPON_RAISING);
    }

    /// Off the ladder a byte of 0 raises nothing: retail's pickup half reads
    /// only the cmd (section 9.7), so the old play client's echoed 0 kept a
    /// climber empty-handed on a retail server.
    #[test]
    fn a_climber_whose_byte_reads_zero_stays_empty_handed() {
        let (mut ps, w) = armed(&carbine());
        ps.on_ladder = true;
        step(&mut ps, &w, &held(false), 16);
        assert_eq!(ps.weapon, 0);
        ps.on_ladder = false;
        assert!(step(&mut ps, &w, &PmInput::default(), 20).is_empty());
        assert_eq!((ps.weapon, ps.weaponstate), (0, WEAPON_READY));
    }

    /// A cmd byte of 0 under a held weapon is a switch to empty hands, as
    /// retail's change check reads it (section 1.8: a change to `cmd.weapon`
    /// when it is 0 or held): the putaway, then weapon 0 raised with the
    /// event keyed off the weapon that went down.
    #[test]
    fn a_cmd_weapon_byte_of_zero_holsters_the_held_weapon() {
        let (mut ps, w) = armed(&carbine());
        assert_eq!(
            step(&mut ps, &w, &PmInput::default(), 1),
            vec![EV_PUTAWAY_WEAPON]
        );
        assert_eq!(ps.weaponstate, WEAPON_DROPPING);
        assert_eq!(
            step(&mut ps, &w, &PmInput::default(), 14),
            vec![EV_RAISE_WEAPON]
        );
        assert_eq!(ps.weapon, 0);
        // And the byte brings it back.
        step(&mut ps, &w, &PmInput::default(), 1);
        step(&mut ps, &w, &held(false), 1);
        assert_eq!((ps.weapon, ps.weaponstate), (1, WEAPON_RAISING));
    }

    /// 1.5's `disableWeapon` holsters as the ladder does, and after
    /// `enableWeapon` the weapon comes back only through the cmd's byte: a
    /// cmd that echoes the holstered 0 stays empty-handed, as retail UO's did
    /// (`gametypes/mp_foy-basend-*-uo.txt`).
    #[test]
    fn a_disabled_weapon_goes_down_like_the_ladders() {
        let (mut ps, w) = armed(&carbine());
        ps.weapon_disabled = true;
        assert_eq!(run(&mut ps, &w, false, 1), vec![EV_PUTAWAY_WEAPON]);
        run(&mut ps, &w, false, 15);
        assert_eq!(ps.weapon, 0);
        assert!(
            run(&mut ps, &w, true, 20).is_empty(),
            "no shot with it down"
        );
        ps.weapon_disabled = false;
        run(&mut ps, &w, false, 5);
        assert_eq!(ps.weapon, 0, "an echoed 0 keeps the hands empty");
        let input = PmInput {
            weapon: 1,
            ..Default::default()
        };
        step(&mut ps, &w, &input, 1);
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

        let events = step(&mut ps, &w, &input, 14);
        assert_eq!(events, vec![EV_RAISE_WEAPON]);
        assert_eq!(ps.weapon, 2);
        assert_eq!(ps.weaponstate, WEAPON_RAISING);

        // The raise runs for `raiseTime` and not for a frame: the capture's
        // `to_frag` holds `weaponstate` 1 for 6 snapshots of the frag's 250.
        step(&mut ps, &w, &input, 9);
        assert_eq!(ps.weaponstate, WEAPON_RAISING);
        step(&mut ps, &w, &input, 1);
        assert_eq!(ps.weaponstate, WEAPON_READY);
        assert_eq!(ps.weap_anim & !ANIM_TOGGLEBIT, WEAP_IDLE);
    }

    /// The pickup reads the cmd's byte, not the putaway's target (section
    /// 1.8, `.so` 0x37d84): a byte gone back to the held weapon mid-drop
    /// brings that weapon back to idle, with no raise.
    #[test]
    fn the_pickup_raises_what_the_cmd_names_at_the_drop_end() {
        let (mut ps, mut w) = armed(&carbine());
        w.push(Some(def(0.2, 0.0, 1.0, 0.3, 8, false)));
        give(&mut ps, 2, 3);
        let old = ps.weapon;
        let to = |weapon| PmInput {
            weapon,
            ..Default::default()
        };
        assert_eq!(step(&mut ps, &w, &to(2), 1), vec![EV_PUTAWAY_WEAPON]);
        let events = step(&mut ps, &w, &to(old), 14);
        assert!(events.is_empty(), "{events:?}");
        assert_eq!(ps.weapon, old);
        assert_eq!(ps.weaponstate, WEAPON_READY);
        assert_eq!(ps.weap_anim & !ANIM_TOGGLEBIT, WEAP_IDLE);
    }

    /// `EV_PUTAWAY_WEAPON` goes through `PM_AddEvent`, whose parm is 0:
    /// both retail weapsel runs read 0 (section 1.8).
    #[test]
    fn the_putaway_event_carries_no_parm() {
        let (mut ps, mut w) = armed(&carbine());
        w.push(Some(def(0.2, 0.0, 1.0, 0.3, 8, false)));
        give(&mut ps, 2, 3);
        let mut ev = Vec::new();
        let input = PmInput {
            weapon: 2,
            ..Default::default()
        };
        pm_weapon(&mut ps, &input, &w, 50, &mut ev);
        assert_eq!(
            ev,
            vec![PmEvent {
                event: EV_PUTAWAY_WEAPON,
                parm: 0
            }]
        );
    }

    /// A select-fire pair, thompson-like: 1 and 2 name each other as
    /// `altWeapon`, 1 in the primary slot.
    fn select_fire() -> (PlayerState, Vec<Option<WeaponDef>>) {
        let auto = WeaponDef {
            alt_index: 2,
            alt_drop_time: 0.4,
            alt_raise_time: 0.3,
            weapon_slot: "primary".into(),
            ..carbine()
        };
        let semi = WeaponDef {
            alt_index: 1,
            ..auto.clone()
        };
        let (mut ps, mut w) = armed(&auto);
        w.push(Some(semi));
        ps.weapons_held |= 1 << 2;
        (ps, w)
    }

    /// The switch to a weapon's `altWeapon` is the alt arm on both halves:
    /// `EV_WEAPON_ALT` and `WEAP_ALTSWITCHFROM` for `altDropTime`, then the
    /// mode up on `altRaiseTime` with `WEAP_ALTSWITCHTO`, no raise event,
    /// the cone opened only to 128, and the primary slot naming the mode
    /// now in hand (`.so` 0x37a9c, 0x37d84).
    #[test]
    fn an_alt_switch_takes_the_alt_arm_and_moves_the_slot() {
        let (mut ps, w) = select_fire();
        ps.aim_spread_scale = 20.0;
        let input = PmInput {
            weapon: 2,
            ..Default::default()
        };
        assert_eq!(step(&mut ps, &w, &input, 1), vec![EV_WEAPON_ALT]);
        assert_eq!(ps.weap_anim & !ANIM_TOGGLEBIT, WEAP_ALTSWITCHFROM);
        assert_eq!(ps.weapon_time_ms, 400);
        assert_eq!(ps.weapon_slots[1], 2);

        assert!(step(&mut ps, &w, &input, 8).is_empty());
        assert_eq!(ps.weapon, 2);
        assert_eq!(ps.weaponstate, WEAPON_RAISING);
        assert_eq!(ps.weapon_time_ms, 300);
        assert_eq!(ps.weap_anim & !ANIM_TOGGLEBIT, WEAP_ALTSWITCHTO);
        assert_eq!(ps.aim_spread_scale, 128.0);

        // And back: the slot follows the chain from the mode in hand.
        step(&mut ps, &w, &input, 7);
        let back = PmInput {
            weapon: 1,
            ..Default::default()
        };
        assert_eq!(step(&mut ps, &w, &back, 1), vec![EV_WEAPON_ALT]);
        assert_eq!(ps.weapon_slots[1], 1);
    }

    /// A spent `clipOnly` weapon goes down with no event and no anim write
    /// (`.so` 0x37b87).
    #[test]
    fn a_spent_clip_only_weapon_goes_down_silently() {
        let mut d = frag();
        d.clip_only = true;
        let (mut ps, mut w) = armed(&d);
        w.push(Some(carbine()));
        give(&mut ps, 2, 1);
        ps.ammoclip[1] = 0;
        let anim = ps.weap_anim;
        let input = PmInput {
            weapon: 2,
            ..Default::default()
        };
        assert!(step(&mut ps, &w, &input, 1).is_empty());
        assert_eq!(ps.weaponstate, WEAPON_DROPPING);
        assert_eq!(ps.weap_anim, anim);
    }

    /// A weapon taken from under the player goes down at once; UO's
    /// putaway says so with `EV_REMOVE_WEAPON_ATTACHMENTS`, its parm the
    /// weapon gone (the satchel capture's 202 parm 48), and 1.1's raises
    /// nothing.
    #[test]
    fn a_weapon_gone_from_under_the_player_goes_down_with_202_on_uo() {
        for rules in [MoveRules::V1_1, MoveRules::Uo] {
            let (mut ps, mut w) = armed(&frag());
            ps.rules = rules;
            w.push(Some(carbine()));
            give(&mut ps, 2, 2);
            ps.weapons_held &= !(1 << 1);
            let input = PmInput {
                weapon: 2,
                ..Default::default()
            };
            let mut ev = Vec::new();
            pm_weapon(&mut ps, &input, &w, 50, &mut ev);
            // No drop time: the raise follows in the same step.
            assert_eq!(ps.weaponstate, WEAPON_RAISING);
            let gone: Vec<_> = ev
                .iter()
                .filter(|e| e.event == EV_UO_REMOVE_WEAPON_ATTACHMENTS)
                .collect();
            let expect = if rules == MoveRules::Uo {
                vec![&PmEvent {
                    event: EV_UO_REMOVE_WEAPON_ATTACHMENTS,
                    parm: 1,
                }]
            } else {
                Vec::new()
            };
            assert_eq!(gone, expect, "{rules:?}");
        }
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
            weapon: 1,
            reload: true,
            ..Default::default()
        };
        let events = step(&mut ps, &w, &input, 1);
        assert_eq!(events, vec![EV_RELOAD_START]);
        assert_eq!(ps.weaponstate, WEAPON_RELOAD_START);
        let events = step(&mut ps, &w, &held(false), 120);
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
            weapon: 1,
            reload: true,
            ..Default::default()
        };
        let events = step(&mut ps, &w, &input, 1);
        assert_eq!(events, vec![EV_RELOAD_START]);
        let mut states = Vec::new();
        let mut events = Vec::new();
        for _ in 0..60 {
            events.extend(step(&mut ps, &w, &held(false), 50));
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
        step(&mut ps, &w, &held(false), 1);
        assert_eq!(ps.weaponstate, WEAPON_RELOADING);
        let input = PmInput {
            weapon: 1,
            attack: true,
            ..Default::default()
        };
        step(&mut ps, &w, &input, 1);
        assert_eq!(ps.weaponstate, WEAPON_RELOADING_INTERUPT);
        step(&mut ps, &w, &held(false), 40);
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

    /// The legs up blend toward the crouch and standing minimums, a dive's
    /// legs run on their shorter lengths, and an eye already at the leg's
    /// end reads the stance flags (combat doc 2.1).
    #[test]
    fn the_hip_spread_minimum_blends_every_leg_of_the_eye() {
        let def = WeaponDef {
            hip_spread_stand_min: 3.0,
            hip_spread_ducked_min: 2.0,
            hip_spread_prone_min: 1.0,
            ..WeaponDef::default()
        };
        let leg = |target: f32, down: bool, dive: bool, ms: i32| SpreadStance {
            prone: false,
            ducked: false,
            view_height: 30.0,
            lerp_target: target,
            lerp_down: down,
            dive,
            into_leg_ms: Some(ms),
        };
        let min = |s: SpreadStance| hip_spread_min(&def, &s);
        assert_eq!(
            min(leg(40.0, false, false, 100)),
            1.25,
            "prone up to crouch, 400 ms"
        );
        assert_eq!(
            min(leg(60.0, false, false, 50)),
            2.25,
            "crouch up to standing, 200 ms"
        );
        assert_eq!(
            min(leg(40.0, true, true, 50)),
            2.5,
            "a dive's 100 ms leg to crouch"
        );
        assert_eq!(
            min(leg(11.0, true, true, 100)),
            1.5,
            "a dive's 200 ms leg to prone"
        );
        let settled = SpreadStance {
            view_height: 40.0,
            ducked: true,
            ..leg(40.0, true, false, 75)
        };
        assert_eq!(min(settled), 2.0);
        let idle = SpreadStance {
            prone: true,
            into_leg_ms: None,
            ..settled
        };
        assert_eq!(min(idle), 1.0);
    }

    /// `rechamberWhileAds 0` lowers the sight through a rechamber, under
    /// 1.5's rules only.
    #[test]
    fn a_1_5_rechamber_lowers_a_sight_the_file_forbids() {
        let def = WeaponDef {
            aim_down_sight: true,
            rechamber_drops_ads: true,
            ads_trans_in: 0.2,
            ads_trans_out: 0.2,
            ..WeaponDef::default()
        };
        for (rules, frac) in [
            (crate::pmove::MoveRules::V1_1, 1.0),
            (crate::pmove::MoveRules::V1_5, 0.9),
        ] {
            let mut ps = PlayerState {
                rules,
                ads_active: true,
                weapon_pos_frac: 1.0,
                weaponstate: WEAPON_RECHAMBERING,
                ..PlayerState::spawn(glam::Vec3::ZERO, 0.0)
            };
            advance_ads(&mut ps, &def, 20);
            assert!((ps.weapon_pos_frac - frac).abs() < 1e-6, "{rules:?}");
        }
    }

    /// `PM_Weapon` 0x39254: a prone body with a move key holds the hip cone
    /// at 255, every frame; still, it decays.
    #[test]
    fn a_prone_body_on_the_move_holds_the_cone_open() {
        let (mut ps, w) = armed(&carbine());
        ps.stance = Stance::Prone;
        let crawl = PmInput {
            forward: 1.0,
            ..Default::default()
        };
        step(&mut ps, &w, &crawl, 3);
        assert_eq!(ps.aim_spread_scale, 255.0);
        step(&mut ps, &w, &PmInput::default(), 3);
        assert!(ps.aim_spread_scale < 255.0);
    }

    /// `.so` 0x375d0 returns 0 when a rechamber starts and while it runs,
    /// so the prone cone still opens on every frame of the bolt but the one
    /// whose delay edge ejects the case.
    #[test]
    fn a_crawl_holds_the_cone_open_through_a_rechamber() {
        let (mut ps, w) = armed(&WeaponDef {
            rechamber_bolt_time: 0.5,
            ..mosin()
        });
        ps.stance = Stance::Prone;
        let crawl = PmInput {
            weapon: 1,
            forward: 1.0,
            ..Default::default()
        };
        let fire = PmInput {
            weapon: 1,
            attack: true,
            ..crawl
        };
        step(&mut ps, &w, &fire, 1);
        let mut started = false;
        for _ in 0..40 {
            let ev = step(&mut ps, &w, &crawl, 1);
            started |= ev.contains(&EV_RECHAMBER_WEAPON);
            if started && ps.weaponstate == WEAPON_RECHAMBERING && !ev.contains(&EV_EJECT_BRASS) {
                assert_eq!(ps.aim_spread_scale, 255.0);
            }
        }
        assert!(started);
        assert_eq!(ps.weapon_rechamber, 0);
    }
}
