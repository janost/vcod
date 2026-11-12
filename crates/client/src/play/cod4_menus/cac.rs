//! The stat commands of Create a Class (cod4-front-end.md 4.1): `statset`
//! and the menus' `statSetUsingTable`, `statClearBitMask` and
//! `statClearPerkNew`, each as `iw3mp.exe` 1.7 runs it, into the
//! profile's block. Where retail `Com_Error`s (`ERR_DROP`) vcod logs the
//! same message and leaves the stat alone.

use vcod_cod4::hud::Table;
use vcod_cod4::menuexp::atoi;

use super::ScriptMenus;

/// The create-a-class item table (0x724bb8, `tools/re/cod4_cac_items.py`):
/// an item's `mp/statstable.csv` number, the rank it unlocks at and the
/// slots it may fill (`SLOT_*` bits).
const ITEMS: [(i32, i32, i32); 61] = [
    (0, 0, 0x2),
    (1, 15, 0x2),
    (2, 0, 0x2),
    (3, 42, 0x2),
    (4, 54, 0x2),
    (10, 0, 0x1),
    (11, 0, 0x1),
    (12, 12, 0x1),
    (13, 27, 0x1),
    (14, 39, 0x1),
    (20, 0, 0x1),
    (21, 45, 0x1),
    (22, 51, 0x1),
    (23, 24, 0x1),
    (24, 36, 0x1),
    (25, 0, 0x1),
    (26, 9, 0x1),
    (50, 0, 0x4),
    (55, 0, 0x4),
    (60, 21, 0x1),
    (61, 0, 0x1),
    (62, 48, 0x1),
    (64, 33, 0x1),
    (65, 6, 0x1),
    (70, 30, 0x1),
    (71, 0, 0x1),
    (80, 0, 0x1),
    (81, 0, 0x1),
    (82, 18, 0x1),
    (90, 0, 0x4),
    (91, 22, 0x4),
    (100, 0, 0x20),
    (101, 0, 0x20),
    (102, 0, 0x20),
    (103, 0, 0x20),
    (150, 34, 0x10),
    (151, 10, 0x8),
    (152, 25, 0x10),
    (153, 43, 0x10),
    (154, 0, 0x10),
    (155, 13, 0x4),
    (156, 0, 0x8),
    (157, 7, 0x10),
    (158, 16, 0x10),
    (160, 0, 0x8),
    (161, 0, 0x10),
    (162, 0, 0x10),
    (163, 28, 0x8),
    (164, 19, 0x8),
    (165, 31, 0x4),
    (166, 37, 0x8),
    (167, 0, 0x8),
    (173, 40, 0x4),
    (176, 0, 0x4),
    (184, 0, 0x4),
    (185, 22, 0x4),
    (186, 0, 0x4),
    (190, 0, 0x4),
    (191, 0, 0x4),
    (192, 0, 0x4),
    (193, 0, 0x4),
];

/// The slot bits of [`ITEMS`], by a class stat's last digit (0x4fc4a6's
/// jump table): 1 primary, 3 secondary, 5..7 the perks, 8 the special
/// grenade; the attachments, the camo and the class's first stat are not
/// checked.
fn slot_bits(digit: i32, overkill: bool) -> Option<i32> {
    Some(match digit {
        1 => 0x1,
        // Overkill (`specialty_twoprimaries`, 166) in perk 2 takes a
        // primary there too (0x4fc3d0).
        3 if overkill => 0x3,
        3 => 0x2,
        5 => 0x4,
        6 => 0x8,
        7 => 0x10,
        8 => 0x20,
        _ => return None,
    })
}

/// The stat holding the profile's XP (`rankxp`).
const XP: i32 = 2301;

/// A cell of `t`, "" past it.
fn cell(t: &Table, row: i32, column: i32) -> &str {
    let (Ok(r), Ok(c)) = (usize::try_from(row), usize::try_from(column)) else {
        return "";
    };
    if c >= t.columns {
        return "";
    }
    t.cells.get(r * t.columns + c).map_or("", String::as_str)
}

