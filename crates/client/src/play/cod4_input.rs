//! CoD4 1.7's keys -> usercmd buttons, moves and weapon bytes, as
//! `CL_CmdButtons`, `CL_KeyMove` and the stance handlers build them
//! (INFERRED for 1.7; the
//! bits the probe sends did on retail what they say, cod4-netfields.md 8).
//! The keys come through vcod's console binds as [`Action`]s.
//!
//! Bits: `+attack` 0x1, `+sprint` 0x2 (not while backing), `+melee` 0x4,
//! `+activate` 0x8, `+reload` 0x10, `+usereload` 0x20, `+leanleft` 0x40,
//! `+leanright` 0x80, prone 0x100, crouch 0x200, jump 0x400 (`+gostand`
//! standing, or held by a spectator), the sight 0x800 (`+speed` XOR the
//! `toggleads` toggle), `+holdbreath` 0x2000, `+frag` 0x4000, `+smoke`
//! 0x8000, the held sight key's throw bit 0x80000 (`+speed_throw`), and
//! 0x100000 while a menu or the console holds the keys. The stock binds'
//! `+breath_sprint` holds 0x2000 and the sprint, `+toggleads_throw`'s
//! press flips the sight toggle and its hold is the throw bit, and
//! `+talk` holds nothing (voice only).
//!
//! The weapon byte is `cg_weaponSelect`, never an echo of `ps.weapon`:
//! a spawn sets it to the spawn weapon, `weapnext` / `weapprev` walk the
//! held primaries, the server's `a <n>` picks one and `C <n>` the equipped
//! offhand.

use std::collections::HashSet;

use vcod_cod4::pmove::button;

use super::input::{Action, StancePress, held_stance, stance_press};
use crate::console::shell::Effect;

/// `+speed_throw`'s throw bit (`KEY_THROW`, kb 26).
pub const THROW: u32 = 0x8_0000;
pub const ACTIVATE: u32 = 0x8;
pub const RELOAD: u32 = 0x10;
pub const USE_RELOAD: u32 = 0x20;
pub const HOLD_BREATH: u32 = 0x2000;

/// The held key each button bit follows.
const HELD: [(Action, u32); 14] = [
    (Action::Attack, button::ATTACK),
    (Action::Melee, button::MELEE),
    (Action::MeleeBreath, button::MELEE | HOLD_BREATH),
    (Action::Use, ACTIVATE),
    (Action::Reload, RELOAD),
    (Action::UseReload, USE_RELOAD),
    (Action::LeanLeft, button::LEAN_LEFT),
    (Action::LeanRight, button::LEAN_RIGHT),
    (Action::HoldBreath, HOLD_BREATH),
    (Action::Frag, button::FRAG),
    (Action::Smoke, button::SMOKE),
    (Action::Ads, THROW),
    (Action::BreathSprint, HOLD_BREATH),
    (Action::ToggleAdsThrow, THROW),
];

/// `cl->stance`.
pub use super::input::Stance;

#[derive(Default)]
pub struct Cod4Input {
    down: HashSet<Action>,
    pub stance: Stance,
    /// `cl->usingAds`.
    ads_toggle: bool,
    /// The held `+gostand` / `+moveup` keys that pressed the jump (kb 10)
    /// rather than moving the stance.
    jump_by: HashSet<Action>,
    /// `cg_weaponSelect`.
    selected: u8,
    /// The equipped offhand.
    offhand: u8,
    /// The spawn count (`stats[4]`) the selection was last reset for.
    spawn: Option<(u32, i32)>,
    /// The clock of the last `+actionslot` that did something
    /// (`iw3mp.exe` 0x84ca90).
    last_action_slot: Option<i32>,
    /// The last primary the selection sat on (0x79e6ac), what a weapon
    /// slot pressed on its own weapon goes back to.
    last_primary: u8,
    /// `cg.extraButtons`: bits the next cmd carries once.
    extra: u32,
}

