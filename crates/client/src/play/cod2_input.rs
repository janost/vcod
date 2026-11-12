//! CoD2 1.3's keys -> `UserCmd2` buttons, moves and weapon bytes, as
//! `CL_CreateCmd` (Mac 0x189f0e) builds them: `CL_CmdButtons` (Mac
//! 0x188ee2) for the held keys, `CL_KeyMove` (Mac 0x18996e) for the stance,
//! the sight and the move keys, and the cgame's `cg_weaponSelect` (cg
//! +0x2be50) and equipped offhand (cg +0x2be70) for the two weapon bytes
//! (`CG_DrawActive`'s `CL_SetUserCmdValue`, Mac 0x1cefca). The keys come
//! through vcod's console binds as [`Action`]s; CoD1's [`super::input`]
//! is not involved.
//!
//! Every button bit, VERIFIED off `CL_CmdButtons`' `kb[]` offsets and the
//! `IN_*` handlers `CL_InitInput` registers: `+attack` 0x1, `+melee` 0x4,
//! `+activate` 0x8, `+reload` 0x10, `+usereload` 0x20, `+leanleft` 0x40,
//! `+leanright` 0x80, `+binoculars` 0x4000, `+holdbreath` 0x8000, `+frag`
//! 0x10000, `+smoke` 0x20000, `+melee_breath` both 0x4 and 0x8000. The
//! jump 0x400 is `kb` 200, which `+gostand` and `+moveup` press only
//! standing (`IN_GoStandDown`, `IN_UpDown`); held, either also sets 0x400
//! for a noclip, UFO or spectator `pm_type`. A held `+prone` sends 0x100
//! and a held `+movedown` 0x200, each with 0x2000; otherwise `cl_stance`
//! (legacyHacks +8) 1 sends 0x200, 2 0x100. The stance binds move
//! `cl_stance` as 1.1's do ([`super::input::stance_press`]). 0x1000
//! (the sight) is `toggleads` XOR a held `+speed`; selecting another
//! weapon clears the toggle (`CL_SetADS(0)` in `CG_SelectWeaponIndex`).
//! The console or chat line adds 0x40000.
//!
//! Not modelled: `+stance`'s hold timer, the binoculars' sight, `CG_CycleWeap`'s ammo and alt-mode tests (the
//! cycle here walks the two `weaponslots`) and the offhand switch command.

use std::collections::HashSet;

use vcod_cod2::net::PlayerState2;

use super::input::{Action, StancePress, held_stance, stance_press};
use crate::console::shell::Effect;

pub mod bit {
    pub const ATTACK: i32 = 0x1;
    pub const MELEE: i32 = 0x4;
    pub const ACTIVATE: i32 = 0x8;
    pub const RELOAD: i32 = 0x10;
    pub const USE_RELOAD: i32 = 0x20;
    pub const LEAN_LEFT: i32 = 0x40;
    pub const LEAN_RIGHT: i32 = 0x80;
    pub const PRONE: i32 = 0x100;
    pub const CROUCH: i32 = 0x200;
    pub const JUMP: i32 = 0x400;
    /// The stance comes from a held `+prone` / `+movedown`.
    pub const STANCE_HELD: i32 = 0x2000;
    pub const SHELLSHOCK: i32 = 0x800;
    pub const ADS: i32 = 0x1000;
    pub const BINOCULARS: i32 = 0x4000;
    pub const HOLD_BREATH: i32 = 0x8000;
    pub const FRAG: i32 = 0x10000;
    pub const SMOKE: i32 = 0x20000;
    pub const TALK: i32 = 0x40000;
}

