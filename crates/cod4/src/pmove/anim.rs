//! The legs and torso anims 1.7's pmove writes into the playerstate:
//! `PM_Footsteps`' and `Mantle_Move`'s `BG_AnimScriptAnimation` calls, the
//! jump, land, fire, reload, raise, drop and melee `BG_AnimScriptEvent`s,
//! and `BG_PlayAnim`'s channel rule (docs/research/cod4-player-anims.md 7).
//! The script is 1.7's `mp/playeranim.script` (a `common_mp` rawfile)
//! through vcod-common's parser; the movetypes, the condition tables and
//! the calls are INFERRED.
//!
//! Only a [`super::Pmove`] handed an [`AnimInput`] selects anims (the
//! server's); without one the timers still run down and the anims hold
//! what the snapshot said. Not here: the turn anims (`player_turnAnims` 0)
//! and the stance changes' events.

use vcod_common::animscript::{AnimRef, AnimScript, Clause, CondKind, Condition};
use vcod_common::animtree::PlayerAnims;

use super::{PlayerState, Pmove, ef, pm_type, pmf};

/// `legsAnim`/`torsoAnim`'s restart toggle.
pub const ANIM_TOGGLEBIT: u32 = 0x200;

/// `animMoveTypesStr`, lowercased as the script spells its
/// blocks; 1.7's script names each of these it uses (VERIFIED, the
/// `common_mp` rawfile). 1.7 adds `sprint` at 20 over CoD2's list.
pub const MOVETYPES: [&str; 43] = [
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
    "sprint",
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
    "stumble_sprint_forward",
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
    pub const SPRINT: u8 = 20;
    /// `MANTLE_ROOT`: a mantle clip's movetype is this plus its
    /// `s_mantleAnimNames` slot (`Mantle_Move`, 0x80515da).
    pub const MANTLE_ROOT: u8 = 21;
    pub const FLINCH_FORWARD: u8 = 32;
    pub const STUMBLE_FORWARD: u8 = 36;
    pub const STUMBLE_BACKWARD: u8 = 37;
    pub const STUMBLE_WALK_FORWARD: u8 = 38;
    pub const STUMBLE_WALK_BACKWARD: u8 = 39;
    pub const STUMBLE_CROUCH_FORWARD: u8 = 40;
    pub const STUMBLE_CROUCH_BACKWARD: u8 = 41;
    pub const STUMBLE_SPRINT_FORWARD: u8 = 42;
}

/// `animWeaponClassStr`: `weapClass` by number.
const WEAPON_CLASSES: [&str; 10] = [
    "rifle",
    "mg",
    "smg",
    "spread",
    "pistol",
    "grenade",
    "rocketlauncher",
    "turret",
    "non-player",
    "item",
];

/// `mp/playeranimtypes.txt` in `common_mp` (VERIFIED): `playerAnimType` by
/// number.
pub const PLAYER_ANIM_TYPES: [&str; 15] = [
    "none",
    "other",
    "pistol",
    "smg",
    "autorifle",
    "mg",
    "sniper",
    "rocketlauncher",
    "explosive",
    "grenade",
    "turret",
    "c4",
    "m203",
    "hold",
    "briefcase",
];

/// `animStrafeStateStr`.
const STRAFING: [&str; 3] = ["not", "left", "right"];

/// `player_dmgtimer_stumbleTime`, `..flinchTime` (500, 500),
/// `player_strafeAnimCosAngle` (0.5): 1.7's registered defaults
/// (`tools/re/cod4_dvars.py`, VERIFIED).
const STUMBLE_TIME: i32 = 500;
const FLINCH_TIME: i32 = 500;
const STRAFE_ANIM_COS: f32 = 0.5;

/// What one cmd's anim calls read beside the playerstate: the player
/// animtree with the script, and the viewmodel weapon's `playerAnimType`
/// (`WeaponDef`+296) and `weapClass` (+304), which
/// `BG_AnimUpdatePlayerStateConditions` sets before each step.
#[derive(Clone, Copy)]
pub struct AnimInput<'a> {
    pub anims: &'a PlayerAnims,
    pub player_anim_type: i32,
    pub weapon_class: i32,
}