impl ScriptMenus {
    /// Custom class `n`'s name (`customclass<n>`, a vcod cvar).
    pub fn set_class_name(&mut self, n: u32, name: &str) {
        self.set_dvar(&format!("customclass{n}"), name);
    }

    /// The `statset <index> <value>` command (`iw3mp.exe` 0x579d50): the
    /// client's own stats, the create-a-class slots 200..249 (past
    /// [`Self::cac_refusal`]) and 256..279, into the profile's block. Any
    /// other index takes only 0; a non-zero value there, or 250, is
    /// retail's `Com_Error` ("Trying to set a server-only stat", 0x6e1004;
    /// "trying to set invalid create-a-class stat", 0x6e0fc0).
    pub(super) fn stat_set(&mut self, index: i32, value: i32) {
        let client = (200..250).contains(&index) || (256..280).contains(&index);
        if index == 250 {
            log::error!("trying to set invalid create-a-class stat.  You can't set stat {index}.");
            return;
        }
        if !client && value != 0 {
            log::error!("Trying to set a server-only stat {index}.");
            return;
        }
        if (200..250).contains(&index)
            && let Some(e) = self.cac_refusal(index, value)
        {
            log::error!("{e}");
            return;
        }
        self.store_stat(index, value);
    }

    /// `statSetUsingTable` (0x54c940): `index` takes `table`'s column `ret`
    /// of the row whose column `column` reads `value` (0 with no such row
    /// or table). Only the create-a-class slots 200..249, past
    /// [`Self::cac_refusal`], and the menus' scratch stats 2400..2402.
    pub(super) fn stat_set_using_table(
        &mut self,
        index: &str,
        table: &str,
        column: &str,
        value: &str,
        ret: &str,
    ) {
        let index = atoi(index);
        let found = match self.tables.get(&table.to_ascii_lowercase()) {
            Some(t) => t.lookup(atoi(column), value, atoi(ret)).to_string(),
            None => {
                log::warn!("Unable to find the lookup table in the fastfile, aborting lookup");
                String::new()
            }
        };
        let v = atoi(&found);
        if !(2400..=2402).contains(&index) {
            if !(200..250).contains(&index) {
                log::error!(
                    "statsetusingtable should only be used for create-a-class.  You can't set stat {index}"
                );
                return;
            }
            if let Some(e) = self.cac_refusal(index, v) {
                log::error!("{e}");
                return;
            }
        }
        self.store_stat(index, v);
    }

    /// `statClearBitMask` (0x54d060): the stat whose index the dvar
    /// `index` holds loses the bits the dvar `mask` holds (each 0 when
    /// unset). Any index.
    pub(super) fn stat_clear_bit_mask(&mut self, index: &str, mask: &str) {
        let read = |d: &str| self.dvar(d).map_or(0, |v| atoi(&v));
        let (index, mask) = (read(index), read(mask));
        let now = self.stat(index);
        self.store_stat(index, now & !mask);
    }

    /// `statClearPerkNew` (0x54cee0): the perk's stat (`mp/statstable.csv`,
    /// the row whose column 4 names it, column 1) takes 1 (unlocked) and
    /// loses 2 (new). The stat must be a perk's, 150..298, holding 1 or 2.
    pub(super) fn stat_clear_perk_new(&mut self, perk: &str) {
        let index = match self.tables.get("mp/statstable.csv") {
            Some(t) if t.columns > 1 => atoi(t.lookup(4, perk, 1)),
            Some(_) => 0,
            None => {
                log::warn!("Unable to find the lookup table in the fastfile, aborting lookup");
                0
            }
        };
        if !(150..=298).contains(&index) {
            log::error!("statClearPerkNew: Invalid perk index {index} for {perk}");
            return;
        }
        let now = self.stat(index);
        if now & 3 == 0 {
            log::error!("statClearPerkNew: perk {perk}[{index}] isn't unlocked.");
            return;
        }
        self.store_stat(index, (now & !2) | 1);
    }

