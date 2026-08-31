//! The server's cvar table. `GameHost` owns it while the script runs and
//! `Server` reads it back every frame, the same shape as the configstring
//! table: one owner at a time, no half-synced copy.
//!
//! Configstrings 140..203 hold the names of the mirrored cvars and
//! 204..267 their values, paired by offset 64, and the client runs
//! `Cvar_Set(cs[140 + i], cs[204 + i])` down the block until a value is
//! empty. The block is sorted by name, not written in call order: the
//! evidence is in docs/design/2026-08-30-gsc-stage3-design.md.

use std::collections::BTreeMap;
use vcod_gsc::ErrorKind;

pub const MIRROR_NAMES: std::ops::RangeInclusive<usize> = 140..=203;
pub const MIRROR_VALUES: std::ops::RangeInclusive<usize> = 204..=267;

/// The 21 cvars the game module registers into the mirror at init, before
/// any script runs: its cvar table's 0x800-flagged rows. Values are the
/// retail capture's, recorded in docs/research/cod11-server-handshake.md.
const ENGINE_MIRRORED: &[(&str, &str)] = &[
    ("bg_duck2prone_time", "400"),
    ("bg_foliagesnd_fastinterval", "500"),
    ("bg_foliagesnd_maxspeed", "180"),
    ("bg_foliagesnd_minspeed", "40"),
    ("bg_foliagesnd_resetinterval", "500"),
    ("bg_foliagesnd_slowinterval", "1500"),
    ("bg_ladder_yawcap", "100"),
    ("bg_prone2duck_time", "400"),
    ("bg_prone_softyawedge", "1"),
    ("bg_prone_yawcap", "85"),
    ("bg_viewheight_crouched", "40"),
    ("bg_viewheight_prone", "11"),
    ("bg_viewheight_standing", "60"),
    ("g_ScoresBanner_Allies", "gfx/hud/hud@mpflag_american.tga"),
    ("g_ScoresBanner_Axis", "gfx/hud/hud@mpflag_german.tga"),
    ("g_ScoresBanner_None", "gfx/hud/hud@mpflag_none.tga"),
    (
        "g_ScoresBanner_Spectators",
        "gfx/hud/hud@mpflag_spectator.tga",
    ),
    ("g_TeamColor_Allies", "0.5 0.5 1"),
    ("g_TeamColor_Axis", "1 0.5 0.5"),
    ("g_TeamName_Allies", "GAME_ALLIES"),
    ("g_TeamName_Axis", "GAME_AXIS"),
];

/// Rows of the same table that are not mirrored but that a stock script's
/// outcome depends on. One so far: `character\_utility::useOptionalModels`
/// gates every character script's gear model precaches on `g_useGear`, so
/// without it the model configstring block loses every gear model
/// (docs/research/cod11-gsc-object-model.md section 18).
///
/// The other 70 rows are deliberately not transcribed, and a script that
/// reads one of them gets `""` with no warning. `tools/re/dump_cvars.py`
/// prints the table, so adding a row is a lookup: pass the cvar's name and
/// take its default.
const ENGINE_DEFAULTS: &[(&str, &str)] = &[("g_useGear", "1")];

struct Cvar {
    /// The registration spelling. Lookup folds, the mirror does not: the
    /// capture holds `g_TeamName_Allies`, not `g_teamname_allies`.
    name: String,
    value: String,
    /// In the 140/204 mirror. Set by `makeCvarServerInfo`, which despite
    /// its name does not put the cvar in configstring 0: retail's cs 0
    /// holds only the `sv_*` and `g_gametype` set.
    mirrored: bool,
}

pub struct Cvars {
    /// Keyed by the folded name, so lookup is case-insensitive the way
    /// `Cvar_FindVar` is, while the mirror keeps the registration
    /// spelling.
    vars: BTreeMap<String, Cvar>,
}

impl Default for Cvars {
    fn default() -> Self {
        Cvars::new()
    }
}

impl Cvars {
    pub fn new() -> Cvars {
        let mut cv = Cvars {
            vars: BTreeMap::new(),
        };
        for &(name, value) in ENGINE_MIRRORED {
            cv.make_server_info(name, value);
        }
        for &(name, value) in ENGINE_DEFAULTS {
            cv.register(name, value);
        }
        cv
    }

    pub fn get(&self, name: &str) -> &str {
        self.vars
            .get(&name.to_ascii_lowercase())
            .map_or("", |c| c.value.as_str())
    }

