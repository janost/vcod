//! The weapon half of a player's state: which weapons a client holds, which
//! one sits in each weapon slot, and which one it spawned holding. The stock
//! spawn builtins (`giveWeapon`, `setSpawnWeapon`) write it and
//! `ClientSim::to_wire` renders it into `ps.weapons`, `ps.weaponslots` and
//! `ps.weapon`.

use vcod_common::pk3::Pk3Fs;

/// The retail `weaponSlot` name table, `.data` 0x7c940. Index 0 is `"none"`,
/// so a weapon's own slot is 1..=5. Object model doc, section 20.
pub const SLOT_NAMES: [&str; 6] = [
    "none",
    "primary",
    "primaryb",
    "pistol",
    "grenade",
    "smokegrenade",
];

/// `ps.weaponslots` is eight bytes carried by the two 32-bit netfields
/// `weaponslots[0]` and `weaponslots[4]`.
pub const NUM_SLOTS: usize = 8;

/// One player's weapons, in the wire's own terms.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct PlayerWeapons {
    /// `ps.weapons`: bit N set for weapon index N, the 1-based configstring 7
    /// position. Two 32-bit netfields, so bit 63 is the highest that travels.
    /// Decoded against the retail captures in the object model doc, section 20.
    pub held: u64,
    /// `ps.weaponslots`: the weapon index occupying each slot, 0 for empty.
    pub slots: [u8; NUM_SLOTS],
    /// `ps.weapon`, the weapon `setSpawnWeapon` chose.
    pub current: u8,
}

impl PlayerWeapons {
    /// `giveWeapon`: hold the weapon and put it in its file's `weaponSlot`.
    /// A weapon whose file names no slot — including every weapon on a host
    /// with no paks mounted — is still held; only the slot byte goes unset.
    pub fn give(&mut self, index: usize, slot: usize) {
        if index < u64::BITS as usize {
            self.held |= 1u64 << index;
        }
        if slot > 0 && slot < NUM_SLOTS {
            self.slots[slot] = index as u8;
        }
    }

    /// Whether the player holds that weapon: retail's
    /// `COM_BitCheck(ps.weapons, index)`.
    pub fn holds(&self, index: usize) -> bool {
        index < u64::BITS as usize && self.held & (1u64 << index) != 0
    }

    /// The two 32-bit words `weaponslots[0]` and `weaponslots[4]` carry.
    pub fn slot_words(&self) -> [i32; 2] {
        let word = |b: &[u8]| i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        [word(&self.slots[0..4]), word(&self.slots[4..8])]
    }
}

/// The `weaponSlot` a weapon file names, as its index in [`SLOT_NAMES`].
/// `None` when there are no paks to read, when the file has no `weaponSlot`
/// key, or when it names a slot the table does not have.
pub fn weapon_slot(fs: Option<&Pk3Fs>, name: &str) -> Option<usize> {
    let bytes = fs?.read(&format!("weapons/mp/{name}"))?;
    let map = vcod_common::xmodel::parse_weapon(&String::from_utf8_lossy(&bytes));
    let named = map.get("weaponSlot")?;
    SLOT_NAMES
        .iter()
        .position(|s| *s == named.as_str())
        .filter(|i| *i > 0)
}

/// The `weaponClass` a weapon file names, lowercased. This is the value the
/// `weaponclass` condition in `mp/playeranim.script` tests against. `None`
/// when there are no paks to read, or when the file has no `weaponClass` key.
pub fn weapon_class(fs: Option<&Pk3Fs>, name: &str) -> Option<String> {
    let bytes = fs?.read(&format!("weapons/mp/{name}"))?;
    let map = vcod_common::xmodel::parse_weapon(&String::from_utf8_lossy(&bytes));
    map.get("weaponClass").map(|c| c.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configstrings::weapon_index;

    /// The wire words the `playerstate_ab` captures pin, decoded from
    /// `crates/server/tests/fixtures/playerstate/mp_carentan-dm.txt`: an
    /// americans join holds `colt_mp` (4), `fraggrenade_mp` (8) and
    /// `m1carbine_mp` (12), and the slot bytes put the carbine in `primary`
    /// and the colt in `pistol`.
    #[test]
    fn the_wire_words_match_the_retail_capture() {
        let mut w = PlayerWeapons::default();
        w.give(weapon_index("colt_mp").unwrap(), 3);
        w.give(weapon_index("fraggrenade_mp").unwrap(), 4);
        w.give(weapon_index("m1carbine_mp").unwrap(), 1);
        w.current = weapon_index("m1carbine_mp").unwrap() as u8;

        assert_eq!(w.held as u32 as i32, 4368);
        assert_eq!(w.slot_words(), [67111936, 8]);
        assert_eq!(w.current, 12);
    }

    /// The russian branch of the same capture: `luger_mp` (11),
    /// `rgd-33russianfrag_mp` (27) and `mosin_nagant_mp` (18).
    #[test]
    fn the_other_nationality_matches_too() {
        let mut w = PlayerWeapons::default();
        w.give(weapon_index("luger_mp").unwrap(), 3);
        w.give(weapon_index("rgd-33russianfrag_mp").unwrap(), 4);
        w.give(weapon_index("mosin_nagant_mp").unwrap(), 1);
        assert_eq!(w.held as u32 as i32, 134481920);
        assert_eq!(w.slot_words(), [184553984, 27]);
    }

    /// A weapon with no slot is still held: nothing but the slot byte depends
    /// on the weapon file, so a host with no paks mounted still reports the
    /// right `ps.weapons`.
    #[test]
    fn a_weapon_with_no_slot_is_still_held() {
        let mut w = PlayerWeapons::default();
        w.give(weapon_index("colt_mp").unwrap(), 0);
        assert_eq!(w.held, 1 << 4);
        assert_eq!(w.slots, [0; NUM_SLOTS]);
    }

    /// The three slots the stock loadout fills, read out of the shipped
    /// weapon files. Needs the paks; without them there is nothing to read.
    #[test]
    fn the_stock_loadout_slots_come_from_the_weapon_files() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let fs = Some(&fs);
        assert_eq!(weapon_slot(fs, "m1carbine_mp"), Some(1));
        assert_eq!(weapon_slot(fs, "colt_mp"), Some(3));
        assert_eq!(weapon_slot(fs, "fraggrenade_mp"), Some(4));
        assert_eq!(weapon_slot(fs, "no_such_weapon_mp"), None);
    }

    /// The `weaponclass` condition in `mp/playeranim.script` is this field.
    /// Values read straight out of the shipped weapon files.
    #[test]
    fn a_weapon_file_names_its_class() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let fs = Some(&fs);
        assert_eq!(weapon_class(fs, "m1carbine_mp").as_deref(), Some("rifle"));
        assert_eq!(weapon_class(fs, "colt_mp").as_deref(), Some("pistol"));
        assert_eq!(weapon_class(fs, "thompson_mp").as_deref(), Some("smg"));
        assert_eq!(
            weapon_class(fs, "fraggrenade_mp").as_deref(),
            Some("grenade")
        );
        assert_eq!(
            weapon_class(fs, "panzerfaust_mp").as_deref(),
            Some("rocketlauncher")
        );
        assert_eq!(weapon_class(fs, "no_such_weapon_mp"), None);
    }

    /// A host with no paks mounted still runs; every condition that reads the
    /// class simply fails to match.
    #[test]
    fn a_weapon_class_needs_no_paks_to_be_asked_for() {
        assert_eq!(weapon_class(None, "m1carbine_mp"), None);
    }
}