    /// 0x4fc440's check of a create-a-class stat's value: an item of
    /// [`ITEMS`], unlocked at the profile's rank, that may fill the slot.
    /// The `Com_Error` message when it fails.
    fn cac_refusal(&self, index: i32, value: i32) -> Option<String> {
        let digit = index % 10;
        let overkill = self.stat(index / 10 * 10 + 6) == 166;
        let bits = slot_bits(digit, overkill)?;
        let Some(&(_, rank, slots)) = ITEMS.iter().find(|r| r.0 == value) else {
            return Some(format!("No create-a-class item found at index {value}"));
        };
        let have = self.rank();
        if have < rank {
            return Some(format!(
                "Create-a-class item {value} is too high level {} < {}.",
                have + 1,
                rank + 1
            ));
        }
        (bits & slots == 0).then(|| {
            format!(
                "Create-a-class item {value} is not the expected type for this slot (0x{bits:x} & 0x{slots:x}) == 0."
            )
        })
    }

    /// The profile's rank (0x4749d0): `mp/rankTable.csv`'s column 0 of the
    /// row whose XP range (columns 2 and 7) holds the profile's XP, found
    /// by retail's binary search.
    fn rank(&self) -> i32 {
        let Some(t) = self.tables.get("mp/ranktable.csv") else {
            return 0;
        };
        let xp = self.stat(XP);
        let rows = t.cells.len().checked_div(t.columns).unwrap_or(0) as i32;
        let (mut lo, mut hi) = (0, rows - 1);
        let mut mid = hi / 2;
        while hi > lo {
            if xp < atoi(cell(t, mid, 2)) {
                hi = mid - 1;
            } else if xp >= atoi(cell(t, mid, 7)) {
                lo = mid + 1;
            } else {
                break;
            }
            mid = lo + (hi - lo) / 2;
        }
        atoi(cell(t, mid, 0))
    }