/// The held key each button bit follows.
const HELD: [(Action, i32); 12] = [
    (Action::Attack, bit::ATTACK),
    (Action::Melee, bit::MELEE),
    (Action::MeleeBreath, bit::MELEE | bit::HOLD_BREATH),
    (Action::Use, bit::ACTIVATE),
    (Action::Reload, bit::RELOAD),
    (Action::UseReload, bit::USE_RELOAD),
    (Action::LeanLeft, bit::LEAN_LEFT),
    (Action::LeanRight, bit::LEAN_RIGHT),
    (Action::Binoculars, bit::BINOCULARS),
    (Action::HoldBreath, bit::HOLD_BREATH),
    (Action::Frag, bit::FRAG),
    (Action::Smoke, bit::SMOKE),
];

/// `cl_stance`.
pub use super::input::Stance;

#[derive(Default)]
pub struct Cod2Input {
    down: HashSet<Action>,
    pub stance: Stance,
    /// `cl.toggleADS` (cl +0xb).
    ads_toggle: bool,
    /// The held `+gostand` / `+moveup` keys that pressed the jump (`kb`
    /// 200) rather than moving the stance.
    jump_by: HashSet<Action>,
    /// `cg_weaponSelect`, the weapon every cmd asks for.
    selected: u8,
    /// The equipped offhand, `offHandIndex`'s byte.
    offhand: u8,
    /// The `(clientNum, stats[5])` the selection was last reset for.
    spawn: Option<(i32, i32)>,
    /// `CL_SetUserCmdInShellshock` (Mac 0x17a28c): the shellshock runs,
    /// every cmd carries 0x800 (`CL_CmdButtons`, Mac 0x188ee2).
    pub shellshock: bool,
}

/// The weapons a playerstate holds, `weapons[0..1]` as one mask.
fn holds(ps: &PlayerState2, w: u8) -> bool {
    let (word, b) = (usize::from(w) / 32, u32::from(w) % 32);
    word < 2 && w != 0 && ps.i32(&format!("weapons[{word}]")) as u32 >> b & 1 != 0
}

/// `weaponslots` byte `slot` (1 primary, 2 primaryb).
fn slot_weapon(ps: &PlayerState2, slot: usize) -> u8 {
    let word = ps.i32(&format!("weaponslots[{}]", slot / 4 * 4)) as u32;
    (word >> (8 * (slot % 4))) as u8
}

