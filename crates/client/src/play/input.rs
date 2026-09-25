//! Keys and mouse -> [`UserCmd`], retail's default binds
//! (`docs/protocol-1.1.md`, "Usercmd input bits"). Position is
//! server-authoritative; this only ever fills in axes, buttons, angles and
//! the weapon byte.

use std::collections::HashSet;
use vcod_common::net::msg::{self, PlayerState, UserCmd};
use vcod_common::net::protocol::Protocol;

/// `eFlags` bits the server writes for a crouched/prone player
/// (`crates/server/src/spectate.rs`, `to_wire`), the inverse the caller reads
/// to build [`Held::stance`] from a playerstate.
pub const EF_CROUCH: i32 = 0x20;
pub const EF_PRONE: i32 = 0x40;

/// Mouse look rate, matching `FlyCamera::mouse_delta` (`camera.rs`).
const MOUSE_SENS: f32 = 0.003;
/// ANGLE2SHORT units per radian (`65536` units per `360` degrees).
const SHORT_PER_RAD: f32 = 65536.0 / (2.0 * std::f32::consts::PI);
/// `weaponSlots` 1..=5 are the number-key slots a switch cycles through
/// (`docs/protocol-1.1.md`); 0 and 6/7 are not.
const CYCLE_SLOTS: std::ops::RangeInclusive<usize> = 1..=5;

/// A retail default bind. `Slot(n)` is a number key, `n` the weapon slot it
/// requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Forward,
    Back,
    Left,
    Right,
    Jump,
    Crouch,
    Prone,
    LeanLeft,
    LeanRight,
    Attack,
    Ads,
    Melee,
    Use,
    Reload,
    Slot(usize),
    NextWeapon,
    PrevWeapon,
}

/// The client's own stance, held level on the wire
/// (`docs/protocol-1.1.md`, "Usercmd input bits").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Stance {
    #[default]
    Stand,
    Crouch,
    Prone,
}

impl Stance {
    /// The inverse of `spectate.rs`'s `to_wire` write of [`EF_CROUCH`] /
    /// [`EF_PRONE`], for the caller building [`Held`] from a playerstate.
    pub fn from_eflags(e_flags: i32) -> Stance {
        if e_flags & EF_PRONE != 0 {
            Stance::Prone
        } else if e_flags & EF_CROUCH != 0 {
            Stance::Crouch
        } else {
            Stance::Stand
        }
    }
}

/// The bit of the newest playerstate a cmd needs to build against: the
/// weapon held, the eight weapon slots (`weaponslots[0]`/`weaponslots[4]`
/// unpacked little-endian, the inverse of
/// `vcod_common::pmove::weapon::pack_slots`), and the stance
/// ([`Stance::from_eflags`]). The default (weapon 0, no slots, standing)
/// stands in before the first snapshot.
#[derive(Default)]
pub struct Held {
    pub weapon: u8,
    pub slots: [u8; 8],
    pub stance: Stance,
}

impl Held {
    pub fn from_ps(ps: &PlayerState, p: &Protocol) -> Held {
        let lo = ps.field_i32(p, "weaponslots[0]").to_le_bytes();
        let hi = ps.field_i32(p, "weaponslots[4]").to_le_bytes();
        let mut slots = [0u8; 8];
        slots[..4].copy_from_slice(&lo);
        slots[4..].copy_from_slice(&hi);
        Held {
            weapon: ps.field_i32(p, "weapon") as u8,
            slots,
            stance: Stance::from_eflags(ps.field_i32(p, "eFlags")),
        }
    }
}

/// A weapon-select bind not yet resolved against a playerstate (`build`
/// needs `Held` to know what a slot or a next/prev step means).
#[derive(Clone, Copy)]
enum WeaponRequest {
    Slot(usize),
    Next,
    Prev,
}

/// The slot 1..=5 that holds `held.weapon`, or 1 if none does (nothing is
/// bound to slot 0, so this is only reached at all for a weapon that fell
/// out of the cycle range).
fn current_slot(held: &Held) -> usize {
    CYCLE_SLOTS
        .clone()
        .find(|&n| held.slots[n] == held.weapon)
        .unwrap_or(1)
}

/// Next (or, `forward` false, previous) nonzero slot from the current one,
/// wrapping through 1..=5. 0 if none of the other four are set.
fn step_slot(held: &Held, forward: bool) -> u8 {
    let mut n = current_slot(held);
    for _ in 0..5 {
        n = match forward {
            true if n == *CYCLE_SLOTS.end() => *CYCLE_SLOTS.start(),
            true => n + 1,
            false if n == *CYCLE_SLOTS.start() => *CYCLE_SLOTS.end(),
            false => n - 1,
        };
        if held.slots[n] != 0 {
            return held.slots[n];
        }
    }
    0
}

