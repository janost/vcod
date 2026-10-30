//! The legs and torso anims pmove writes into the playerstate:
//! `PM_Footsteps`' `BG_AnimScriptAnimation` calls, the jump and land
//! `BG_AnimScriptEvent`s, and `BG_ExecuteCommand`'s channel rule
//! (docs/research/cod2-player-anims.md 9). The script is CoD2's
//! `mp/playeranim.script` through vcod-common's parser; the conditions and
//! the clause pick are CoD2's.
//!
//! Only a [`super::Pmove`] handed an [`AnimInput`] selects anims (the
//! server's); without one the timers still run down, as `PM_DropTimers`
//! does on every arm, and the anims hold what the snapshot said.

use anyhow::Result;
use vcod_common::animscript::{AnimRef, Clause, CondKind, Condition};
use vcod_common::animtree::PlayerAnims;
use vcod_common::asset_source::AssetSource;

use super::{ENTITYNUM_NONE, PlayerState, Pmove, ef, pm_type, pmf};

/// `legsAnim`/`torsoAnim`'s restart toggle (`^ 0x200`, `BG_ExecuteCommand`).
pub const ANIM_TOGGLEBIT: i32 = 0x200;

/// `animMoveTypesStr` (0x81848c0), lowercased as the script spells its
/// blocks; the index is the `MOVETYPE` condition's value.
pub const MOVETYPES: [&str; 41] = [
    "** unused **",
    "idle",
    "idlecr",
    "idleprone",
    "walk",
    "walkbk",
    "walkcr",
    "walkcrbk",
    "walkprone",
    "walkpronebk",
    "run",
    "runbk",
    "runcr",
    "runcrbk",
    "turnright",
    "turnleft",
    "turnrightcr",
    "turnleftcr",
    "climbup",
    "climbdown",
    "mantle_root",
    "mantle_up_57",
    "mantle_up_51",
    "mantle_up_45",
    "mantle_up_39",
    "mantle_up_33",
    "mantle_up_27",
    "mantle_up_21",
    "mantle_over_high",
    "mantle_over_mid",
    "mantle_over_low",
    "flinch_forward",
    "flinch_backward",
    "flinch_left",
    "flinch_right",
    "stumble_forward",
    "stumble_backward",
    "stumble_walk_forward",
    "stumble_walk_backward",
    "stumble_crouch_forward",
    "stumble_crouch_backward",
];

/// The movetypes `PM_Footsteps` asks for, indices into [`MOVETYPES`].
pub mod movetype {
    pub const IDLE: u8 = 1;
    pub const IDLECR: u8 = 2;
    pub const IDLEPRONE: u8 = 3;
    pub const WALK: u8 = 4;
    pub const WALKBK: u8 = 5;
    pub const WALKCR: u8 = 6;
    pub const WALKCRBK: u8 = 7;
    pub const WALKPRONE: u8 = 8;
    pub const WALKPRONEBK: u8 = 9;
    pub const RUN: u8 = 10;
    pub const RUNBK: u8 = 11;
    pub const RUNCR: u8 = 12;
    pub const RUNCRBK: u8 = 13;
    pub const CLIMBUP: u8 = 18;
    pub const CLIMBDOWN: u8 = 19;
    pub const FLINCH_FORWARD: u8 = 31;
    pub const FLINCH_BACKWARD: u8 = 32;
    pub const FLINCH_LEFT: u8 = 33;
    pub const FLINCH_RIGHT: u8 = 34;
    pub const STUMBLE_FORWARD: u8 = 35;
    pub const STUMBLE_BACKWARD: u8 = 36;
    pub const STUMBLE_WALK_FORWARD: u8 = 37;
    pub const STUMBLE_WALK_BACKWARD: u8 = 38;
    pub const STUMBLE_CROUCH_FORWARD: u8 = 39;
    pub const STUMBLE_CROUCH_BACKWARD: u8 = 40;
}