/// A script event, by its name in `animEventTypesStr`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    FireWeapon,
    Jump,
    JumpBk,
    Land,
    DropWeapon,
    RaiseWeapon,
    Reload,
    MeleeAttack,
    KnifeMelee,
    KnifeMeleeCharge,
}

impl Event {
    fn name(self) -> &'static str {
        match self {
            Event::FireWeapon => "fireweapon",
            Event::Jump => "jump",
            Event::JumpBk => "jumpbk",
            Event::Land => "land",
            Event::DropWeapon => "dropweapon",
            Event::RaiseWeapon => "raiseweapon",
            Event::Reload => "reload",
            Event::MeleeAttack => "meleeattack",
            Event::KnifeMelee => "knife_melee",
            Event::KnifeMeleeCharge => "knife_melee_charge",
        }
    }
}

/// A value list as the script writes it (one word each): names, or
/// `all NOT a b` for every name but those.
fn lists(values: &[String], name: &str) -> bool {
    match values.split_first() {
        Some((all, rest)) if all == "all" => match rest.split_first() {
            Some((not, except)) if not == "not" => !except.iter().any(|v| v == name),
            _ => true,
        },
        _ => values.iter().any(|v| v == name),
    }
}

/// Whether `c` holds (`BG_EvaluateConditions`).
fn holds(ps: &PlayerState, buttons: u32, input: &AnimInput, c: &Condition) -> bool {
    let named = |names: &[&str], i: i32| {
        usize::try_from(i)
            .ok()
            .and_then(|i| names.get(i))
            .is_some_and(|n| lists(&c.values, n))
    };
    match &c.kind {
        CondKind::WeaponClass => named(&WEAPON_CLASSES, input.weapon_class),
        CondKind::Movetype => named(&MOVETYPES, i32::from(ps.anim_movetype)),
        CondKind::Mounted => ps.e_flags & ef::TURRET != 0 && lists(&c.values, "mg42"),
        CondKind::Firing => buttons & super::button::ATTACK != 0,
        // `eFlags` 0x40000 is the entity's; the playerstate's sight is
        // `PMF_SIGHT` (as CoD2's anim_ab found for its 0x40000).
        CondKind::WeaponPosition => lists(
            &c.values,
            if ps.pm_flags & pmf::SIGHT != 0 {
                "ads"
            } else {
                "hip"
            },
        ),
        CondKind::Strafing => named(&STRAFING, i32::from(ps.anim_strafing)),
        CondKind::Unknown(k) if k == "playeranimtype" => {
            named(&PLAYER_ANIM_TYPES, input.player_anim_type)
        }
        // `PERK`'s one value, the last stand.
        CondKind::Unknown(k) if k == "perk" => {
            ps.pm_type == pm_type::LASTSTAND && lists(&c.values, "laststand")
        }
        _ => false,
    }
}

fn matching<'b>(
    ps: &PlayerState,
    buttons: u32,
    input: &AnimInput,
    clauses: &'b [Clause],
) -> Option<&'b Clause> {
    clauses
        .iter()
        .find(|cl| cl.conditions.iter().all(|c| holds(ps, buttons, input, c)))
}

fn commands(clause: &Clause) -> usize {
    clause.legs.len().max(clause.torso.len())
}

/// One channel of `BG_PlayAnim`: a channel whose timer reads 50 or more
/// keeps its anim unless forced; a continuing call leaves the same anim
/// playing; anything else flips the toggle. True when the anim was set.
fn play(
    anim: &mut u32,
    timer: &mut i32,
    index: u32,
    duration: i32,
    set_timer: bool,
    cont: bool,
    force: bool,
) -> bool {
    if *timer >= 50 && !force {
        return false;
    }
    if cont && index == *anim & !ANIM_TOGGLEBIT {
        return false;
    }
    *anim = index | ((*anim & ANIM_TOGGLEBIT) ^ ANIM_TOGGLEBIT);
    if set_timer {
        *timer = duration;
    }
    true
}