/// Accumulates held keys, mouse motion and a pending weapon switch between
/// `build` calls, and turns them into one [`UserCmd`] per call.
#[derive(Default)]
pub struct PlayInput {
    down: HashSet<Action>,
    /// The stance the client wants; level, and resynced to `Held::stance`
    /// whenever the server changes it without a matching key press (a
    /// forced stand at spawn, say) so the client does not fight it.
    wanted_stance: Stance,
    last_seen_stance: Stance,
    pending_request: Option<WeaponRequest>,
    /// A switch in flight: the cmd carries this weapon until the playerstate
    /// reports it, then this clears and the cmd follows `Held::weapon` again.
    pending_weapon: Option<u8>,
    /// True for the one Jump press that stood the client up from crouch or
    /// prone, so that same press does not also raise `up` as a jump.
    jump_consumed_by_stand: bool,
    /// Raw view angles, ANGLE2SHORT units, accumulated from mouse counts;
    /// pitch (index 0) down-positive, yaw (index 1) as `FlyCamera`'s own
    /// sign. Index 2 (roll) is unused. No clamp: the server clamps nothing
    /// and stage 3's pmove owns any clamp.
    raw_angles: [i32; 3],
}

impl PlayInput {
    pub fn key(&mut self, action: Action, pressed: bool) {
        let was_down = self.down.contains(&action);
        if pressed {
            self.down.insert(action);
        } else {
            self.down.remove(&action);
            if action == Action::Jump {
                self.jump_consumed_by_stand = false;
            }
        }
        if pressed && !was_down {
            self.on_press(action);
        }
    }

    fn on_press(&mut self, action: Action) {
        match action {
            Action::Crouch => {
                self.wanted_stance = match self.wanted_stance {
                    Stance::Stand | Stance::Prone => Stance::Crouch,
                    Stance::Crouch => Stance::Stand,
                };
            }
            Action::Prone => {
                self.wanted_stance = match self.wanted_stance {
                    Stance::Prone => Stance::Stand,
                    Stance::Stand | Stance::Crouch => Stance::Prone,
                };
            }
            Action::Jump => {
                if self.wanted_stance != Stance::Stand {
                    self.wanted_stance = Stance::Stand;
                    self.jump_consumed_by_stand = true;
                }
            }
            Action::Slot(n) => self.pending_request = Some(WeaponRequest::Slot(n)),
            Action::NextWeapon => self.pending_request = Some(WeaponRequest::Next),
            Action::PrevWeapon => self.pending_request = Some(WeaponRequest::Prev),
            _ => {}
        }
    }

    /// Lets go of every held key, for a grab release or focus loss. The
    /// stance and a pending switch stay: both are toggles, not holds.
    pub fn release_all(&mut self) {
        self.down.clear();
        self.jump_consumed_by_stand = false;
    }

    pub fn mouse(&mut self, dx: f32, dy: f32) {
        // Wire pitch is down-positive; `FlyCamera::mouse_delta` is
        // up-positive (`pitch -= dy * SENS`), so this accumulator takes the
        // opposite sign on dy. Yaw keeps `FlyCamera`'s own sign.
        self.raw_angles[0] += (dy * MOUSE_SENS * SHORT_PER_RAD) as i32;
        self.raw_angles[1] += (-dx * MOUSE_SENS * SHORT_PER_RAD) as i32;
    }

    pub fn raw_angles(&self) -> [i32; 3] {
        self.raw_angles
    }

    fn resolve_pending(&mut self, held: &Held) {
        if let Some(req) = self.pending_request.take() {
            let candidate = match req {
                WeaponRequest::Slot(n) => held.slots.get(n).copied().unwrap_or(0),
                WeaponRequest::Next => step_slot(held, true),
                WeaponRequest::Prev => step_slot(held, false),
            };
            if candidate != 0 && candidate != held.weapon {
                self.pending_weapon = Some(candidate);
            }
        }
        // Read, or no longer held at all (a death, a map change): either way
        // the cmd goes back to following the playerstate.
        if self
            .pending_weapon
            .is_some_and(|w| w == held.weapon || !held.slots.contains(&w))
        {
            self.pending_weapon = None;
        }
    }