/// What `+actionslot` reads of the newest playerstate.
#[derive(Clone, Copy, Debug, Default)]
pub struct SlotState {
    /// `actionSlotType[n - 1]` and `actionSlotParam[n - 1]`.
    pub kind: u32,
    pub param: u32,
    pub weapon_state: i32,
    pub pm_flags: u32,
    pub weap_flags: u32,
    pub e_flags: u32,
    /// The newest snapshot's `otherFlags`.
    pub other_flags: u32,
    pub held: [u32; 4],
    /// `ps.weapon`.
    pub weapon: u8,
}

/// `actionSlotType_t`: what a slot does (the server's `setActionSlot`; 3
/// is `nightvision`).
pub mod slot_kind {
    pub const WEAPON: u32 = 1;
    pub const ALT_WEAPON: u32 = 2;
    pub const NIGHT_VISION: u32 = 3;
}

/// `cg.extraButtons`' night vision request (`+actionslot`, 0x4579fa); the
/// server's pmove toggles the goggles on it.
pub const NIGHT_VISION: u32 = 0x4_0000;
/// The extra bit a weapon slot adds while `eFlags` 0x300 (a turret or
/// vehicle) is set.
const SLOT_ON_USE: u32 = 0x20;

/// The weapons a playerstate holds: `weapons[0..3]`, a bit each, so
/// weapons 1..=127; a higher index is held by nobody.
pub fn holds(weapons: [u32; 4], w: u8) -> bool {
    let (word, b) = (usize::from(w) / 32, u32::from(w) % 32);
    w != 0 && weapons.get(word).is_some_and(|bits| bits >> b & 1 != 0)
}

impl Cod4Input {
    /// Whether `action`'s key is down (`+talk` for voice chat,
    /// `IN_IsTalkKeyHeld`).
    pub fn held(&self, action: Action) -> bool {
        self.down.contains(&action)
    }

    /// A `+button` bind's edge.
    pub fn key(&mut self, action: Action, pressed: bool) {
        let was = self.down.contains(&action);
        if pressed {
            self.down.insert(action);
        } else {
            self.down.remove(&action);
        }
        // `IN_ToggleADS_Throw_Down`: the press flips the sight toggle.
        if action == Action::ToggleAdsThrow && pressed && !was {
            self.ads_toggle = !self.ads_toggle;
        }
        if !pressed {
            self.jump_by.remove(&action);
        } else if !was && matches!(action, Action::Jump | Action::MoveUp) {
            self.stance_press(action);
        }
    }

    /// `IN_GoStandDown`, `IN_UpDown` and the stance one-shots (`iw3mp.exe`
    /// 0x462f40, 0x4628a0, 0x462e20..0x462f20), each gated on the held
    /// `+prone` (kb byte 0x8f1708) and `+movedown` (0x8f1604).
    fn stance_press(&mut self, action: Action) {
        match stance_press(action, self.stance, held_stance(&self.down).is_some()) {
            StancePress::Set(s) => self.stance = s,
            StancePress::Jump => {
                self.jump_by.insert(action);
            }
            StancePress::Nothing => {}
        }
    }

    /// `CL_SetStance` (0x462870), what a forced-stance event calls: the
    /// stance, unless `+prone` or `+movedown` is held.
    pub fn set_stance(&mut self, stance: Stance) {
        if held_stance(&self.down).is_none() {
            self.stance = stance;
        }
    }

    /// A console effect a bind raised: a `+button` edge or a one-shot
    /// (the stance ones, `toggleads`, `leaveads`, `weapnext`, `weapprev`,
    /// `weaponslot`). `primaries` are the held weapons
    /// `weapnext` walks, in index order. True when it was the input's.
    pub fn effect(&mut self, effect: &Effect, primaries: &[u8]) -> bool {
        match effect {
            Effect::Button(action, down) => self.key(*action, *down),
            Effect::Impulse(action) => match action {
                Action::Crouch
                | Action::Prone
                | Action::ToggleCrouch
                | Action::ToggleProne
                | Action::LowerStance
                | Action::RaiseStance => self.stance_press(*action),
                Action::ToggleAds => self.ads_toggle = !self.ads_toggle,
                Action::LeaveAds => self.ads_toggle = false,
                Action::NextWeapon | Action::PrevWeapon => {
                    self.cycle(primaries, *action == Action::NextWeapon)
                }
                Action::Slot(n) => {
                    if let Some(&w) = n.checked_sub(1).and_then(|i| primaries.get(i)) {
                        self.select(w);
                    }
                }
                _ => return false,
            },
            _ => return false,
        }
        true
    }