    /// `Cvar_Set`: registers the cvar if it does not exist, and never
    /// changes an existing one's flags.
    pub fn set(&mut self, name: &str, value: &str) {
        let entry = self
            .vars
            .entry(name.to_ascii_lowercase())
            .or_insert_with(|| Cvar {
                name: name.to_string(),
                value: String::new(),
                mirrored: false,
            });
        entry.value = value.to_string();
    }

    /// `Cvar_Get(name, default)`: takes the default only when the cvar does
    /// not exist, so a cvar already set on the command line keeps its
    /// value. This is how a cvar table's row is seeded, and the opposite of
    /// `set` above -- seeding with `set` would silently overwrite whatever
    /// the operator passed.
    pub fn register(&mut self, name: &str, default: &str) {
        self.vars
            .entry(name.to_ascii_lowercase())
            .or_insert_with(|| Cvar {
                name: name.to_string(),
                value: default.to_string(),
                mirrored: false,
            });
    }

    /// Applies a config file's `set` lines, as `Cvar_Set` each, and returns
    /// how many it applied. `bind`, `unbindall` and everything else in the
    /// file is dropped: nothing else in `default_mp.cfg` reaches a
    /// dedicated server's cvar table.
    ///
    /// `set` and not `register` is the right primitive here. Nothing the
    /// stock file names is in the game module's own cvar table, so there is
    /// no registration to lose, and `set` never flags a cvar into the
    /// 140/204 mirror -- a `scr_*` value from the file reaches the mirror
    /// only if a script later calls `makeCvarServerInfo` on that name,
    /// which keeps the existing value.
    pub fn exec_cfg(&mut self, text: &str) -> usize {
        let mut applied = 0;
        for line in text.lines() {
            let tokens = cfg_tokens(line);
            if tokens.len() >= 2 && tokens[0].eq_ignore_ascii_case("set") {
                self.set(&tokens[1], tokens.get(2).map_or("", |s| s.as_str()));
                applied += 1;
            }
        }
        applied
    }

    /// `makeCvarServerInfo(name, default)`: `register` plus the mirror
    /// flag, which it sets whether or not the cvar already existed.
    pub fn make_server_info(&mut self, name: &str, default: &str) {
        self.register(name, default);
        if let Some(c) = self.vars.get_mut(&name.to_ascii_lowercase()) {
            c.mirrored = true;
        }
    }

    /// Rebuilds the 140/204 block from the flagged set. Sorted by
    /// registration name in byte order; the retail capture cannot tell byte
    /// order from case-insensitive order, so this is the choice, not a
    /// measurement.
    pub fn write_mirror(&self, cs: &mut [String]) -> Result<(), ErrorKind> {
        let mut flagged: Vec<&Cvar> = self.vars.values().filter(|c| c.mirrored).collect();
        flagged.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        if flagged.len() > MIRROR_NAMES.count() {
            // Checked before the clear loop below: `Server::tick` passes its
            // live configstring table in, and an overflow here must leave
            // the previous frame's mirror stale rather than wipe it.
            return Err(ErrorKind::BadType("cvar mirror range exhausted"));
        }
        for i in MIRROR_NAMES.chain(MIRROR_VALUES) {
            cs[i] = String::new();
        }
        for (n, c) in flagged.iter().enumerate() {
            cs[MIRROR_NAMES.start() + n] = c.name.clone();
            cs[MIRROR_VALUES.start() + n] = c.value.clone();
        }
        Ok(())
    }
}