    pub fn build(&mut self, server_time: i32, held: &Held) -> UserCmd {
        if held.stance != self.last_seen_stance {
            self.wanted_stance = held.stance;
        }
        self.last_seen_stance = held.stance;

        self.resolve_pending(held);
        let weapon = self.pending_weapon.unwrap_or(held.weapon);

        let axis = |pos: bool, neg: bool| (pos as i32 - neg as i32) as i8 * 127;

        let mut wbuttons = 0u8;
        let mut up = 0i8;
        match self.wanted_stance {
            Stance::Crouch => {
                wbuttons |= msg::WBUTTON_CROUCH;
                up = -127;
            }
            Stance::Prone => {
                wbuttons |= msg::WBUTTON_PRONE;
                up = -127;
            }
            Stance::Stand => {
                if self.down.contains(&Action::Jump) && !self.jump_consumed_by_stand {
                    up = 127;
                }
            }
        }
        if self.down.contains(&Action::LeanLeft) {
            wbuttons |= msg::WBUTTON_LEAN_LEFT;
        }
        if self.down.contains(&Action::LeanRight) {
            wbuttons |= msg::WBUTTON_LEAN_RIGHT;
        }
        if self.down.contains(&Action::Reload) {
            wbuttons |= msg::WBUTTON_RELOAD;
        }

        let mut buttons = 0u8;
        if self.down.contains(&Action::Attack) {
            buttons |= msg::BUTTON_ATTACK;
        }
        if self.down.contains(&Action::Ads) {
            buttons |= msg::BUTTON_ADS;
        }
        if self.down.contains(&Action::Melee) {
            buttons |= msg::BUTTON_MELEE;
        }
        if self.down.contains(&Action::Use) {
            buttons |= msg::BUTTON_USE;
        }

        UserCmd {
            server_time,
            buttons,
            wbuttons,
            weapon,
            angles: self.raw_angles,
            forward: axis(
                self.down.contains(&Action::Forward),
                self.down.contains(&Action::Back),
            ),
            right: axis(
                self.down.contains(&Action::Right),
                self.down.contains(&Action::Left),
            ),
            up,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(weapon: u8) -> Held {
        Held {
            weapon,
            slots: [0, 10, 0, 3, 6, 0, 0, 0],
            stance: Stance::Stand,
        }
    }

    #[test]
    fn the_cmd_carries_the_held_weapon() {
        let mut i = PlayInput::default();
        assert_eq!(i.build(100, &held(10)).weapon, 10);
    }

    #[test]
    fn switch_holds_until_the_playerstate_reads_it() {
        let mut i = PlayInput::default();
        i.key(Action::Slot(3), true);
        assert_eq!(i.build(100, &held(10)).weapon, 3);
        assert_eq!(i.build(108, &held(10)).weapon, 3);
        assert_eq!(i.build(116, &held(3)).weapon, 3);
        // Released: follows the playerstate again.
        assert_eq!(i.build(124, &held(10)).weapon, 10);
    }

    #[test]
    fn a_switch_to_a_weapon_no_longer_held_is_dropped() {
        let mut i = PlayInput::default();
        i.key(Action::Slot(3), true);
        assert_eq!(i.build(100, &held(10)).weapon, 3);
        // A map change: no snapshot yet, then a loadout without the colt.
        assert_eq!(i.build(108, &Held::default()).weapon, 0);
        let other = Held {
            weapon: 12,
            slots: [0, 12, 0, 0, 0, 0, 0, 0],
            stance: Stance::Stand,
        };
        assert_eq!(i.build(116, &other).weapon, 12);
    }

    #[test]
    fn empty_slot_requests_nothing() {
        let mut i = PlayInput::default();
        i.key(Action::Slot(2), true);
        assert_eq!(i.build(100, &held(10)).weapon, 10);
    }

    #[test]
    fn crouch_toggles_and_holds_up_down() {
        let mut i = PlayInput::default();
        i.key(Action::Crouch, true);
        i.key(Action::Crouch, false);
        let c = i.build(100, &held(10));
        assert_ne!(c.wbuttons & msg::WBUTTON_CROUCH, 0);
        assert_eq!(c.up, -127);
        i.key(Action::Crouch, true);
        i.key(Action::Crouch, false);
        let c = i.build(108, &held(10));
        assert_eq!(c.wbuttons & msg::WBUTTON_CROUCH, 0);
        assert_eq!(c.up, 0);
    }

    #[test]
    fn space_stands_from_prone_and_jumps_when_standing() {
        let mut i = PlayInput::default();
        i.key(Action::Prone, true);
        i.key(Action::Prone, false);
        i.key(Action::Jump, true);
        let c = i.build(100, &held(10));
        assert_eq!(c.wbuttons & msg::WBUTTON_PRONE, 0);
        assert_eq!(c.up, 0);
        i.key(Action::Jump, false);
        i.key(Action::Jump, true);
        assert_eq!(i.build(108, &held(10)).up, 127);
    }

    #[test]
    fn mouse_turns_raw_angles() {
        let mut i = PlayInput::default();
        i.mouse(100.0, 0.0);
        let yaw = i.build(100, &held(10)).angles[1];
        assert_ne!(yaw, 0);
    }

    #[test]
    fn prone_sets_the_wire_bit_directly() {
        let mut i = PlayInput::default();
        i.key(Action::Prone, true);
        let c = i.build(100, &held(10));
        assert_ne!(c.wbuttons & msg::WBUTTON_PRONE, 0);
        assert_eq!(c.up, -127);
    }

    #[test]
    fn buttons_and_lean_and_reload_are_level() {
        let mut i = PlayInput::default();
        i.key(Action::Attack, true);
        i.key(Action::Ads, true);
        i.key(Action::Melee, true);
        i.key(Action::Use, true);
        i.key(Action::Reload, true);
        i.key(Action::LeanLeft, true);
        let c = i.build(100, &held(10));
        assert_eq!(
            c.buttons,
            msg::BUTTON_ATTACK | msg::BUTTON_ADS | msg::BUTTON_MELEE | msg::BUTTON_USE
        );
        assert_eq!(c.wbuttons, msg::WBUTTON_RELOAD | msg::WBUTTON_LEAN_LEFT);
        i.key(Action::Attack, false);
        i.key(Action::LeanLeft, false);
        i.key(Action::LeanRight, true);
        let c = i.build(108, &held(10));
        assert_eq!(
            c.buttons,
            msg::BUTTON_ADS | msg::BUTTON_MELEE | msg::BUTTON_USE
        );
        assert_eq!(c.wbuttons, msg::WBUTTON_RELOAD | msg::WBUTTON_LEAN_RIGHT);
    }

    #[test]
    fn opposite_axes_cancel() {
        let mut i = PlayInput::default();
        i.key(Action::Forward, true);
        i.key(Action::Back, true);
        i.key(Action::Left, true);
        let c = i.build(100, &held(10));
        assert_eq!(c.forward, 0);
        assert_eq!(c.right, -127);
    }

    #[test]
    fn next_and_prev_weapon_walk_nonzero_slots_wrapping() {
        // slot 4 holds 3, slot 5 holds 6, slots 2..3 empty, slot 1 holds 10.
        let mut i = PlayInput::default();
        i.key(Action::NextWeapon, true);
        assert_eq!(i.build(100, &held(10)).weapon, 3);
        let mut i = PlayInput::default();
        i.key(Action::PrevWeapon, true);
        assert_eq!(i.build(100, &held(10)).weapon, 6);
    }

    #[test]
    fn a_server_forced_stance_change_is_adopted() {
        let mut i = PlayInput::default();
        i.key(Action::Crouch, true);
        i.key(Action::Crouch, false);
        let crouched = Held {
            weapon: 10,
            slots: [0, 10, 0, 3, 6, 0, 0, 0],
            stance: Stance::Crouch,
        };
        let c = i.build(100, &crouched);
        assert_ne!(c.wbuttons & msg::WBUTTON_CROUCH, 0);
        // The server stands the client up on its own (a respawn); no key
        // press asked for it, and the client must not fight it back.
        let stood = Held {
            weapon: 10,
            slots: [0, 10, 0, 3, 6, 0, 0, 0],
            stance: Stance::Stand,
        };
        let c = i.build(108, &stood);
        assert_eq!(c.wbuttons & msg::WBUTTON_CROUCH, 0);
    }

    #[test]
    fn held_unpacks_the_playerstate() {
        let p = &vcod_common::net::protocol::PROTOCOL_V1;
        let mut ps = PlayerState::null(p);
        let mut set = |name: &str, v: i32| {
            ps.fields[PlayerState::field_index(p, name).unwrap()] = v;
        };
        set("weapon", 10);
        let [lo, hi] = vcod_common::pmove::weapon::pack_slots(&[0, 10, 0, 3, 6, 0, 0, 7]);
        set("weaponslots[0]", lo);
        set("weaponslots[4]", hi);
        set("eFlags", EF_PRONE);
        let h = Held::from_ps(&ps, p);
        assert_eq!(h.weapon, 10);
        assert_eq!(h.slots, [0, 10, 0, 3, 6, 0, 0, 7]);
        assert_eq!(h.stance, Stance::Prone);
    }

    #[test]
    fn release_all_lets_go_of_keys_but_keeps_the_stance() {
        let mut i = PlayInput::default();
        i.key(Action::Forward, true);
        i.key(Action::Attack, true);
        i.key(Action::Crouch, true);
        i.release_all();
        let c = i.build(100, &held(10));
        assert_eq!((c.forward, c.buttons), (0, 0));
        assert_ne!(c.wbuttons & msg::WBUTTON_CROUCH, 0);
    }

    #[test]
    fn stance_from_eflags_round_trips() {
        assert_eq!(Stance::from_eflags(0), Stance::Stand);
        assert_eq!(Stance::from_eflags(EF_CROUCH), Stance::Crouch);
        assert_eq!(Stance::from_eflags(EF_PRONE), Stance::Prone);
    }
}