    /// Lets go of every held key; the stance and the selection stay.
    pub fn release_all(&mut self) {
        self.down.clear();
        self.jump_by.clear();
    }

    /// `CG_SelectWeaponIndex`: another weapon drops the sight toggle.
    pub fn select(&mut self, w: u8) {
        if w != self.selected {
            self.selected = w;
            self.ads_toggle = false;
        }
    }

    /// `+actionslot` (`iw3mp.exe` 0x457960, client-view doc 9) on a slot
    /// whose state is `s`, at the client clock `now_ms`, over the held
    /// `primaries` and each weapon's alt (`alt_of`). True when the press
    /// acted: it selected a weapon or asked for the goggles.
    pub fn action_slot(
        &mut self,
        s: &SlotState,
        alt_of: &dyn Fn(u8) -> u8,
        primaries: &[u8],
        now_ms: i32,
    ) -> bool {
        if !self.slot_usable(s, now_ms) {
            return false;
        }
        // vcod's stand-in for the select's own bookkeeping (0x458f7d): the
        // selection's primary, its own or an alt mode's.
        if let Some(&p) = [self.selected, alt_of(self.selected)]
            .iter()
            .find(|p| primaries.contains(p))
        {
            self.last_primary = p;
        }
        let acted = match s.kind {
            slot_kind::WEAPON => self.weapon_slot(s, primaries),
            slot_kind::ALT_WEAPON => self.alt_mode_slot(s, alt_of, primaries),
            slot_kind::NIGHT_VISION => {
                self.extra |= NIGHT_VISION;
                true
            }
            _ => false,
        };
        if acted {
            self.last_action_slot = Some(now_ms);
            if s.kind == slot_kind::WEAPON && s.e_flags & 0x300 != 0 {
                self.extra |= SLOT_ON_USE;
            }
        }
        acted
    }

    /// Whether a press may act at all (0x457969..0x4579bd): a body in the
    /// world (`otherFlags` 4), its weapon not going down (states 3 and
    /// 4), not just respawned or frozen, weapons on (`weapFlags` 0x80
    /// clear), and 250 ms since the last press that acted.
    fn slot_usable(&self, s: &SlotState, now_ms: i32) -> bool {
        use vcod_cod4::pmove::{POF_PLAYER, pmf};
        let in_world = s.other_flags & POF_PLAYER != 0;
        let lowering = matches!(s.weapon_state, 3 | 4);
        let held_still = s.pm_flags & (pmf::RESPAWNED | pmf::FROZEN) != 0;
        let weapons_off = s.weap_flags & 0x80 != 0;
        let too_soon = self.last_action_slot.is_some_and(|t| now_ms - t < 250);
        in_world && !lowering && !held_still && !weapons_off && !too_soon
    }

    /// A `weapon` slot (0x457a19): another held weapon than the one in
    /// hand is selected; pressed on the selection itself, the last held
    /// primary, else the next primary.
    fn weapon_slot(&mut self, s: &SlotState, primaries: &[u8]) -> bool {
        let w = s.param as u8;
        if w != self.selected {
            let usable = holds(s.held, w) && s.weapon != w;
            if usable {
                self.select(w);
            }
            return usable;
        }
        let back = self.last_primary;
        if back != w && holds(s.held, back) {
            self.select(back);
            return true;
        }
        self.cycle(primaries, true);
        self.selected != w
    }

    /// An `altMode` slot (0x4578b0): not while the weapon raises, drops or
    /// switches mode (states 2..4); the selection's alt, else the alt of
    /// the first held primary after the selection that has one. 1.7 looks
    /// for one with ammo first (0x457630); vcod takes the first.
    fn alt_mode_slot(
        &mut self,
        s: &SlotState,
        alt_of: &dyn Fn(u8) -> u8,
        primaries: &[u8],
    ) -> bool {
        if (2..=4).contains(&s.weapon_state) {
            return false;
        }
        let after = primaries
            .iter()
            .position(|&p| p == self.selected)
            .map_or(0, |i| i + 1);
        let next_alt = || {
            primaries
                .iter()
                .cycle()
                .skip(after)
                .take(primaries.len())
                .map(|&p| alt_of(p))
                .find(|&a| a != 0)
        };
        let Some(alt) = Some(alt_of(self.selected))
            .filter(|&a| a != 0)
            .or_else(next_alt)
        else {
            return false;
        };
        self.select(alt);
        true
    }

