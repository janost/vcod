//! The binds of the offline modes, `vcod <map>`'s fly camera and `--walk`:
//! a key reaches them through the console's bind table
//! (`console::shell::Shell::key_event`), as it reaches a session, and its
//! button or one-shot lands in one [`KeyState`].

use crate::camera::InputState;
use crate::play::input::{Action, KeyState, StanceBits};
use vcod_common::pmove::PmInput;

/// The held binds and the kept stance (`cl_stance`) of an offline mode.
pub(crate) type OfflineKeys = KeyState<()>;

const HELD: u8 = 1;
const CROUCH: u8 = 2;
const PRONE: u8 = 4;
const JUMP: u8 = 8;
const ADS: u8 = 16;
const BITS: StanceBits<u8> = StanceBits {
    held: HELD,
    crouch: CROUCH,
    prone: PRONE,
    jump: JUMP,
    ads: ADS,
};

/// A one-shot bind: the stance ones and `toggleads`/`leaveads` land in
/// `keys`; walk's weapon picks come back as a [`WeaponPick`].
pub(crate) fn impulse(keys: &mut OfflineKeys, action: Action) -> Option<WeaponPick> {
    if keys.stance_or_sight(action) {
        return None;
    }
    match action {
        Action::Slot(n) => n.checked_sub(1).map(WeaponPick::Slot),
        Action::NextWeapon => Some(WeaponPick::Next),
        Action::PrevWeapon => Some(WeaponPick::Prev),
        _ => None,
    }
}

/// Walk's weapon pick: `weaponslot N` takes loadout entry N - 1,
/// `weapnext`/`weapprev` step through the loadout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WeaponPick {
    Slot(usize),
    Next,
    Prev,
}

impl WeaponPick {
    /// The loadout index the pick asks for, from `current` of `len`.
    pub(crate) fn index(self, current: usize, len: usize) -> usize {
        match self {
            WeaponPick::Slot(i) => i,
            WeaponPick::Next => (current + 1) % len,
            WeaponPick::Prev => (current + len - 1) % len,
        }
    }
}

/// The fly camera's keys. Rise and sink as a spectator's `upmove`
/// (`CL_KeyMove`, CoDMP.exe 0x40afc2..0x40b010): a held `+gostand` or
/// `+moveup` rises, a crouched or prone stance, kept or held, sinks.
/// `+sprint` (CoD4's `+breath_sprint`) boosts.
pub(crate) fn fly(keys: &OfflineKeys) -> InputState {
    let bits = keys.stance_and_jump_bits(vcod_common::net::flags::PM_SPECTATOR, &BITS);
    let [forward, right] = keys.move_axes();
    InputState {
        forward: forward > 0,
        back: forward < 0,
        right: right > 0,
        left: right < 0,
        up: bits & JUMP != 0,
        down: bits & (CROUCH | PRONE) != 0,
        boost: keys.held(Action::Sprint) || keys.held(Action::BreathSprint),
    }
}

/// Walk's frame input from the binds, as a cmd's keys feed pmove: the move
/// axes, the stance and its held bit, the jump, the leans. Whether the
/// sight is up comes back too (`+speed` held, or `toggleads`).
pub(crate) fn walk(keys: &OfflineKeys, input: &mut PmInput) -> bool {
    let bits = keys.stance_and_jump_bits(vcod_common::net::flags::PM_NORMAL, &BITS);
    let [forward, right] = keys.move_axes();
    input.forward = f32::from(forward.signum());
    input.right = f32::from(right.signum());
    input.jump = bits & JUMP != 0;
    input.crouch = bits & CROUCH != 0;
    input.prone = bits & PRONE != 0;
    input.stance_held = bits & HELD != 0;
    input.lean_left = keys.held(Action::LeanLeft);
    input.lean_right = keys.held(Action::LeanRight);
    bits & ADS != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::input::Stance;

    #[test]
    fn fly_moves_on_the_move_binds_and_boosts_on_sprint() {
        let mut k = OfflineKeys::default();
        k.key(Action::Forward, true);
        k.key(Action::Left, true);
        k.key(Action::Sprint, true);
        let i = fly(&k);
        assert!(i.forward && i.left && i.boost);
        assert!(!i.back && !i.right && !i.up && !i.down);
        k.key(Action::Forward, false);
        k.key(Action::Sprint, false);
        let i = fly(&k);
        assert!(!i.forward && !i.boost && i.left);
    }

    /// `gocrouch` keeps the camera sinking until `+gostand` stands it, as
    /// a spectator's kept stance does; the held `+gostand` then rises.
    #[test]
    fn fly_sinks_on_a_kept_stance_and_rises_on_gostand() {
        let mut k = OfflineKeys::default();
        assert_eq!(impulse(&mut k, Action::Crouch), None);
        assert!(fly(&k).down);
        k.key(Action::Jump, true);
        let i = fly(&k);
        assert!(i.up && !i.down);
        assert_eq!(k.stance, Stance::Stand);
        k.key(Action::Jump, false);
        let i = fly(&k);
        assert!(!i.up && !i.down);
        k.key(Action::MoveDown, true);
        assert!(fly(&k).down);
    }

    #[test]
    fn walk_takes_the_stance_jump_and_sight_off_the_binds() {
        let mut k = OfflineKeys::default();
        let mut i = PmInput::default();
        k.key(Action::Back, true);
        k.key(Action::LeanRight, true);
        k.key(Action::Jump, true);
        assert!(!walk(&k, &mut i));
        assert_eq!((i.forward, i.right), (-1.0, 0.0));
        assert!(i.jump && i.lean_right && !i.crouch && !i.prone);
        k.key(Action::Jump, false);
        impulse(&mut k, Action::Prone);
        k.key(Action::Ads, true);
        assert!(walk(&k, &mut i));
        assert!(!i.jump && i.prone && !i.stance_held);
        // A held `+movedown` crouches over the kept prone and says so.
        k.key(Action::MoveDown, true);
        walk(&k, &mut i);
        assert!(i.crouch && !i.prone && i.stance_held);
    }

    #[test]
    fn walk_picks_weapons_off_weaponslot_and_the_cycle() {
        let mut k = OfflineKeys::default();
        assert_eq!(impulse(&mut k, Action::Slot(3)), Some(WeaponPick::Slot(2)));
        assert_eq!(impulse(&mut k, Action::Slot(0)), None);
        assert_eq!(WeaponPick::Next.index(6, 7), 0);
        assert_eq!(WeaponPick::Prev.index(0, 7), 6);
    }
}