/// `BG_ExecuteCommand` for the clause's `n`-th command: a `both` line puts
/// its anim on the legs and 0 on the torso; the duration is the line's,
/// else the clip's floored at 500, plus 50. The duration when the legs
/// took an anim, else -1.
fn execute(
    ps: &mut PlayerState,
    anims: &PlayerAnims,
    clause: &Clause,
    n: usize,
    (set_timer, cont, force): (bool, bool, bool),
) -> i32 {
    let legs = clause.legs.get(n);
    let torso = clause.torso.get(n);
    let duration = legs.or(torso).map_or(0, |a: &AnimRef| {
        a.duration_ms.map_or_else(
            || anims.length_ms(&a.name).unwrap_or(0).max(500) as i32,
            |d| d as i32,
        )
    }) + 50;
    let mut played = false;
    if let Some(a) = legs
        && let Some(i) = anims.wire_of(&a.name)
    {
        played = play(
            &mut ps.legs_anim,
            &mut ps.legs_timer,
            i as u32,
            duration,
            set_timer,
            cont,
            force,
        );
        if a.both {
            play(
                &mut ps.torso_anim,
                &mut ps.torso_timer,
                0,
                duration,
                set_timer,
                cont,
                force,
            );
            return if played { duration } else { -1 };
        }
    }
    if let Some(a) = torso
        && let Some(i) = anims.wire_of(&a.name)
    {
        play(
            &mut ps.torso_anim,
            &mut ps.torso_timer,
            i as u32,
            duration,
            set_timer,
            cont,
            force,
        );
    }
    if played { duration } else { -1 }
}

/// `BG_AnimScriptAnimation(ps, COMBAT, movetype, force)` (0x80502ea): the
/// movetype's first matching clause, its command picked by `clientNum`,
/// continuing, no timer; `MOVETYPE` takes the movetype once a clause with
/// a command matched. True when one did.
fn animation(pm: &mut Pmove, input: &AnimInput, movetype: u8, force: bool) -> bool {
    if pm.ps.pm_type >= pm_type::DEAD {
        return false;
    }
    let Some(block) = input
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
    execute(pm.ps, input.anims, clause, pick, (false, true, force));
    true
}

/// `Mantle_Move`'s call (0x80515ef): the clip's movetype, forced past a
/// channel's timer.
pub(crate) fn mantle(pm: &mut Pmove, movetype: u8) {
    if let Some(input) = pm.anim {
        animation(pm, &input, movetype, true);
    }
}

/// A pmove event: [`script_event`] on the cmd's buttons, its command
/// `rand() % n` (`BG_AnimScriptEvent`, 0x8050280: one draw whenever a
/// clause with commands matched), the first without [`Pmove::rand`].
pub(crate) fn event(pm: &mut Pmove, event: Event, cont: bool, force: bool) {
    if let Some(input) = pm.anim {
        let rand = &mut pm.rand;
        script_event(
            pm.ps,
            pm.cmd.buttons,
            &input,
            event.name(),
            |n| rand.as_mut().map_or(0, |r| r() as usize % n),
            (cont, force),
        );
    }
}

/// `BG_AnimScriptEvent(ps, event, isContinue, force)` for the event block
/// named `event`: the first matching clause's command `pick(n)` sets its
/// anims and their timers. Only `death` runs for a dead body. The
/// duration set, `None` when no clause with a command matched or the legs
/// kept theirs.
pub fn script_event(
    ps: &mut PlayerState,
    buttons: u32,
    input: &AnimInput,
    event: &str,
    pick: impl FnOnce(usize) -> usize,
    (cont, force): (bool, bool),
) -> Option<i32> {
    if event != "death" && ps.pm_type >= pm_type::DEAD {
        return None;
    }
    let block = input.anims.script.event(event)?;
    let clause = matching(ps, buttons, input, &block.clauses)?;
    let n = commands(clause);
    if n == 0 {
        return None;
    }
    let k = pick(n).min(n - 1);
    let d = execute(ps, input.anims, clause, k, (true, cont, force));
    (d >= 0).then_some(d)
}