    /// `cg.extraButtons` for the cmd going out, cleared.
    pub fn take_extra(&mut self) -> u32 {
        std::mem::take(&mut self.extra)
    }

    /// The server's `C <n>`: the equipped offhand.
    pub fn set_offhand(&mut self, w: u8) {
        self.offhand = w;
    }

    /// `CG_NextWeapon_f` / `CG_PrevWeapon_f` over `primaries`.
    fn cycle(&mut self, primaries: &[u8], forward: bool) {
        let n = primaries.len();
        if n == 0 {
            return;
        }
        let next = match primaries.iter().position(|&w| w == self.selected) {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None => 0,
        };
        self.select(primaries[next]);
    }

    /// Takes the newest playerstate (`clientNum`, the spawn count
    /// `stats[4]`, `weapon`, `offHandIndex`, the held `weapons`): a new
    /// spawn sets the selection to the spawn weapon and stands up, and a
    /// selection the playerstate no longer holds falls back to its weapon.
    pub fn sync(&mut self, client: u32, spawns: i32, weapon: u8, offhand: u8, held: [u32; 4]) {
        let spawn = (client, spawns);
        if self.spawn.replace(spawn) != Some(spawn) {
            self.selected = weapon;
            self.offhand = offhand;
            self.stance = Stance::Stand;
            self.ads_toggle = false;
        } else if !holds(held, self.selected) && weapon != 0 {
            self.selected = weapon;
        }
    }

    /// The cmd's `weapon` and `offHandIndex`.
    pub fn weapons(&self) -> (u8, u8) {
        (self.selected, self.offhand)
    }

    /// The cmd's buttons for a body of `pm_type` (spectators jump on a
    /// held `+gostand`); `talking` while a menu or the console holds the
    /// keys, which sends that bit alone.
    pub fn buttons(&self, pm_type: i32, talking: bool) -> u32 {
        if talking {
            return button::TALK;
        }
        let mut b = 0;
        for (a, bits) in HELD {
            if self.down.contains(&a) {
                b |= bits;
            }
        }
        // `CL_KeyMove` (0x4631b0): a held `+prone`, then `+movedown`, with
        // 0x1000, else `cl->stance`.
        let key_stance = held_stance(&self.down);
        if key_stance.is_some() {
            b |= button::STANCE_HELD;
        }
        b |= match key_stance.unwrap_or(self.stance) {
            Stance::Crouch => button::CROUCH,
            Stance::Prone => button::PRONE,
            Stance::Stand => 0,
        };
        // `CL_CmdButtons`: kb 12, which `+gostand` and `+moveup` hold, held
        // by a noclip, UFO or spectator body.
        let up = self.down.contains(&Action::Jump) || self.down.contains(&Action::MoveUp);
        if !self.jump_by.is_empty() || (up && (2..=4).contains(&pm_type)) {
            b |= button::JUMP;
        }
        if self.ads_toggle != self.down.contains(&Action::Ads) {
            b |= button::ADS_MODE;
        }
        let sprint =
            self.down.contains(&Action::Sprint) || self.down.contains(&Action::BreathSprint);
        if sprint && !self.down.contains(&Action::Back) {
            b |= button::SPRINT;
        }
        b
    }