/// The flinch a hit `flinch_yaw` degrees off the view plays (Linux
/// 0x80e5672, the bounds rodata 0x8155088..0x8155098): ahead within 45,
/// then left, behind and right by quadrant.
fn flinch(flinch_yaw: i32) -> u8 {
    use movetype::*;
    let mut yaw = flinch_yaw as f32;
    if yaw < 0.0 {
        yaw += 360.0;
    }
    if !(45.0..315.0).contains(&yaw) {
        FLINCH_FORWARD
    } else if yaw < 135.0 {
        FLINCH_LEFT
    } else if yaw < 225.0 {
        FLINCH_BACKWARD
    } else {
        FLINCH_RIGHT
    }
}

/// The script events pmove raises, by their name in `animEventTypesStr`
/// (0x8184a20: `FIREWEAPON` 2, `JUMP` 3, `JUMPBK` 4, `LAND` 5,
/// `DROPWEAPON` 6, `RAISEWEAPON` 7, `RELOAD` 10, `PRONE_TO_CROUCH` 12,
/// `STAND_TO_CROUCH` 13, `CROUCH_TO_STAND` 14, `PRONE_TO_STAND` 16,
/// `MELEEATTACK` 17).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    FireWeapon,
    DropWeapon,
    RaiseWeapon,
    Reload,
    MeleeAttack,
    Jump,
    JumpBk,
    Land,
    ProneToCrouch,
    StandToCrouch,
    CrouchToStand,
    ProneToStand,
}

impl Event {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Event::FireWeapon => "fireweapon",
            Event::DropWeapon => "dropweapon",
            Event::RaiseWeapon => "raiseweapon",
            Event::Reload => "reload",
            Event::MeleeAttack => "meleeattack",
            Event::Jump => "jump",
            Event::JumpBk => "jumpbk",
            Event::Land => "land",
            Event::ProneToCrouch => "prone_to_crouch",
            Event::StandToCrouch => "stand_to_crouch",
            Event::CrouchToStand => "crouch_to_stand",
            Event::ProneToStand => "prone_to_stand",
        }
    }

    /// The takeoff a cmd's forward key picks (`(char)cmd.forwardmove < 0`).
    pub(crate) fn takeoff(forward_move: i8) -> Event {
        if forward_move < 0 {
            Event::JumpBk
        } else {
            Event::Jump
        }
    }
}

/// `STRAFING`'s values (0x8184be0).
const STRAFING: [&str; 3] = ["not", "left", "right"];

/// What the anim calls read off the paks: the animtree's wire index with the
/// script (`BG_LoadAnim`, through vcod-common's `PlayerAnims`), and the
/// `playerAnimType` names (`mp/playeranimtypes.txt`).
pub struct AnimTable {
    pub anims: PlayerAnims,
    pub anim_types: Vec<String>,
}

impl AnimTable {
    pub fn load(fs: &dyn AssetSource) -> Result<AnimTable> {
        let anim_types = fs
            .read("mp/playeranimtypes.txt")
            .map(|t| crate::weapon::player_anim_types(&String::from_utf8_lossy(&t)))
            .unwrap_or_default();
        Ok(AnimTable {
            anims: PlayerAnims::load(fs)?,
            anim_types: anim_types.iter().map(|s| s.to_ascii_lowercase()).collect(),
        })
    }

    /// `name`'s wire index.
    pub fn index(&self, name: &str) -> Option<i32> {
        self.anims.wire_of(name)
    }
}

/// What one cmd's anim calls read beside the playerstate: the table and the
/// held weapon's `playerAnimType` and `weaponClass` (`WeaponDef` +0x74 and
/// +0x7c, which `BG_AnimUpdatePlayerStateConditions` (Mac 0x182666) writes
/// into conditions 0 and 1 before each step).
#[derive(Clone, Copy)]
pub struct AnimInput<'a> {
    pub table: &'a AnimTable,
    pub player_anim_type: i32,
    pub weapon_class: i32,
}