/// An anim record's `movetype` (`animation_s` +0x58, a u64 of 1.7's
/// 0x68-byte records): bit `m` for every state's movetype block `m`
/// ([`MOVETYPES`]) with a legs or both line naming the anim
/// (`BG_ParseCommands`); torso lines and events add nothing.
pub fn movetype_mask(script: &AnimScript, name: &str) -> u64 {
    script
        .state_blocks()
        .filter(|b| {
            b.clauses
                .iter()
                .flat_map(|c| &c.legs)
                .any(|a| a.name.eq_ignore_ascii_case(name))
        })
        .filter_map(|b| {
            MOVETYPES
                .iter()
                .position(|m| m.eq_ignore_ascii_case(&b.name))
        })
        .fold(0, |m, i| m | 1 << i)
}

/// What 1.7's cgame reads off another player's legs anim
/// (cod4-effects.md 12.2): prone, `movetype & 0x308` (`iw3mp.exe`
/// 0x4561e7); crouched, `& 0xc4` or the high word's `& 0x300` (0x4059e7);
/// sighted, `& 0x3f0`, any walk (0x456237).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LegsPose {
    pub prone: bool,
    pub crouched: bool,
    pub sighted: bool,
}

impl LegsPose {
    pub fn of(movetype: u64) -> LegsPose {
        LegsPose {
            prone: movetype & 0x308 != 0,
            crouched: movetype & 0xc4 != 0 || (movetype >> 32) & 0x300 != 0,
            sighted: movetype & 0x3f0 != 0,
        }
    }
}

/// What one of an event clause's commands leaves on the playerstate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventAnim {
    pub legs_anim: u32,
    pub torso_anim: u32,
    pub legs_timer: i32,
    pub torso_timer: i32,
    pub duration: i32,
}

/// Every command [`script_event`] could pick for `event` (forced, not
/// continuing), each as it would leave `ps`: what a later draw picks among
/// (the death's `rand() % n` at `player_die`).
pub fn event_choices(
    ps: &PlayerState,
    buttons: u32,
    input: &AnimInput,
    event: &str,
) -> Vec<EventAnim> {
    let Some(block) = input.anims.script.event(event) else {
        return Vec::new();
    };
    let Some(clause) = matching(ps, buttons, input, &block.clauses) else {
        return Vec::new();
    };
    (0..commands(clause))
        .map(|k| {
            let mut p = ps.clone();
            let duration = execute(&mut p, input.anims, clause, k, (true, false, true));
            EventAnim {
                legs_anim: p.legs_anim,
                torso_anim: p.torso_anim,
                legs_timer: p.legs_timer,
                torso_timer: p.torso_timer,
                duration,
            }
        })
        .collect()
}

/// What `PM_Footsteps` found the body doing, which picks the legs' movetype.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Gait {
    /// On a turret (`eFlags` 0x300).
    Mounted,
    /// Under `player_moveThreshhold`, or linked to an entity.
    Still,
    /// Moving with no key held.
    Coasting,
    /// A key held: `row` is the stance's (stand 0, prone 1, crouch 2), 3
    /// more when running backwards.
    Keyed {
        row: usize,
        walking: bool,
        sprinting: bool,
    },
    /// On a ladder's rungs.
    Climbing,
}

/// The legs' clip for a held key, `cod4_lnxded` 0x820e0e0 (int32s, read
/// at 0x80561ab and 0x805a6d6 with index `(row * 2 + walking) * 2 +
/// stumbling`): per row the run, the stumbling run, the walk and the
/// stumbling walk. VERIFIED (the rodata).
const MOVE_ANIMS: [[u8; 4]; 6] = {
    use movetype::*;
    [
        [RUN, STUMBLE_FORWARD, WALK, STUMBLE_WALK_FORWARD],
        [WALKPRONE; 4],
        [
            RUNCR,
            STUMBLE_CROUCH_FORWARD,
            WALKCR,
            STUMBLE_CROUCH_FORWARD,
        ],
        [RUNBK, STUMBLE_BACKWARD, WALKBK, STUMBLE_WALK_BACKWARD],
        [WALKPRONEBK; 4],
        [
            RUNCRBK,
            STUMBLE_CROUCH_BACKWARD,
            WALKCRBK,
            STUMBLE_CROUCH_BACKWARD,
        ],
    ]
};