    /// `forwardmove` and `rightmove`: 127 a held key, opposite keys
    /// cancel; nothing while a menu holds the keys.
    pub fn moves(&self, talking: bool) -> [i8; 2] {
        if talking {
            return [0; 2];
        }
        let axis = |plus: Action, minus: Action| {
            let (p, m) = (self.down.contains(&plus), self.down.contains(&minus));
            127 * i8::from(p) - 127 * i8::from(m)
        };
        [
            axis(Action::Forward, Action::Back),
            axis(Action::Right, Action::Left),
        ]
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_weapon_past_the_bit_array_is_not_held() {
        let all = [u32::MAX; 4];
        assert!(super::holds(all, 127));
        assert!(!super::holds(all, 128));
        assert!(!super::holds(all, 255));
        assert!(!super::holds(all, 0));
    }

    use super::*;

    /// `action_slot`'s pick and refusals on hand-built slot states: the
    /// weapon each press selects (0 for a refused press) and the extra bits
    /// it leaves for the next cmd.
    #[test]
    fn cod4_input_action_slot_picks() {
        use vcod_cod4::pmove::pmf;
        let held = [1 << 3 | 1 << 11 | 1 << 27 | 1 << 28, 1 << 8, 0, 0];
        let alt_of = |w: u8| match w {
            27 => 28,
            28 => 27,
            _ => 0,
        };
        let base = SlotState {
            kind: slot_kind::WEAPON,
            param: 40,
            other_flags: vcod_cod4::pmove::POF_PLAYER,
            held,
            weapon: 11,
            ..Default::default()
        };
        let press = |s: SlotState, selected: u8, primaries: &[u8]| {
            let mut i = Cod4Input::default();
            i.sync(0, 1, selected, 0, held);
            let acted = i.action_slot(&s, &alt_of, primaries, 1000);
            (if acted { i.weapons().0 } else { 0 }, i.take_extra())
        };
        let p = [11, 27];
        assert_eq!(press(base, 11, &p), (40, 0));
        for refused in [
            SlotState {
                weapon_state: 3,
                ..base
            },
            SlotState {
                weapon_state: 4,
                ..base
            },
            SlotState {
                pm_flags: pmf::RESPAWNED,
                ..base
            },
            SlotState {
                pm_flags: pmf::FROZEN,
                ..base
            },
            SlotState {
                weap_flags: 0x80,
                ..base
            },
            SlotState { param: 50, ..base },
            SlotState { weapon: 40, ..base },
            SlotState { kind: 0, ..base },
        ] {
            assert_eq!(press(refused, 11, &p), (0, 0), "{refused:?}");
        }
        assert_eq!(
            press(
                SlotState {
                    weapon_state: 2,
                    ..base
                },
                11,
                &p
            ),
            (40, 0)
        );
        assert_eq!(
            press(
                SlotState {
                    e_flags: 0x100,
                    ..base
                },
                11,
                &p
            ),
            (40, SLOT_ON_USE)
        );
        assert_eq!(
            press(
                SlotState {
                    e_flags: 0x200,
                    ..base
                },
                11,
                &p
            ),
            (40, SLOT_ON_USE)
        );
        let alt = SlotState {
            kind: slot_kind::ALT_WEAPON,
            ..base
        };
        assert_eq!(press(alt, 27, &p), (28, 0));
        assert_eq!(press(alt, 28, &p), (27, 0));
        assert_eq!(press(alt, 11, &p), (28, 0));
        assert_eq!(press(alt, 3, &[3, 11]), (0, 0));
        assert_eq!(
            press(
                SlotState {
                    weapon_state: 2,
                    ..alt
                },
                27,
                &p
            ),
            (0, 0)
        );
        assert_eq!(
            press(
                SlotState {
                    e_flags: 0x100,
                    ..alt
                },
                27,
                &p
            ),
            (28, 0)
        );
        let nv = SlotState {
            kind: slot_kind::NIGHT_VISION,
            ..base
        };
        assert_eq!(press(nv, 11, &p).1, NIGHT_VISION);
        assert_eq!(
            press(
                SlotState {
                    weap_flags: 0x80,
                    ..nv
                },
                11,
                &p
            ),
            (0, 0)
        );
        // One press a 250 ms window, whatever slot made it.
        let mut i = Cod4Input::default();
        i.sync(0, 1, 11, 0, held);
        assert!(i.action_slot(&base, &alt_of, &p, 1000));
        assert!(!i.action_slot(&alt, &alt_of, &p, 1249));
        assert!(i.action_slot(&alt, &alt_of, &p, 1250));
        assert!(!i.action_slot(&nv, &alt_of, &p, 1400));
        // A refused press starts no window.
        let mut i = Cod4Input::default();
        i.sync(0, 1, 11, 0, held);
        assert!(!i.action_slot(
            &SlotState {
                weapon_state: 3,
                ..base
            },
            &alt_of,
            &p,
            1000
        ));
        assert!(i.action_slot(&base, &alt_of, &p, 1010));
    }

    /// 1.7's gates past the old ones (0x457960): a press needs a body in
    /// the world, a vehicle's `eFlags` refuse nothing, and a weapon slot
    /// pressed on its own weapon goes back to the last primary.
    #[test]
    fn cod4_input_action_slot_follows_1_7() {
        let held = [1 << 11 | 1 << 27, 1 << 8, 0, 0];
        let c4 = SlotState {
            kind: slot_kind::WEAPON,
            param: 40,
            other_flags: vcod_cod4::pmove::POF_PLAYER,
            held,
            weapon: 27,
            ..Default::default()
        };
        let none = |_: u8| 0;
        let mut i = Cod4Input::default();
        i.sync(0, 1, 27, 0, held);
        assert!(!i.action_slot(
            &SlotState {
                other_flags: 0,
                ..c4
            },
            &none,
            &[11, 27],
            1000
        ));
        assert!(!i.action_slot(
            &SlotState {
                other_flags: 0xa,
                ..c4
            },
            &none,
            &[11, 27],
            1000
        ));
        assert!(i.action_slot(
            &SlotState {
                e_flags: 0x2_0000,
                ..c4
            },
            &none,
            &[11, 27],
            1000
        ));
        assert_eq!(i.weapons().0, 40);
        let in_hand = SlotState { weapon: 40, ..c4 };
        assert!(i.action_slot(&in_hand, &none, &[11, 27], 1300));
        assert_eq!(i.weapons().0, 27);
        // With no last primary held, the press cycles the primaries.
        let mut i = Cod4Input::default();
        i.sync(0, 1, 40, 0, held);
        assert!(i.action_slot(&in_hand, &none, &[11, 27], 1000));
        assert_eq!(i.weapons().0, 11);
    }

    #[test]
    fn cod4_stance_jump_and_sight() {
        let mut i = Cod4Input::default();
        i.effect(&Effect::Impulse(Action::Crouch), &[]);
        assert_eq!(i.buttons(0, false), button::CROUCH);
        // The first press stands up, the next jumps.
        i.key(Action::Jump, true);
        assert_eq!(i.buttons(0, false), 0);
        i.key(Action::Jump, false);
        i.key(Action::Jump, true);
        assert_eq!(i.buttons(0, false), button::JUMP);
        i.key(Action::Jump, false);
        i.key(Action::Ads, true);
        assert_eq!(i.buttons(0, false), button::ADS_MODE | THROW);
        i.key(Action::Ads, false);
        i.effect(&Effect::Impulse(Action::ToggleAds), &[]);
        assert_eq!(i.buttons(0, false), button::ADS_MODE);
        assert_eq!(i.buttons(0, true), button::TALK);
    }

    /// `CL_KeyMove` (0x4631b0) and `CL_SetStance` (0x462870) under a held
    /// `+movedown`; `togglecrouch` keeps the stance alone.
    #[test]
    fn cod4_held_and_toggled_stances() {
        let mut i = Cod4Input::default();
        i.key(Action::MoveDown, true);
        assert_eq!(i.buttons(0, false), button::CROUCH | button::STANCE_HELD);
        i.set_stance(Stance::Prone);
        i.effect(&Effect::Impulse(Action::ToggleCrouch), &[]);
        i.key(Action::MoveDown, false);
        assert_eq!(i.buttons(0, false), 0);
        i.effect(&Effect::Impulse(Action::ToggleCrouch), &[]);
        assert_eq!(i.buttons(0, false), button::CROUCH);
        i.key(Action::HoldProne, true);
        assert_eq!(i.buttons(0, false), button::PRONE | button::STANCE_HELD);
    }

    #[test]
    fn cod4_selection_is_the_clients_own() {
        let mut i = Cod4Input::default();
        let held = [1 << 11 | 1 << 27 | 1 << 28, 1 << (40 - 32), 0, 0];
        i.sync(2, 3, 27, 40, held);
        assert_eq!(i.weapons(), (27, 40));
        // `weapnext` walks the primaries; the playerstate's weapon does
        // not pull it back while the selection is held.
        i.effect(&Effect::Impulse(Action::NextWeapon), &[11, 27]);
        assert_eq!(i.weapons().0, 11);
        i.sync(2, 3, 27, 40, held);
        assert_eq!(i.weapons().0, 11);
        // A respawn takes the spawn weapon.
        i.sync(2, 4, 27, 40, held);
        assert_eq!(i.weapons().0, 27);
        i.select(9);
        i.sync(2, 4, 27, 40, held);
        assert_eq!(i.weapons().0, 27);
    }

    /// The stock keys a CoD4 install starts with (`main.rs`: vcod's
    /// defaults under CoD2's under CoD4's) through the console's binds into the cmd's
    /// buttons and weapon byte.
    #[test]
    fn cod4_default_binds_fire_sight_and_switch() {
        use crate::console::shell::Shell;
        const PRIMARIES: [u8; 2] = [11, 27];
        fn key(shell: &mut Shell, i: &mut Cod4Input, k: &str, down: bool) {
            for e in shell.key_event(k, down) {
                assert!(i.effect(&e, &PRIMARIES), "{k}: {e:?}");
            }
        }
        fn tap(shell: &mut Shell, i: &mut Cod4Input, k: &str) {
            key(shell, i, k, true);
            key(shell, i, k, false);
        }
        fn held(shell: &mut Shell, i: &mut Cod4Input, k: &str) -> u32 {
            key(shell, i, k, true);
            let b = i.buttons(0, false);
            key(shell, i, k, false);
            assert_eq!(i.buttons(0, false), 0, "{k} let go");
            b
        }
        let mut shell = Shell::new();
        shell.use_cod2_binds();
        shell.use_cod4_binds();
        let sh = &mut shell;
        let mut i = Cod4Input::default();
        let i = &mut i;
        i.sync(0, 1, 27, 40, [1 << 11 | 1 << 27, 0, 0, 0]);
        assert_eq!(held(sh, i, "MOUSE1"), button::ATTACK);
        assert_eq!(held(sh, i, "R"), RELOAD);
        assert_eq!(held(sh, i, "F"), ACTIVATE);
        assert_eq!(held(sh, i, "G"), button::FRAG);
        assert_eq!(held(sh, i, "4"), button::SMOKE);
        // `default_mp_controls.cfg`'s `bind SHIFT "+breath_sprint"` and
        // `bind v "+melee"`; backing, the sprint drops.
        assert_eq!(held(sh, i, "SHIFT"), button::SPRINT | HOLD_BREATH);
        key(sh, i, "S", true);
        assert_eq!(held(sh, i, "SHIFT"), HOLD_BREATH);
        key(sh, i, "S", false);
        assert_eq!(held(sh, i, "V"), button::MELEE);
        // MOUSE2 is `+toggleads_throw`: held, the throw key; a tap sights,
        // the next one lets go.
        key(sh, i, "MOUSE2", true);
        assert_eq!(i.buttons(0, false), button::ADS_MODE | THROW);
        key(sh, i, "MOUSE2", false);
        assert_eq!(i.buttons(0, false), button::ADS_MODE);
        tap(sh, i, "MOUSE2");
        assert_eq!(i.buttons(0, false), 0);
        tap(sh, i, "MOUSE2");
        assert_eq!(i.buttons(0, false), button::ADS_MODE);
        tap(sh, i, "MOUSE2");
        assert_eq!(i.buttons(0, false), 0);
        // `+speed` sights while held, with the throw bit.
        sh.execute("bind MOUSE4 +speed");
        assert_eq!(held(sh, i, "MOUSE4"), button::ADS_MODE | THROW);
        // 1 and 2 are `weapnext`, the wheel `weapnext` / `weapprev`.
        assert_eq!(i.weapons(), (27, 40));
        tap(sh, i, "1");
        assert_eq!(i.weapons().0, 11);
        tap(sh, i, "MWHEELUP");
        assert_eq!(i.weapons().0, 27);
        tap(sh, i, "MWHEELDOWN");
        assert_eq!(i.weapons().0, 11);
        // A switch drops the sight toggle (`CG_SelectWeaponIndex`).
        tap(sh, i, "MOUSE2");
        tap(sh, i, "2");
        assert_eq!((i.weapons().0, i.buttons(0, false)), (27, 0));
        sh.execute("bind 3 weaponslot 1");
        tap(sh, i, "3");
        assert_eq!(i.weapons().0, 11);
        // Z is `+talk`, which no cmd carries; T and Y open the chat field.
        assert_eq!(held(sh, i, "Z"), 0);
        assert_eq!(
            sh.key_event("T", true),
            vec![crate::console::shell::Effect::MessageMode { team: false }]
        );
        assert_eq!(
            sh.key_event("Y", true),
            vec![crate::console::shell::Effect::MessageMode { team: true }]
        );
        // Tab is the board's, never the cmd's.
        assert_eq!(
            sh.key_event("TAB", true),
            vec![crate::console::shell::Effect::Scores(true)]
        );
    }

    /// `default_mp_controls.cfg`'s `bind 5 "+actionslot 3"`: on the slot
    /// `_class.gsc` makes `altMode`, the M16 (27) goes to its grenade
    /// launcher (28) and, 250 ms on, back; a slot `setActionSlot` named a
    /// weapon selects it; nothing while the weapon drops.
    #[test]
    fn cod4_actionslot_3_toggles_the_grenade_launcher() {
        use crate::console::shell::{Effect, Shell};
        let mut shell = Shell::new();
        shell.use_cod2_binds();
        shell.use_cod4_binds();
        assert_eq!(
            shell.key_event("5", true),
            [Effect::Impulse(Action::UseSlot(3))]
        );
        assert!(shell.key_event("5", false).is_empty());
        let mut i = Cod4Input::default();
        let held = [1 << 11 | 1 << 27 | 1 << 28, 0, 0, 0];
        i.sync(0, 1, 27, 40, held);
        let alt_of = |w: u8| match w {
            27 => 28,
            28 => 27,
            _ => 0,
        };
        let s = SlotState {
            kind: slot_kind::ALT_WEAPON,
            other_flags: vcod_cod4::pmove::POF_PLAYER,
            held,
            weapon: 27,
            ..Default::default()
        };
        assert!(i.action_slot(&s, &alt_of, &[11, 27], 1000));
        assert_eq!(i.weapons().0, 28);
        assert!(!i.action_slot(&s, &alt_of, &[11, 27], 1100));
        assert!(i.action_slot(&s, &alt_of, &[11, 27], 1250));
        assert_eq!(i.weapons().0, 27);
        let dropping = SlotState {
            weapon_state: 3,
            ..s
        };
        assert!(!i.action_slot(&dropping, &alt_of, &[11, 27], 2000));
        // A `nightvision` slot asks the next cmd, once, for the goggles.
        let goggles = SlotState {
            kind: slot_kind::NIGHT_VISION,
            ..s
        };
        assert!(i.action_slot(&goggles, &alt_of, &[11, 27], 2500));
        assert_eq!(i.take_extra(), NIGHT_VISION);
        assert_eq!(i.take_extra(), 0);
        let pistol = SlotState {
            kind: slot_kind::WEAPON,
            param: 11,
            ..s
        };
        assert!(i.action_slot(&pistol, &alt_of, &[11, 27], 3000));
        assert_eq!(i.weapons().0, 11);
    }

    #[test]
    fn cod4_sprint_and_moves() {
        let mut i = Cod4Input::default();
        i.key(Action::Forward, true);
        i.key(Action::Sprint, true);
        assert_eq!(i.buttons(0, false), button::SPRINT);
        assert_eq!(i.moves(false), [127, 0]);
        i.key(Action::Back, true);
        assert_eq!(i.buttons(0, false), 0);
        assert_eq!(i.moves(false), [0, 0]);
        assert_eq!(i.moves(true), [0, 0]);
    }
}