    /// `LiveStorage_SetStat` (0x579c70): a byte stat under 2000 (0..255,
    /// else retail's `Com_Error`, 0x6f5cf0), a word to 3497; nothing past.
    fn store_stat(&mut self, index: i32, value: i32) {
        if (0..2000).contains(&index) && !(0..256).contains(&value) {
            log::error!(
                "Trying to set index {index} (which is a byte value) to invalid value {value}"
            );
            return;
        }
        let Some(stats) = self.stats.as_mut() else {
            log::info!("Tried to set stat index {index} before we have obtained player stats");
            return;
        };
        if let Ok(i) = u16::try_from(index) {
            stats.set(i, value as u32);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use vcod_cod4::net::parse::Gamestate;
    use vcod_cod4::scriptmenu::Cmd;

    use super::*;

    /// `mp/statstable.csv`'s columns 0, 1 and 4 for a few rows, and a
    /// rank table whose rank `r` runs from `100 r` to `100 (r + 1)` XP.
    fn menus() -> ScriptMenus {
        let stats = [
            ("0", "3000", "beretta"),
            ("2", "3002", "usp"),
            ("25", "3025", "m16"),
            ("26", "3026", "m4"),
            ("160", "160", "specialty_bulletdamage"),
            ("151", "151", "specialty_bulletpenetration"),
            ("102", "102", "flash_grenade"),
        ];
        let mut cells = Vec::new();
        for (n, stat, name) in stats {
            cells.extend([n, stat, "", "", name].map(str::to_string));
        }
        let statstable = Table { columns: 5, cells };
        let mut cells = Vec::new();
        for r in 0..55 {
            let (min, max) = ((100 * r).to_string(), (100 * (r + 1)).to_string());
            cells.extend([r.to_string(), String::new(), min, String::new()]);
            cells.extend([String::new(), String::new(), String::new(), max]);
        }
        let ranktable = Table { columns: 8, cells };
        let tables = HashMap::from([
            ("mp/statstable.csv".to_string(), statstable),
            ("mp/ranktable.csv".to_string(), ranktable),
        ]);
        let mut m = ScriptMenus::front_end();
        m.set_defs(HashMap::new(), tables);
        m
    }

    fn run(m: &mut ScriptMenus, script: &str) {
        let mut out = Vec::new();
        m.run(
            "test",
            None,
            &Cmd::parse(script),
            &Gamestate::default(),
            &mut out,
            0,
        );
    }

    /// `statsetusingtable` writes a class slot off the table and the
    /// menus' scratch 2400..2402; any other stat, an item the slot does
    /// not take or one past the profile's rank is refused.
    #[test]
    fn cod4_statsetusingtable_writes_a_class_slot() {
        let mut m = menus();
        let set = |stat: &str, name: &str, ret: u8| {
            format!(
                "\"statsetusingtable\" ( \"{stat}\" , \"tablelookup\" ( \"mp/statstable.csv\" , 4 , \"{name}\" , {ret} ) ) ; "
            )
        };
        run(&mut m, &set("211", "m16", 0));
        run(&mut m, &set("2400", "m16", 1));
        assert_eq!((m.stat(211), m.stat(2400)), (25, 3025));
        // Not a class stat; a pistol as the primary; the M4 at rank 0
        // (it unlocks at 9).
        run(&mut m, &set("260", "m16", 0));
        run(&mut m, &set("201", "beretta", 0));
        run(&mut m, &set("211", "m4", 0));
        assert_eq!((m.stat(260), m.stat(201), m.stat(211)), (0, 25, 25));
        m.stats.as_mut().unwrap().set(2301, 950);
        run(&mut m, &set("211", "m4", 0));
        assert_eq!(m.stat(211), 26);
        // A pistol in the secondary, a perk in perk 2, and Overkill's
        // secondary takes a primary.
        run(&mut m, &set("203", "usp", 0));
        run(&mut m, &set("206", "specialty_bulletdamage", 1));
        assert_eq!((m.stat(203), m.stat(206)), (2, 160));
        run(&mut m, &set("203", "m16", 0));
        assert_eq!(m.stat(203), 2);
        // Overkill unlocks at 37.
        m.exec("statset 206 166");
        assert_eq!(m.stat(206), 160);
        m.stats.as_mut().unwrap().set(2301, 3800);
        m.exec("statset 206 166");
        run(&mut m, &set("203", "m16", 0));
        assert_eq!(m.stat(203), 25);
    }

    /// `statClearBitMask` clears the dvars' bits off the stat the first
    /// names; `statClearPerkNew` keeps an earned perk and drops its new
    /// bit, and refuses a locked one.
    #[test]
    fn cod4_stat_bit_commands_clear_the_new_marks() {
        let mut m = menus();
        m.stats.as_mut().unwrap().set(3025, 0x10001);
        m.exec("set ui_primary_weapon 3025; set ui_tempMask 65536");
        run(
            &mut m,
            "\"statClearBitMask\" \"ui_primary_weapon\" \"ui_tempMask\" ; ;",
        );
        assert_eq!(m.stat(3025), 1);
        m.stats.as_mut().unwrap().set(160, 2);
        run(
            &mut m,
            "\"statClearPerkNew\" ( \"specialty_bulletdamage\" ) ;",
        );
        assert_eq!(m.stat(160), 1);
        run(
            &mut m,
            "\"statClearPerkNew\" ( \"specialty_bulletpenetration\" ) ;",
        );
        run(&mut m, "\"statClearPerkNew\" ( \"m16\" ) ;");
        assert_eq!((m.stat(151), m.stat(3025)), (0, 1));
    }

    /// The template's `setFromLocString` names the class from its key
    /// when the zones have no string for it; `uploadstats` asks for the
    /// file.
    #[test]
    fn cod4_template_names_the_class_and_uploads() {
        let mut m = menus();
        m.exec("setFromLocString customclass1 @CLASS_CUSTOM1; uploadstats");
        assert_eq!(m.dvar("customclass1").as_deref(), Some("@CLASS_CUSTOM1"));
        assert!(m.take_upload_stats());
        assert!(!m.take_upload_stats());
    }
}