impl Cod2Input {
    /// Whether `action`'s key is held (`+talk` for voice chat).
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
            self.jump_by.remove(&action);
        }
        if pressed && !was && matches!(action, Action::Jump | Action::MoveUp) {
            self.stance_press(action);
        }
    }

    /// `IN_GoStandDown`, `IN_UpDown` and the stance one-shots (Mac
    /// 0x187e98, 0x187732, 0x187d64..0x187e6e), each gated on the held
    /// `+prone` (kb 0x1f4) and `+movedown` (kb 0xdc).
    fn stance_press(&mut self, action: Action) {
        match stance_press(action, self.stance, held_stance(&self.down).is_some()) {
            StancePress::Set(s) => self.stance = s,
            StancePress::Jump => {
                self.jump_by.insert(action);
            }
            StancePress::Nothing => {}
        }
    }

    /// A one-shot bind: the stance ones, `toggleads`, `leaveads`,
    /// `weapnext`, `weapprev`, `weaponslot`. `ps` is the newest
    /// playerstate, what a weapon pick reads. False for one CoD2 has no
    /// use for.
    pub fn impulse(&mut self, action: Action, ps: Option<&PlayerState2>) -> bool {
        match action {
            Action::Crouch
            | Action::Prone
            | Action::ToggleCrouch
            | Action::ToggleProne
            | Action::LowerStance
            | Action::RaiseStance => self.stance_press(action),
            Action::ToggleAds => self.ads_toggle = !self.ads_toggle,
            Action::LeaveAds => self.ads_toggle = false,
            Action::NextWeapon | Action::PrevWeapon => {
                if let Some(ps) = ps {
                    self.cycle(ps, action == Action::NextWeapon);
                }
            }
            Action::Slot(n) => {
                if let Some(ps) = ps
                    && (1..=2).contains(&n)
                {
                    let w = slot_weapon(ps, n);
                    if w != 0 {
                        self.select(w);
                    }
                }
            }
            _ => return false,
        }
        true
    }

    /// A console effect a key's bind raised: a `+button` edge or a
    /// one-shot. True when it was the input's.
    pub fn effect(&mut self, effect: &Effect, ps: Option<&PlayerState2>) -> bool {
        match effect {
            Effect::Button(action, down) => self.key(*action, *down),
            Effect::Impulse(action) => {
                self.impulse(*action, ps);
            }
            _ => return false,
        }
        true
    }

    /// Lets go of every held key; the stance and the selection stay.
    pub fn release_all(&mut self) {
        self.down.clear();
        self.jump_by.clear();
    }

    /// `CG_SelectWeaponIndex` (Mac 0x1d7ef6): another weapon drops the
    /// sight toggle. The cycle keys and the server's `a <index>` call it.
    pub fn select(&mut self, w: u8) {
        if w != self.selected {
            self.selected = w;
            self.ads_toggle = false;
        }
    }

    /// `CG_CycleWeap` over the two weapon slots: the other slot's weapon.
    fn cycle(&mut self, ps: &PlayerState2, forward: bool) {
        let slots: Vec<u8> = [1, 2]
            .map(|s| slot_weapon(ps, s))
            .into_iter()
            .filter(|&w| w != 0)
            .collect();
        if slots.is_empty() {
            return;
        }
        let at = slots.iter().position(|&w| w == self.selected);
        let n = slots.len();
        let next = match at {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None => 0,
        };
        self.select(slots[next]);
    }

    /// Takes the newest playerstate: `CG_Respawn` (a new
    /// `(clientNum, stats[5])`, `CG_SetNextSnap` Mac 0x1dee08) sets the
    /// selection to `ps.weapon` and the offhand to `ps.offHandIndex`, and
    /// a selection the playerstate no longer holds falls back to its weapon
    /// (`CG_DrawActive`'s range check, here any weapon not held).
    pub fn sync(&mut self, ps: &PlayerState2) {
        let spawn = (ps.i32("clientNum"), ps.stats[5]);
        let weapon = ps.i32("weapon") as u8;
        if self.spawn.replace(spawn) != Some(spawn) {
            self.selected = weapon;
            self.offhand = ps.i32("offHandIndex") as u8;
            self.stance = Stance::Stand;
            self.ads_toggle = false;
        } else if !holds(ps, self.selected) && weapon != 0 {
            self.selected = weapon;
        }
    }

    /// A gamestate: the next playerstate is a respawn.
    pub fn reset(&mut self) {
        self.spawn = None;
        self.release_all();
    }

    /// The cmd's `weapon` and `offHandIndex`.
    pub fn weapons(&self) -> (u8, u8) {
        (self.selected, self.offhand)
    }

    /// The cmd's buttons for a body of `pm_type`; `talking` while a key
    /// catcher (console, chat) is up.
    pub fn buttons(&self, pm_type: i32, talking: bool) -> i32 {
        let mut b = 0;
        for (a, bits) in HELD {
            if self.down.contains(&a) {
                b |= bits;
            }
        }
        let key_stance = held_stance(&self.down);
        if key_stance.is_some() {
            b |= bit::STANCE_HELD;
        }
        b |= match key_stance.unwrap_or(self.stance) {
            Stance::Crouch => bit::CROUCH,
            Stance::Prone => bit::PRONE,
            Stance::Stand => 0,
        };
        // kb 0xf0, which `+gostand` and `+moveup` hold.
        let up = self.down.contains(&Action::Jump) || self.down.contains(&Action::MoveUp);
        if !self.jump_by.is_empty() || (up && (2..=4).contains(&pm_type)) {
            b |= bit::JUMP;
        }
        if self.ads_toggle != self.down.contains(&Action::Ads) {
            b |= bit::ADS;
        }
        if talking {
            b |= bit::TALK;
        }
        if self.shellshock {
            b |= bit::SHELLSHOCK;
        }
        b
    }

    /// `forwardmove` and `rightmove`: 127 a held key, opposite keys cancel.
    pub fn moves(&self) -> (i8, i8) {
        let axis = |plus: Action, minus: Action| {
            let (p, m) = (self.down.contains(&plus), self.down.contains(&minus));
            127 * i8::from(p) - 127 * i8::from(m)
        };
        (
            axis(Action::Forward, Action::Back),
            axis(Action::Right, Action::Left),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_cod2::net::delta::field_index;
    use vcod_cod2::net::fields::PLAYER_STATE_FIELDS;

    fn ps(weapon: u32, slots: [u8; 2]) -> PlayerState2 {
        let mut p = PlayerState2::default();
        let mut set = |n: &str, v: u32| p.f[field_index(&PLAYER_STATE_FIELDS, n)] = v;
        set("weapon", weapon);
        set("weapons[0]", (1 << slots[0]) | (1 << slots[1]));
        set(
            "weaponslots[0]",
            u32::from(slots[0]) << 8 | u32::from(slots[1]) << 16,
        );
        p
    }

    /// `CL_KeyMove` (Mac 0x18996e) under a held `+prone`, and
    /// `IN_ToggleProne` / `IN_UpDown` on `cl_stance`.
    #[test]
    fn held_and_toggled_stances() {
        let mut i = Cod2Input::default();
        i.key(Action::HoldProne, true);
        assert_eq!(i.buttons(0, false), bit::PRONE | bit::STANCE_HELD);
        i.key(Action::HoldProne, false);
        assert_eq!(i.buttons(0, false), 0);
        i.impulse(Action::ToggleProne, None);
        assert_eq!(i.buttons(0, false), bit::PRONE);
        i.key(Action::MoveUp, true);
        assert_eq!(i.buttons(0, false), bit::CROUCH);
    }

    #[test]
    fn the_stance_and_the_jump() {
        let mut i = Cod2Input::default();
        i.impulse(Action::Crouch, None);
        assert_eq!(i.buttons(0, false), bit::CROUCH);
        // The first press stands up, the next jumps.
        i.key(Action::Jump, true);
        assert_eq!(i.buttons(0, false), 0);
        i.key(Action::Jump, false);
        i.key(Action::Jump, true);
        assert_eq!(i.buttons(0, false), bit::JUMP);
        i.key(Action::Jump, false);
        i.impulse(Action::Prone, None);
        assert_eq!(i.buttons(0, false), bit::PRONE);
    }

    #[test]
    fn the_sight_is_the_toggle_against_the_held_key() {
        let mut i = Cod2Input::default();
        i.impulse(Action::ToggleAds, None);
        assert_eq!(i.buttons(0, false), bit::ADS);
        i.key(Action::Ads, true);
        assert_eq!(i.buttons(0, false), 0);
        i.impulse(Action::ToggleAds, None);
        assert_eq!(i.buttons(0, false), bit::ADS);
    }

    #[test]
    fn held_keys_and_melee_breath() {
        let mut i = Cod2Input::default();
        i.key(Action::MeleeBreath, true);
        i.key(Action::Frag, true);
        i.key(Action::Attack, true);
        assert_eq!(
            i.buttons(0, true),
            bit::MELEE | bit::HOLD_BREATH | bit::FRAG | bit::ATTACK | bit::TALK
        );
        i.key(Action::Forward, true);
        i.key(Action::Left, true);
        assert_eq!(i.moves(), (127, -127));
        i.release_all();
        assert_eq!((i.buttons(0, false), i.moves()), (0, (0, 0)));
    }

    /// The selection is the client's own: a respawn seeds it from the
    /// playerstate, `weapnext` cycles the slots, and the playerstate's
    /// weapon moving on its own (the raise still running) leaves it.
    #[test]
    fn the_weapon_byte_is_the_selection() {
        let mut i = Cod2Input::default();
        let p = ps(3, [3, 9]);
        i.sync(&p);
        assert_eq!(i.weapons().0, 3);
        i.impulse(Action::ToggleAds, None);
        i.impulse(Action::NextWeapon, Some(&p));
        assert_eq!(i.weapons().0, 9);
        assert_eq!(
            i.buttons(0, false) & bit::ADS,
            0,
            "a switch leaves the sight"
        );
        i.sync(&p);
        assert_eq!(i.weapons().0, 9, "never an echo of ps.weapon");
        i.impulse(Action::NextWeapon, Some(&p));
        assert_eq!(i.weapons().0, 3);
        i.impulse(Action::Slot(2), Some(&p));
        assert_eq!(i.weapons().0, 9);
        // A respawn takes the playerstate's.
        let mut q = ps(3, [3, 9]);
        q.stats[5] = 1;
        i.sync(&q);
        assert_eq!(i.weapons().0, 3);
    }

    /// Every stock action key, from the bind through the console to the
    /// cmd: what `main.rs` routes for a CoD2 session.
    #[test]
    fn the_stock_binds_reach_the_cmd() {
        use crate::console::shell::Shell;
        let mut sh = Shell::new();
        sh.use_cod2_binds();
        let p = ps(3, [3, 9]);
        let mut i = Cod2Input::default();
        i.sync(&p);
        let mut press = |i: &mut Cod2Input, key: &str, down: bool| {
            for e in sh.key_event(key, down) {
                assert!(i.effect(&e, Some(&p)), "{key}: {e:?} not the input's");
            }
        };
        for (key, b) in [
            ("MOUSE1", bit::ATTACK),
            ("R", bit::RELOAD),
            ("SHIFT", bit::MELEE | bit::HOLD_BREATH),
            ("F", bit::ACTIVATE),
            ("Q", bit::LEAN_LEFT),
            ("E", bit::LEAN_RIGHT),
            ("G", bit::FRAG),
            ("MOUSE3", bit::FRAG),
            ("4", bit::SMOKE),
            ("B", bit::BINOCULARS),
        ] {
            press(&mut i, key, true);
            assert_eq!(i.buttons(0, false), b, "{key} held");
            press(&mut i, key, false);
            assert_eq!(i.buttons(0, false), 0, "{key} let go");
        }
        press(&mut i, "C", true);
        assert_eq!(i.buttons(0, false), bit::CROUCH);
        press(&mut i, "CTRL", true);
        assert_eq!(i.buttons(0, false), bit::PRONE);
        press(&mut i, "SPACE", true);
        press(&mut i, "SPACE", false);
        assert_eq!(i.buttons(0, false), 0, "space stands up");
        press(&mut i, "SPACE", true);
        assert_eq!(i.buttons(0, false), bit::JUMP);
        press(&mut i, "SPACE", false);
        press(&mut i, "MOUSE2", true);
        assert_eq!(i.buttons(0, false), bit::ADS);
        press(&mut i, "MOUSE2", true);
        assert_eq!(i.buttons(0, false), 0);
        for (key, w) in [("MWHEELDOWN", 9), ("MWHEELUP", 3), ("1", 9), ("2", 3)] {
            press(&mut i, key, true);
            assert_eq!(i.weapons().0, w, "{key}");
        }
        // Tab is the board's, not the cmd's.
        assert_eq!(sh.key_event("TAB", true), [Effect::Scores(true)]);
        // X asks the server for the weapon menu (`default_mp.cfg`).
        assert_eq!(
            sh.key_event("X", true),
            [Effect::OpenScriptMenu {
                menu: "ingame".into(),
                response: "changeweapon".into()
            }]
        );
    }
}