/// `Cmd_TokenizeString` cut down to what a cfg line needs: `//` starts a
/// comment, a double-quoted run is one token, everything else splits on
/// whitespace.
fn cfg_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                tokens.push(std::mem::take(&mut cur));
                quoted = false;
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '/' if chars.peek() == Some(&'/') => break,
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `g_useGear` is in the game module's own cvar table with default
    /// `"1"` and flags 0x21, not the 0x800 the mirrored set carries, so it
    /// never reaches the 140/204 block -- but every character script gates
    /// its gear model precaches on it through
    /// `character\_utility::useOptionalModels`, so a missing default costs
    /// the model configstring block every gear model on the map.
    #[test]
    fn g_use_gear_defaults_to_one_and_stays_out_of_the_mirror() {
        let cv = Cvars::new();
        assert_eq!(cv.get("g_useGear"), "1");
        let mut cs = vec![String::new(); 2048];
        cv.write_mirror(&mut cs).unwrap();
        assert!(!cs[MIRROR_NAMES].iter().any(|n| n == "g_useGear"));
    }

    /// Seeding a cvar table row keeps a value that is already there, so an
    /// operator's command-line `+set g_useGear 0` survives the seed. Both
    /// `ENGINE_MIRRORED` and `ENGINE_DEFAULTS` go through this primitive;
    /// `set` is the other direction and would overwrite.
    #[test]
    fn registering_a_default_never_overwrites_an_existing_value() {
        let mut cv = Cvars::new();
        cv.set("g_useGear", "0");
        cv.register("g_useGear", "1");
        assert_eq!(cv.get("g_useGear"), "0");
    }

    /// The 20 stock script cvars beyond the engine's 21: the 18
    /// `scr_allow_*` names and `scr_motd` that `_teams::initGlobalCvars()`
    /// passes to `makeCvarServerInfo`, plus `scr_layoutimage`, flagged
    /// earlier in `Callback_StartGameType`. Names and values are the
    /// retail capture's 161..180 / 225..244 block
    /// (crates/server/tests/fixtures/configstrings/mp_pavlov-dm.txt).
    ///
    /// `scr_allow_fg42` is `"0"` here (slot 164/228 in that capture) while
    /// every other `scr_allow_*` is `"1"`. Activision's own
    /// `_teams::initGlobalCvars()` passes `"1"` as fg42's default too, and
    /// `makeCvarServerInfo` never overwrites a cvar that already has a
    /// value, so that call is not what produced the `"0"`. What did is
    /// `default_mp.cfg`, which the engine execs at startup and whose line
    /// 95 is `set scr_allow_fg42 0` -- the file's only `scr_allow_*` row
    /// set to `0` (docs/research/cod11-gsc-object-model.md section 18).
    /// `Server::cvars` runs the file, so this table is a transcription of
    /// the capture, not a source of values.
    const STOCK_SCRIPT_CVARS: &[(&str, &str)] = &[
        ("scr_allow_bar", "1"),
        ("scr_allow_bren", "1"),
        ("scr_allow_enfield", "1"),
        ("scr_allow_fg42", "0"),
        ("scr_allow_kar98k", "1"),
        ("scr_allow_kar98ksniper", "1"),
        ("scr_allow_m1carbine", "1"),
        ("scr_allow_m1garand", "1"),
        ("scr_allow_mp40", "1"),
        ("scr_allow_mp44", "1"),
        ("scr_allow_nagant", "1"),
        ("scr_allow_nagantsniper", "1"),
        ("scr_allow_panzerfaust", "1"),
        ("scr_allow_ppsh", "1"),
        ("scr_allow_springfield", "1"),
        ("scr_allow_sten", "1"),
        ("scr_allow_thompson", "1"),
        ("scr_allow_vote", "1"),
        ("scr_layoutimage", "levelshots/layouts/hud@layout_mp_pavlov"),
        ("scr_motd", ""),
    ];

    /// The mirror is a sorted rebuild of the flagged set, not a call-order
    /// log. `scr_layoutimage` is flagged in `Callback_StartGameType`,
    /// before `_teams::initGlobalCvars()` runs its 18 calls, and still
    /// lands after every `scr_allow_*`; `scr_motd`, one of those 18, lands
    /// last. Registering the full stock flagged set (the 21 engine cvars
    /// plus these 20) is what makes the absolute slots match the retail
    /// capture; a handful of cvars sorts into different slots than the
    /// stock set does.
    #[test]
    fn the_mirror_is_sorted_not_call_ordered() {
        let mut cv = Cvars::new();
        let mut cs = vec![String::new(); 2048];
        for &(name, value) in STOCK_SCRIPT_CVARS {
            cv.make_server_info(name, value);
        }
        cv.write_mirror(&mut cs).unwrap();
        assert_eq!(cs[178], "scr_allow_vote");
        assert_eq!(cs[179], "scr_layoutimage");
        assert_eq!(cs[180], "scr_motd");
        assert_eq!(cs[243], "levelshots/layouts/hud@layout_mp_pavlov");
        // scr_motd's value is empty, which is also the client's loop stop
        // condition, so 244 stays unset.
        assert_eq!(cs[244], "");
    }

    /// The 21 cvars the engine registers into the mirror before any script
    /// runs, from the retail capture's 140..160 block.
    #[test]
    fn the_engine_defaults_fill_140_to_160() {
        let cv = Cvars::new();
        let mut cs = vec![String::new(); 2048];
        cv.write_mirror(&mut cs).unwrap();
        assert_eq!(cs[140], "bg_duck2prone_time");
        assert_eq!(cs[204], "400");
        assert_eq!(cs[152], "bg_viewheight_standing");
        assert_eq!(cs[216], "60");
        assert_eq!(cs[159], "g_TeamName_Allies");
        assert_eq!(cs[223], "GAME_ALLIES");
        assert_eq!(cs[160], "g_TeamName_Axis");
        assert_eq!(cs[224], "GAME_AXIS");
    }

    /// `exec_cfg` takes the `set` lines and nothing else, dequotes a
    /// quoted value, stops at a `//` comment, and overwrites a value that
    /// is already there the way `Cvar_Set` does. The shapes here are all
    /// from `default_mp.cfg` itself: tab-separated binds, a `set` whose
    /// value is quoted, and a commented-out line.
    #[test]
    fn exec_cfg_applies_set_lines_only() {
        let mut cv = Cvars::new();
        cv.set("scr_allow_fg42", "1");
        let applied = cv.exec_cfg(concat!(
            "// Hello Dave, Would you like to play a game?\r\n",
            "unbindall\r\n",
            "bind\tPAUSE\t\t\"toggle cl_paused\"\r\n",
            "//set scr_allow_mp40 0\r\n",
            "set scr_allow_fg42 0\r\n",
            "set m_pitch \"0.022\"\r\n",
        ));
        assert_eq!(applied, 2);
        assert_eq!(cv.get("scr_allow_fg42"), "0");
        assert_eq!(cv.get("m_pitch"), "0.022");
        assert_eq!(cv.get("scr_allow_mp40"), "");
        assert_eq!(cv.get("toggle cl_paused"), "");
    }

    /// A cfg value never reaches the 140/204 mirror by itself: `set` does
    /// not flag, so a `scr_*` row only appears there once a script's
    /// `makeCvarServerInfo` flags that name, and then with the cfg's value
    /// rather than the script's default. That pair is exactly how retail's
    /// slot 228 reads `0` while `_teams::initGlobalCvars()` passes `"1"`.
    #[test]
    fn a_cfg_value_enters_the_mirror_only_when_a_script_flags_it() {
        let mut cv = Cvars::new();
        cv.exec_cfg("set scr_allow_fg42 0\nset scr_dm_scorelimit 50\n");
        let mut cs = vec![String::new(); 2048];
        cv.write_mirror(&mut cs).unwrap();
        assert!(!cs[MIRROR_NAMES].iter().any(|n| n == "scr_allow_fg42"));
        cv.make_server_info("scr_allow_fg42", "1");
        cv.write_mirror(&mut cs).unwrap();
        let at = cs[MIRROR_NAMES]
            .iter()
            .position(|n| n == "scr_allow_fg42")
            .expect("flagged now");
        assert_eq!(cs[MIRROR_VALUES.start() + at], "0");
        assert!(!cs[MIRROR_NAMES].iter().any(|n| n == "scr_dm_scorelimit"));
    }

    /// An unset cvar reads empty, which is what `Cvar_VariableString`
    /// returns for an unregistered name.
    #[test]
    fn an_unset_cvar_reads_empty() {
        let cv = Cvars::new();
        assert_eq!(cv.get("no_such_cvar"), "");
    }

    /// `setCvar` on a mirrored cvar updates the mirrored value; on an
    /// unflagged one it does not enter the mirror at all.
    #[test]
    fn set_updates_a_mirrored_value_and_does_not_flag_a_new_one() {
        let mut cv = Cvars::new();
        let mut cs = vec![String::new(); 2048];
        cv.set("g_TeamName_Allies", "MPSCRIPT_RUSSIAN");
        cv.set("scr_dm_scorelimit", "50");
        cv.write_mirror(&mut cs).unwrap();
        assert_eq!(cs[223], "MPSCRIPT_RUSSIAN");
        assert!(!cs[140..=203].contains(&"scr_dm_scorelimit".to_string()));
        assert_eq!(cv.get("scr_dm_scorelimit"), "50");
    }

    /// The mirror never reaches a slot the static table owns; 140..267 is
    /// clear of all six.
    #[test]
    fn the_mirror_is_clear_of_the_static_table() {
        for &(i, _) in crate::configstrings::STATIC {
            assert!(
                !MIRROR_NAMES.contains(&i) && !MIRROR_VALUES.contains(&i),
                "the static table sets {i}, which the mirror writes"
            );
        }
    }
}