/// Whether `c` holds. `PLAYERANIMTYPE`, `WEAPONCLASS` and `MOVETYPE` are bit
/// sets over their names; `MOUNTED`, `FIRING`, `WEAPON_POSITION` and
/// `STRAFING` one value each (`animConditionsTable` 0x8184c60). Every value
/// the script lists is a name, so both read as "the current name is listed".
fn holds(ps: &PlayerState, buttons: i32, input: &AnimInput, c: &Condition) -> bool {
    let listed = |name: &str| c.values.iter().any(|v| v == name);
    let named = |names: &[&str], i: i32| names.get(i as usize).is_some_and(|n| listed(n));
    match &c.kind {
        CondKind::WeaponClass => named(&crate::weapon::WEAPON_CLASSES, input.weapon_class),
        CondKind::Movetype => named(&MOVETYPES, i32::from(ps.anim_movetype)),
        // 1 under a turret's stance bits, the only value past `** UNUSED **`.
        CondKind::Mounted => ps.e_flags & ef::TURRET != 0 && listed("mg42"),
        CondKind::Firing => buttons & super::button::ATTACK != 0,
        // The code reads `eFlags` 0x40000 (Linux 0x80d9a1c), which no store
        // puts on a playerstate; retail's clauses go `ads` exactly while
        // `pm_flags` 0x40 is up (anim_ab's `fire` and `hurt` captures), the
        // bit `BG_PlayerStateToEntityState` turns into the entity's 0x40000.
        CondKind::WeaponPosition => listed(if ps.pm_flags & pmf::SIGHT != 0 {
            "ads"
        } else {
            "hip"
        }),
        CondKind::Strafing => named(&STRAFING, i32::from(ps.anim_strafing)),
        CondKind::Unknown(k) if k == "playeranimtype" => input
            .table
            .anim_types
            .get(input.player_anim_type as usize)
            .is_some_and(|n| listed(n)),
        _ => false,
    }
}

/// The first clause whose conditions all hold: the file's own rule.
fn matching<'b>(
    ps: &PlayerState,
    buttons: i32,
    input: &AnimInput,
    clauses: &'b [Clause],
) -> Option<&'b Clause> {
    clauses
        .iter()
        .find(|cl| cl.conditions.iter().all(|c| holds(ps, buttons, input, c)))
}

/// A clause's commands (`animScriptCommand_t`): line `i` is its `i`-th legs
/// and `i`-th torso anim, a `both` line one command on both channels.
fn commands(clause: &Clause) -> usize {
    clause.legs.len().max(clause.torso.len())
}

/// One channel of `BG_ExecuteCommand` (Mac 0x186baa): a channel whose timer
/// reads 50 or more keeps its anim unless forced; a continuing call leaves
/// the anim playing alone; anything else flips the toggle.
fn set_channel(
    anim: &mut i32,
    timer: &mut i32,
    index: i32,
    duration: i32,
    (set_timer, is_continue, force): (bool, bool, bool),
) {
    if *timer >= 50 && !force {
        return;
    }
    if is_continue && index == *anim & !ANIM_TOGGLEBIT {
        // A looping anim's timer is renewed here; no caller both sets the
        // timer and continues.
        return;
    }
    *anim = index | ((*anim & ANIM_TOGGLEBIT) ^ ANIM_TOGGLEBIT);
    if set_timer {
        *timer = duration;
    }
}

/// `BG_ExecuteCommand` for the clause's `n`-th command: a `both` line puts
/// its anim on the legs and no anim (0) on the torso. The duration is the
/// line's `duration`, else the anim's own (`BG_ParseCommands` copies the
/// record's +0x48, the clip's ms floored at 500 (INFERRED from
/// `BG_LoadAnim`'s store), plus 50. Returns the duration.
fn execute(
    ps: &mut PlayerState,
    table: &AnimTable,
    clause: &Clause,
    n: usize,
    flags: (bool, bool, bool),
) -> i32 {
    let legs = clause.legs.get(n);
    let torso = clause.torso.get(n);
    let first = legs.or(torso);
    let duration = first.map_or(0, |a: &AnimRef| {
        a.duration_ms.map_or_else(
            || table.anims.length_ms(&a.name).unwrap_or(0).max(500) as i32,
            |d| d as i32,
        )
    }) + 50;
    if let Some(a) = legs
        && let Some(i) = table.index(&a.name)
    {
        set_channel(&mut ps.legs_anim, &mut ps.legs_timer, i, duration, flags);
        if a.both {
            set_channel(&mut ps.torso_anim, &mut ps.torso_timer, 0, duration, flags);
            return duration;
        }
    }
    if let Some(a) = torso
        && let Some(i) = table.index(&a.name)
    {
        set_channel(&mut ps.torso_anim, &mut ps.torso_timer, i, duration, flags);
    }
    duration
}