/// Inside `window` ms of the last hit: `damageTimer` past `damageDuration`
/// less the window, floored at 0 (the flinch's and the stumble's test,
/// 0x8056181..0x805619b).
fn hurt(ps: &PlayerState, window: i32) -> bool {
    let since = (ps.damage_duration - window).max(0);
    ps.damage_timer > since
}

/// `PM_Footsteps`' legs pick (docs/research/cod4-player-anims.md 7): the
/// gait's movetypes in order of preference, the first whose script block
/// has a matching clause playing, continued, its command by `clientNum`.
/// A held key sets `STRAFING` first, which the clauses test.
pub(crate) fn legs(pm: &mut Pmove, gait: Gait) {
    let Some(input) = pm.anim else { return };
    let ps = &mut *pm.ps;
    let flinch = hurt(ps, FLINCH_TIME);
    let idle_or_flinch = if flinch {
        movetype::FLINCH_FORWARD
    } else {
        movetype::IDLE
    };
    let prefs: [Option<u8>; 2] = match gait {
        Gait::Mounted => {
            let idle = if ps.pm_flags & pmf::PRONE != 0 {
                movetype::IDLEPRONE
            } else if ps.pm_flags & pmf::DUCKED != 0 {
                movetype::IDLECR
            } else {
                movetype::IDLE
            };
            [Some(idle), None]
        }
        // A standing body inside the flinch window flinches toward the
        // hit (`flinchYawAnim`, the four directions) and nothing else.
        Gait::Still if flinch && ps.view_height_target == super::VIEW_STAND => [
            Some(movetype::FLINCH_FORWARD + ps.flinch_yaw_anim.min(3) as u8),
            None,
        ],
        Gait::Still => {
            let idle = match ps.effective_stance() {
                super::Stance::Stand => movetype::IDLE,
                super::Stance::Crouch => movetype::IDLECR,
                super::Stance::Prone => movetype::IDLEPRONE,
            };
            let fallback = if ps.view_height_target == super::VIEW_CROUCH {
                movetype::IDLECR
            } else {
                idle_or_flinch
            };
            [Some(idle), Some(fallback)]
        }
        // Coasting faster than 120 leaves the legs as they were.
        Gait::Coasting if pm.xyspeed > 120.0 => [None, None],
        Gait::Coasting => {
            let low = match ps.view_height_target {
                super::VIEW_PRONE => Some(movetype::IDLEPRONE),
                super::VIEW_CROUCH => Some(movetype::IDLECR),
                _ => None,
            };
            [low, Some(idle_or_flinch)]
        }
        Gait::Keyed {
            row,
            walking,
            sprinting,
        } => {
            ps.anim_strafing = strafing(pm.cmd.forward_move, pm.cmd.right_move);
            let stumble = hurt(ps, STUMBLE_TIME);
            // A standing sprint has its own clip, 0x80561c1.
            let m = match (row, sprinting, stumble) {
                (0, true, false) => movetype::SPRINT,
                (0, true, true) => movetype::STUMBLE_SPRINT_FORWARD,
                _ => MOVE_ANIMS[row][usize::from(walking) * 2 + usize::from(stumble)],
            };
            [Some(m), Some(movetype::IDLE)]
        }
        Gait::Climbing => {
            let m = if ps.velocity.z < 0.0 {
                movetype::CLIMBDOWN
            } else {
                movetype::CLIMBUP
            };
            [Some(m), None]
        }
    };
    for m in prefs.into_iter().flatten() {
        if animation(pm, &input, m, false) {
            break;
        }
    }
}

