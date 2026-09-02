//! Means of death: the enum the engine converts a script's `MOD_*` string
//! into, and which of its values the obituary tags.

/// The 25 names, in the order of the pointer table at `.so` file offset
/// `0x7cda0` (`docs/research/cod11-hud-protocol.md` section 2). The index is
/// the enum value the obituary encodes.
pub const MOD_NAMES: [&str; 25] = [
    "MOD_UNKNOWN",
    "MOD_PISTOL_BULLET",
    "MOD_RIFLE_BULLET",
    "MOD_GRENADE",
    "MOD_GRENADE_SPLASH",
    "MOD_PROJECTILE",
    "MOD_PROJECTILE_SPLASH",
    "MOD_MELEE",
    "MOD_HEAD_SHOT",
    "MOD_MORTAR",
    "MOD_MORTAR_SPLASH",
    "MOD_KICKED",
    "MOD_GRABBER",
    "MOD_DYNAMITE",
    "MOD_DYNAMITE_SPLASH",
    "MOD_AIRSTRIKE",
    "MOD_WATER",
    "MOD_SLIME",
    "MOD_LAVA",
    "MOD_CRUSH",
    "MOD_TELEFRAG",
    "MOD_FALLING",
    "MOD_SUICIDE",
    "MOD_TRIGGER_HURT",
    "MOD_EXPLOSIVE",
];

/// The seven means of death the obituary sends as `0x80 | mod` instead of a
/// weapon index; every other one sends the weapon's configstring 7 index
/// (hud protocol doc section 2, "Which deaths get the `0x80` flag").
pub const MOD_FLAGGED: [i32; 7] = [7, 8, 16, 17, 19, 21, 22];

/// The enum value of a `MOD_*` name, or `None` for a name the table has no
/// row for. Script string values intern exactly, not folded
/// (`docs/research/cod11-gsc-language.md`), and every stock call site spells
/// the name the way the table does.
pub fn mod_index(name: &str) -> Option<i32> {
    MOD_NAMES.iter().position(|n| *n == name).map(|i| i as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flagged_mods_are_the_seven_the_builtin_tags() {
        assert_eq!(mod_index("MOD_MELEE"), Some(7));
        assert_eq!(mod_index("MOD_HEAD_SHOT"), Some(8));
        assert_eq!(mod_index("MOD_SUICIDE"), Some(22));
        assert_eq!(mod_index("MOD_EXPLOSIVE"), Some(24));
        assert_eq!(mod_index("MOD_NOT_A_THING"), None);
        for m in MOD_FLAGGED {
            assert!(MOD_NAMES.get(m as usize).is_some());
        }
        // The two the client draws a weapon icon for, not a MOD icon.
        assert!(!MOD_FLAGGED.contains(&mod_index("MOD_RIFLE_BULLET").unwrap()));
        assert!(!MOD_FLAGGED.contains(&mod_index("MOD_TRIGGER_HURT").unwrap()));
    }
}