/// `BG_AnimScriptAnimation(ps, COMBAT, movetype, 1)` (Mac 0x1873d2): the
/// movetype's first matching clause, its command picked by `clientNum`,
/// continuing and setting no timer. The `MOVETYPE` condition takes the
/// movetype once a clause with a command matched. Returns whether one did
/// (retail's `-1` is none). `RELAXED` and `ALERT`, which the loop falls
/// back to, are empty in the stock script.
fn animation(pm: &mut Pmove, input: &AnimInput, movetype: u8) -> bool {
    if pm.ps.pm_type >= pm_type::DEAD {
        return false;
    }
    let table = input.table;
    let Some(block) = table
        .anims
        .script
        .state("combat")
        .and_then(|b| b.iter().find(|b| b.name == MOVETYPES[movetype as usize]))
    else {
        return false;
    };
    let Some(clause) = matching(pm.ps, pm.cmd.buttons, input, &block.clauses) else {
        return false;
    };
    let n = commands(clause);
    if n == 0 {
        return false;
    }
    pm.ps.anim_movetype = movetype;
    let pick = pm.ps.client_num as usize % n;
    execute(pm.ps, table, clause, pick, (false, true, false));
    true
}

/// `Mantle_Move`'s `BG_AnimScriptAnimation(ps, COMBAT, slot + 20, 1)`
/// (0x80de260): the mantle anim slot's movetype (`MOVETYPES[20 + slot]`).
pub(crate) fn mantle(pm: &mut Pmove, slot: usize) {
    if let Some(input) = pm.anim {
        animation(pm, &input, (20 + slot) as u8);
    }
}

/// A pmove event: [`script_event`] off the cmd's buttons, the first
/// command (the events pmove raises list one).
pub(crate) fn event(pm: &mut Pmove, event: Event, is_continue: bool, force: bool) {
    if let Some(input) = pm.anim {
        script_event(
            pm.ps,
            pm.cmd.buttons,
            &input,
            event.name(),
            |_| 0,
            (is_continue, force),
        );
    }
}

/// `BG_AnimScriptEvent(ps, event, isContinue, force)` (Mac 0x18729a) for
/// the event block named `event` (the script's lowercase name): the first
/// matching clause sets its anims and their timers. `pick` draws the
/// command among the clause's `n` (retail's `rand() % n`). Only `death`
/// runs for a body past `pm_type` 5. Returns the duration the command
/// set, `None` when no clause with a command matched.
pub fn script_event(
    ps: &mut PlayerState,
    buttons: i32,
    input: &AnimInput,
    event: &str,
    pick: impl FnOnce(usize) -> usize,
    (is_continue, force): (bool, bool),
) -> Option<i32> {
    if event != "death" && ps.pm_type >= pm_type::DEAD {
        return None;
    }
    let block = input.table.anims.script.event(event)?;
    let clause = matching(ps, buttons, input, &block.clauses)?;
    let n = commands(clause);
    if n == 0 {
        return None;
    }
    let k = pick(n).min(n - 1);
    Some(execute(
        ps,
        input.table,
        clause,
        k,
        (true, is_continue, force),
    ))
}