/// `STRAFING` off the keys: not (0) unless the key's direction is within
/// `player_strafeAnimCosAngle` of the side, then left (1, no right key or
/// a left one) or right (2).
fn strafing(forward: i8, right: i8) -> u8 {
    let (fm, rm) = (f32::from(forward), f32::from(right));
    let len = (rm * rm + fm * fm).sqrt();
    let cos = if len > 0.0 { fm / len } else { 0.0 };
    if cos.abs() > STRAFE_ANIM_COS {
        0
    } else if right > 0 {
        2
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The player tree and script of a 1.7 install (`COD4_DIR`).
    fn install_anims() -> Option<PlayerAnims> {
        use crate::walk::{WalkOptions, walk};
        let install = crate::testing::cod4_install()?;
        let zone = |name: &str| {
            let z = crate::zone::Zone::open_path(&install.zone("english", name)).unwrap();
            walk(z, WalkOptions { keep_bulk: true }).unwrap().assets
        };
        let (map, common) = (zone("mp_crash"), zone("common_mp"));
        let src = crate::anim::BodySource::from_zones(&[&map, &common], Default::default()).ok()?;
        PlayerAnims::load(&src).ok()
    }

    struct Open;

    impl super::super::PmWorld for Open {
        fn trace(
            &self,
            _: glam::Vec3,
            _: glam::Vec3,
            _: glam::Vec3,
            _: glam::Vec3,
            _: u32,
            _: u32,
        ) -> super::super::Tr {
            super::super::Tr::default()
        }
    }

    /// One `PM_Footsteps` on `ps` with the forward and right keys.
    fn footsteps(input: AnimInput, mut ps: PlayerState, keys: (i8, i8)) -> PlayerState {
        use super::super::*;
        let cmd = Cmd {
            server_time: 5000,
            forward_move: keys.0,
            right_move: keys.1,
            ..Default::default()
        };
        let grounded = ps.ground_entity_num != ENTITYNUM_NONE;
        let mut pm = Pmove::new(&mut ps, cmd, cmd, &Open, Rules::default());
        pm.anim = Some(input);
        let pml = Pml {
            forward: glam::Vec3::X,
            right: glam::Vec3::Y,
            up: glam::Vec3::Z,
            frametime: 0.05,
            msec: 50,
            walking: grounded,
            ground_plane: grounded,
            almost_ground_plane: grounded,
            ground_trace: Tr::default(),
            impact_speed: 0.0,
            previous_origin: glam::Vec3::ZERO,
            previous_velocity: glam::Vec3::ZERO,
        };
        footsteps::footsteps(&mut pm, &pml);
        ps
    }

    /// The legs half of `PM_Footsteps` over a grid of stances, eye
    /// targets, speeds, keys, walk/sprint/back flags and hurts, pinned by
    /// an FNV-1a digest of the anim fields each call leaves.
    #[test]
    fn footsteps_legs_picks_hold() {
        use super::super::*;
        let Some(anims) = install_anims() else { return };
        let input = AnimInput {
            anims: &anims,
            player_anim_type: 4,
            weapon_class: 0,
        };
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut seen = std::collections::BTreeSet::new();
        let mut eat = |ps: &PlayerState| {
            seen.insert((ps.anim_movetype, ps.anim_strafing));
            let words = [
                u32::from(ps.anim_movetype),
                u32::from(ps.anim_strafing),
                ps.legs_anim,
                ps.torso_anim,
                ps.legs_timer as u32,
                ps.torso_timer as u32,
            ];
            for b in words.iter().flat_map(|w| w.to_le_bytes()) {
                h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
            }
        };
        let base = PlayerState {
            client_num: 1,
            ground_entity_num: ENTITYNUM_WORLD,
            legs_anim: 7,
            ..Default::default()
        };
        let flag_sets = [
            0,
            pmf::SIGHT_WALK,
            pmf::SPRINTING,
            pmf::BACKWARDS_RUN,
            pmf::BACKWARDS_RUN | pmf::SIGHT_WALK,
        ];
        let keys = [
            (0i8, 0i8),
            (127, 0),
            (0, 127),
            (0, -127),
            (-127, 0),
            (90, 90),
        ];
        let mut n = 0;
        for stance in [0, pmf::DUCKED, pmf::PRONE] {
            for view in [VIEW_STAND, VIEW_CROUCH, VIEW_PRONE] {
                for speed in [0.0f32, 5.0, 100.0, 150.0] {
                    for keys in keys {
                        for flags in flag_sets {
                            for (damage_timer, flinch_yaw_anim) in [(0, 0), (600, 1), (600, 5)] {
                                for extra in 0..4 {
                                    let ps = PlayerState {
                                        pm_type: if extra == 3 {
                                            pm_type::NORMAL_LINKED
                                        } else {
                                            pm_type::NORMAL
                                        },
                                        pm_flags: stance | flags,
                                        view_height_target: view,
                                        velocity: glam::Vec3::new(speed, 0.0, 0.0),
                                        leanf: if extra == 1 { 0.5 } else { 0.0 },
                                        e_flags: if extra == 2 { ef::TURRET } else { 0 },
                                        damage_timer,
                                        damage_duration: 1000,
                                        flinch_yaw_anim,
                                        ..base.clone()
                                    };
                                    eat(&footsteps(input, ps, keys));
                                    n += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        // Ladder climbs, both ways.
        for vz in [-50.0f32, 50.0] {
            let ps = PlayerState {
                ground_entity_num: ENTITYNUM_NONE,
                pm_flags: pmf::LADDER,
                velocity: glam::Vec3::new(0.0, 0.0, vz),
                ..base.clone()
            };
            eat(&footsteps(input, ps, (0, 0)));
        }
        assert_eq!(n, 3 * 3 * 4 * 6 * 5 * 3 * 4);
        // 63 (movetype, strafing) pairs: every walk, run, climb, sprint,
        // flinch and stumble the table names.
        assert_eq!(seen.len(), 63, "{seen:?}");
        assert_eq!(h, 0xc22e_8d7d_3ea8_4bd1, "digest {h:#x}");
    }

    #[test]
    fn all_not_lists_every_name_but_the_excepted() {
        let v: Vec<String> = ["all", "not", "m203"].map(String::from).to_vec();
        assert!(lists(&v, "autorifle"));
        assert!(!lists(&v, "m203"));
        assert!(lists(&["pistol".to_string()], "pistol"));
    }

    /// The legs anim's movetype bits and what 1.7's cgame reads off them:
    /// a walk reads sighted, a crouched walk crouched and sighted, a
    /// crouched run neither (0x4059e7 leaves `runcr` out).
    #[test]
    fn the_legs_movetypes_read_as_one_seven_reads_them() {
        let s = AnimScript::parse(
            "ANIMATIONS\nSTATE COMBAT\n{\n\tidle\n\t{\n\t\tdefault\n\t\t{\n\t\t\tboth pb_stand\n\t\t}\n\t}\n\twalk\n\t{\n\t\tdefault\n\t\t{\n\t\t\tlegs pb_walk\n\t\t\ttorso pt_walk\n\t\t}\n\t}\n\twalkcr\n\t{\n\t\tdefault\n\t\t{\n\t\t\tboth pb_crwalk\n\t\t}\n\t}\n\truncr\n\t{\n\t\tdefault\n\t\t{\n\t\t\tboth pb_crrun\n\t\t}\n\t}\n\tidleprone\n\t{\n\t\tdefault\n\t\t{\n\t\t\tboth pb_prone\n\t\t}\n\t}\n}\n",
        )
        .unwrap();
        let pose = |n: &str| LegsPose::of(movetype_mask(&s, n));
        assert_eq!(movetype_mask(&s, "pb_stand"), 1 << movetype::IDLE);
        assert_eq!(pose("pb_stand"), LegsPose::default());
        assert!(pose("pb_walk").sighted && !pose("pb_walk").crouched);
        assert_eq!(movetype_mask(&s, "pt_walk"), 0);
        let cr = pose("pb_crwalk");
        assert!(cr.crouched && cr.sighted && !cr.prone);
        assert_eq!(pose("pb_crrun"), LegsPose::default());
        assert!(pose("pb_prone").prone && !pose("pb_prone").sighted);
    }
}