/// `PM_Footsteps`' anim half (0x80e572c, Mac 0x6ed5c), called once
/// `xyspeed` is measured: the gunner's stance idle, the climb on a ladder,
/// then the idle under `player_moveThreshhold` or the move clip by stance,
/// the back key and the sighted walk, with the `STRAFING` condition the
/// keys set. Inside `stumble_time` of a hit (`damageTimer` above
/// `damageDuration - player_dmgtimer_stumbleTime`) a standing idle flinches
/// by `flinchYaw` and a standing or crouched move stumbles.
/// `player_turnAnims` registers 0 (`Dvar_RegisterBool`), so no turn anim.
pub(crate) fn footsteps(pm: &mut Pmove) {
    use movetype::*;
    let Some(input) = pm.anim else {
        return;
    };
    let ps = &*pm.ps;
    if ps.e_flags & ef::TURRET != 0 {
        let m = if ps.pm_flags & pmf::PRONE != 0 {
            IDLEPRONE
        } else if ps.pm_flags & pmf::DUCKED != 0 {
            IDLECR
        } else {
            IDLE
        };
        animation(pm, &input, m);
        return;
    }
    // `viewHeightTarget` 40 crouches and 11 lies down, the `pm_flags`
    // stance bits' values (2 and 1).
    let stance: u32 = match ps.view_height_target {
        super::VIEW_CROUCH => 2,
        super::VIEW_PRONE => 1,
        _ => 0,
    };
    let idle = match stance {
        1 => IDLEPRONE,
        2 => IDLECR,
        _ => IDLE,
    };
    if ps.ground_entity_num == ENTITYNUM_NONE && ps.pm_type != pm_type::NORMAL_LINKED {
        if ps.pm_flags & pmf::LADDER != 0 {
            if pm.cmd.server_time - ps.jump_time < 300 {
                return;
            }
            let m = if ps.velocity.z < 0.0 {
                CLIMBDOWN
            } else {
                CLIMBUP
            };
            animation(pm, &input, m);
        }
        if stance == pm.ps.pm_flags & (pmf::PRONE | pmf::DUCKED) {
            return;
        }
    }
    let ps = &*pm.ps;
    let stumble = ps.damage_timer > (ps.damage_duration - pm.rules.stumble_time).max(0);
    if pm.xyspeed < pm.rules.move_threshold || ps.pm_type == pm_type::NORMAL_LINKED {
        if stance == 0 && stumble {
            animation(pm, &input, flinch(ps.flinch_yaw));
            return;
        }
        if !animation(pm, &input, idle) {
            let m = match (stance, stumble) {
                (2, _) => IDLECR,
                (_, true) => FLINCH_FORWARD,
                _ => IDLE,
            };
            animation(pm, &input, m);
        }
        return;
    }
    let (fm, rm) = (pm.cmd.forward_move, pm.cmd.right_move);
    if fm != 0 {
        pm.ps.anim_strafing = 0;
    } else if rm != 0 {
        pm.ps.anim_strafing = if rm < 1 { 1 } else { 2 };
    }
    let ps = &*pm.ps;
    let sighted = ps.pm_flags & pmf::SIGHT_WALK != 0 || ps.leanf != 0.0;
    let back = ps.pm_flags & pmf::BACKWARDS_RUN != 0;
    let m = match (stance, back, sighted, stumble) {
        (1, true, _, _) => WALKPRONEBK,
        (1, false, _, _) => WALKPRONE,
        (2, true, _, true) => STUMBLE_CROUCH_BACKWARD,
        (2, false, _, true) => STUMBLE_CROUCH_FORWARD,
        (2, true, true, _) => WALKCRBK,
        (2, true, false, _) => RUNCRBK,
        (2, false, true, _) => WALKCR,
        (2, false, false, _) => RUNCR,
        (_, true, true, true) => STUMBLE_WALK_BACKWARD,
        (_, true, false, true) => STUMBLE_BACKWARD,
        (_, false, true, true) => STUMBLE_WALK_FORWARD,
        (_, false, false, true) => STUMBLE_FORWARD,
        (_, true, true, false) => WALKBK,
        (_, true, false, false) => RUNBK,
        (_, false, true, false) => WALK,
        (_, false, false, false) => RUN,
    };
    let moved = animation(pm, &input, m);
    if fm != 0 || rm != 0 {
        if !moved {
            animation(pm, &input, IDLE);
        }
        return;
    }
    // No key held: the move clip plays on past 120 (rodata 0x8155104); under
    // it a crouched or prone body takes its idle, a standing one keeps the
    // move clip.
    if pm.xyspeed > 120.0 {
        return;
    }
    let held = match stance {
        0 => moved,
        _ => animation(pm, &input, idle),
    };
    if !held {
        animation(pm, &input, if stumble { FLINCH_FORWARD } else { IDLE });
    }
}
