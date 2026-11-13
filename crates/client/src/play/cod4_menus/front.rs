//! The front end's half of the menus: the `uiScript`s the main menu's
//! screens run with no game up (Start New Server, the Mods list, the
//! player profiles, Quit) and the feeders their list boxes read
//! (cod4-front-end.md 3, 5 and 6; what is not labelled VERIFIED there is
//! INFERRED, not yet confirmed on 1.7). What the client has to carry out
//! goes into [`Front::requests`]; the menu sounds into [`Front::sounds`].

use std::path::{Path, PathBuf};

use vcod_cod4::menuexp::atoi;
use vcod_cod4::net::parse::Gamestate;
use vcod_common::net::cdkey::{self, KeyGame};

use super::ScriptMenus;

/// `FEEDER_MODS` and `FEEDER_PLAYER_PROFILES`, the feeders `mods_menu`'s
/// and `player_profile`'s list boxes name (the `ui_mp` menus' `feeder`).
pub const FEEDER_MODS: f32 = 9.0;
pub const FEEDER_PROFILES: f32 = 24.0;
/// The bytes of a mod's `description.txt` the list shows (`FS_GetModList`
/// reads 0x30).
const DESCRIPTION_BYTES: usize = 0x30;
/// `UI_CreatePlayerProfile` refuses past 64 profiles, and keeps a new
/// name's first 31 bytes (cod4-front-end.md 3).
const MAX_PROFILES: usize = 64;
const PROFILE_NAME_BYTES: usize = 31;

/// The settings `createserver` and its game type pages set, which a
/// server started from the menu takes when they have a value.
pub const SERVER_DVARS: [&str; 9] = [
    "sv_hostname",
    "sv_maxclients",
    "sv_minping",
    "sv_maxping",
    "g_password",
    "sv_voice",
    "scr_teambalance",
    "g_allowvote",
    "sv_punkbuster",
];

/// A dvar the front end keeps in the UI's table, vcod having no cvar for
/// it: its own `ui_*` ones, the server settings Start New Server sets and
/// the CD key menu's five fields.
pub fn ui_table_dvar(name: &str) -> bool {
    name.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("ui_"))
        || SERVER_DVARS.iter().any(|d| d.eq_ignore_ascii_case(name))
        || CDKEY_DVARS.iter().any(|d| d.eq_ignore_ascii_case(name))
}

/// `cdkey_menu`'s fields: the key's four quarters and its check.
const CDKEY_DVARS: [&str; 5] = ["cdkey1", "cdkey2", "cdkey3", "cdkey4", "cdkey5"];

/// What a front-end script asks the client to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// `uiScript StartServer`: `dedicated` set to `ui_dedicated`,
    /// `g_gametype` to the picked game type, then `map <map>`; vcod
    /// starts `vcod-server --game cod4` with them, and the settings
    /// `cvars` names (`(name, value)`).
    StartServer {
        map: String,
        gametype: String,
        dedicated: i32,
        cvars: Vec<(String, String)>,
    },
    /// `uiScript RunMod` (`fs_game` `mods/<dir>` and a `vid_restart`), or
    /// `ClearMods` (`None`).
    RunMod(Option<String>),
    /// `uiScript loadPlayerProfile` (`Com_ChangePlayerProfile`): the
    /// profile becomes the active one, `active.txt` names it and its
    /// config takes over.
    LoadProfile(String),
    /// `uiScript deletePlayerProfile` removed this profile's directory.
    ProfileDeleted(String),
}

/// The front end's state past the menus.
#[derive(Clone, Debug, Default)]
pub struct Front {
    pub requests: Vec<Request>,
    /// The aliases the menus' `play` played.
    pub sounds: Vec<String>,
    /// The install root, whose `mods/` the list reads.
    pub root: Option<PathBuf>,
    /// The directory holding `players/profiles/` (the install root; a
    /// test's own directory).
    pub players_root: Option<PathBuf>,
    /// `mods/`' directories and what each lists as (its
    /// `description.txt`, else its name), and the pick.
    pub mods: Vec<(String, String)>,
    mod_pick: usize,
    /// `players/profiles/`' directories, by name; the pick is
    /// `ui_playerProfileSelected`.
    pub profiles: Vec<String>,
}

/// `FS_GetModList`: each directory under `<root>/mods` with its
/// description, by name.
pub fn mod_list(root: &Path) -> Vec<(String, String)> {
    let Some(dir) = find_dir(root, "mods") else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let desc = std::fs::read(e.path().join("description.txt"))
                .ok()
                .map(|b| {
                    let b = &b[..b.len().min(DESCRIPTION_BYTES)];
                    String::from_utf8_lossy(b).trim().to_string()
                })
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| name.clone());
            (name, desc)
        })
        .collect();
    out.sort_by_key(|(n, _)| n.to_ascii_lowercase());
    out
}

/// The profiles under `<root>/players/profiles`, by name.
pub fn profile_list(root: &Path) -> Vec<String> {
    let Some(dir) = find_dir(root, "players").and_then(|p| find_dir(&p, "profiles")) else {
        return Vec::new();
    };
    let mut out: Vec<String> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort_by_key(|n| n.to_ascii_lowercase());
    out
}

/// `Com_SetInitialPlayerProfile`: `players/profiles/active.txt`'s first
/// word, when that profile's directory exists.
pub fn active_profile(root: &Path) -> Option<String> {
    let dir = find_dir(root, "players").and_then(|p| find_dir(&p, "profiles"))?;
    let text = std::fs::read_to_string(dir.join("active.txt")).ok()?;
    let name = text.split_whitespace().next()?;
    profile_list(root)
        .into_iter()
        .find(|p| p.eq_ignore_ascii_case(name))
}

/// Profile `name`'s directory under `<root>/players/profiles`, if it
/// exists.
pub fn profile_dir(root: &Path, name: &str) -> Option<PathBuf> {
    let dir = find_dir(root, "players").and_then(|p| find_dir(&p, "profiles"))?;
    find_dir(&dir, name)
}

/// `players/profiles` under `root`, as it is on disk, else where 1.7
/// creates it.
pub fn profiles_root(root: &Path) -> PathBuf {
    let players = find_dir(root, "players").unwrap_or_else(|| root.join("players"));
    find_dir(&players, "profiles").unwrap_or_else(|| players.join("profiles"))
}

/// `Com_NewPlayerProfile`: the directory `players/profiles/<name>/`; false
/// when it cannot be made. A name that would leave the directory (a
/// separator, `.` or `..`) is refused, where retail's file field never
/// lets one be typed.
pub fn create_profile(root: &Path, name: &str) -> bool {
    let unsafe_name = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', ':'])
        || name.chars().any(char::is_control);
    if unsafe_name {
        log::warn!("cod4 front end: profile name {name:?} refused");
        return false;
    }
    let dir = profiles_root(root).join(name);
    match std::fs::create_dir_all(&dir) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("Unable to create new profile path: {}: {e}", dir.display());
            false
        }
    }
}

/// `Com_DeletePlayerProfile`: the profile's directory and what it holds;
/// false when there is none or it cannot be removed.
pub fn delete_profile(root: &Path, name: &str) -> bool {
    let Some(dir) = profile_dir(root, name).filter(|_| !name.is_empty()) else {
        return false;
    };
    std::fs::remove_dir_all(&dir)
        .inspect_err(|e| log::warn!("cannot delete {}: {e}", dir.display()))
        .is_ok()
}

/// `dir/name` as it is on disk, matched case-insensitively.
fn find_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = dir.join(name);
    if exact.is_dir() {
        return Some(exact);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
        .filter(|p| p.is_dir())
}

impl ScriptMenus {
    /// A front-end `uiScript`; false when it is not one.
    pub(super) fn front_script(
        &mut self,
        name: &str,
        args: &[String],
        gs: &Gamestate,
        out: &mut Vec<String>,
        depth: u32,
    ) -> bool {
        let root = self.front.root.clone();
        let players = self.front.players_root.clone();
        match name.to_ascii_lowercase().as_str() {
            // `Cmd_ExecuteSingleCommand(.., "quit")`.
            "quit" => self.console.push("quit".into()),
            "startserver" => self.start_server(),
            "loadmods" => {
                self.front.mods = root.as_deref().map(mod_list).unwrap_or_default();
                self.front.mod_pick = 0;
            }
            "runmod" => {
                if let Some((dir, _)) = self.front.mods.get(self.front.mod_pick) {
                    let game = format!("mods/{dir}");
                    self.front.requests.push(Request::RunMod(Some(game)));
                }
            }
            "clearmods" => self.front.requests.push(Request::RunMod(None)),
            "addplayerprofiles" => {
                self.front.profiles = players.as_deref().map(profile_list).unwrap_or_default();
                self.profile_count();
            }
            // `UI_SelectActivePlayerProfile`: the list's pick follows
            // `com_playerProfile`, when it is listed.
            "selectactiveplayerprofile" => {
                let active = self.dvar("com_playerProfile").unwrap_or_default();
                if let Some(row) = self.profile_row(&active) {
                    self.select_profile(row);
                }
            }
            "createplayerprofile" => self.create_profile(gs, out, depth),
            "deleteplayerprofile" => self.delete_profile(gs, out, depth),
            // `UI_LoadPlayerProfile` / `Com_ChangePlayerProfile`: the picked
            // profile, when it is not the active one and its directory is
            // there, becomes `com_playerProfile`; the client writes
            // `active.txt` and switches to its config.
            "loadplayerprofile" => {
                let picked = self.dvar("ui_playerProfileSelected").unwrap_or_default();
                let active = self.dvar("com_playerProfile").unwrap_or_default();
                let exists = players
                    .as_deref()
                    .is_some_and(|r| profile_dir(r, &picked).is_some());
                if !picked.is_empty() && !picked.eq_ignore_ascii_case(&active) && exists {
                    self.ui
                        .dvars
                        .insert("com_playerprofile".into(), picked.clone());
                    self.front.requests.push(Request::LoadProfile(picked));
                }
            }
            // `error_popmenu`'s dismissal.
            "clearerror" => {
                self.ui
                    .dvars
                    .insert("com_errormessage".into(), String::new());
            }
            "closemenuondvar" | "closemenuondvarnot" => {
                let [dvar, value, menu] =
                    [0, 1, 2].map(|i| args.get(i).cloned().unwrap_or_default());
                let is = self.dvar(&dvar).unwrap_or_default() == value;
                if is == name.eq_ignore_ascii_case("closeMenuOnDvar") {
                    self.close_named(&menu, gs, out, depth + 1);
                }
            }
            // `getCDKey` (`iw3mp.exe` 0x545cdf): the five fields cleared,
            // then the key's quarters with 16 characters and the check with
            // 4 (`CL_GetCDKey` 0x475ef0).
            "getcdkey" => {
                let (key, check) = cdkey::menu_key(KeyGame::Cod4);
                for i in 1..=5 {
                    self.set_dvar(&format!("cdkey{i}"), "");
                }
                if key.len() == 16 {
                    for i in 0..4 {
                        self.set_dvar(&format!("cdkey{}", i + 1), &key[i * 4..][..4]);
                    }
                }
                if check.len() == 4 {
                    self.set_dvar("cdkey5", &check);
                }
            }
            // `verifyCDKey` (0x545e8a): the first four fields are the key,
            // the fifth its check (`CL_CDKeyValidate` 0x4712e0);
            // `ui_cdkeyvalid` takes the localized verdict and a valid key
            // is set (`CL_SetCDKey` 0x475f70, whose registry write vcod
            // leaves out).
            "verifycdkey" => {
                let field = |i: u32| self.dvar(&format!("cdkey{i}")).unwrap_or_default();
                let key: String = (1..=4).map(field).collect();
                let check = field(5);
                let ok = cdkey::valid(&key, &check);
                let msg = if ok {
                    "EXE_CDKEYVALID"
                } else {
                    "EXE_CDKEYINVALID"
                };
                let text = (self.strings.as_ref())
                    .and_then(|l| l.get(msg))
                    .unwrap_or(msg)
                    .to_string();
                self.set_dvar("ui_cdkeyvalid", &text);
                if ok {
                    cdkey::store(KeyGame::Cod4, &key, &check, false);
                }
            }
            // No single player, no list refresh in the front end's own
            // scripts (the browser's are its own), nothing to sort.
            "startsingleplayer" | "stoprefresh" | "loadarenas" | "sortplayerprofiles" => {
                log::debug!("cod4 front end: uiScript {name}: nothing to do");
            }
            _ => return false,
        }
        true
    }

    /// `ui_playerProfileCount`, the listed profiles.
    fn profile_count(&mut self) {
        let n = self.front.profiles.len().to_string();
        self.ui.dvars.insert("ui_playerprofilecount".into(), n);
    }

    /// The listed profile `name`'s row, in any case.
    fn profile_row(&self, name: &str) -> Option<usize> {
        (!name.is_empty())
            .then(|| {
                self.front
                    .profiles
                    .iter()
                    .position(|p| p.eq_ignore_ascii_case(name))
            })
            .flatten()
    }

    /// `UI_SelectPlayerProfileIndex`: every open profile list box picks
    /// `row`, which `ui_playerProfileSelected` names.
    fn select_profile(&mut self, row: usize) {
        for m in self.below.iter_mut().chain(self.open.as_mut()) {
            let Some(layout) = &m.layout else {
                continue;
            };
            for (i, it) in layout.items.iter().enumerate() {
                if it.feeder == FEEDER_PROFILES {
                    let start = m.lists.get(&i).map_or(0, |l| l.0.min(row));
                    m.lists.insert(i, (start, row));
                }
            }
        }
        self.front_feeder_select(FEEDER_PROFILES, row);
    }

    /// `UI_CreatePlayerProfile`: `ui_playerProfileNameNew` (cleared) as a
    /// new profile, listed and picked; a popup says why when the list is
    /// full, the name is taken or the directory cannot be made.
    fn create_profile(&mut self, gs: &Gamestate, out: &mut Vec<String>, depth: u32) {
        let new = self.dvar("ui_playerProfileNameNew").unwrap_or_default();
        if new.is_empty() {
            return;
        }
        let mut cut = PROFILE_NAME_BYTES.min(new.len());
        while !new.is_char_boundary(cut) {
            cut -= 1;
        }
        let name = new[..cut].to_string();
        self.ui
            .dvars
            .insert("ui_playerprofilenamenew".into(), String::new());
        let popup = if self.front.profiles.len() >= MAX_PROFILES {
            Some("profile_create_too_many_popmenu")
        } else if self.profile_row(&name).is_some() {
            Some("profile_exists_popmenu")
        } else if !self
            .front
            .players_root
            .as_deref()
            .is_some_and(|r| create_profile(r, &name))
        {
            Some("profile_create_fail_popmenu")
        } else {
            None
        };
        if let Some(menu) = popup {
            self.push_menu(menu, gs, out, depth + 1);
            return;
        }
        self.front.profiles.push(name.clone());
        self.front.profiles.sort_by_key(|n| n.to_ascii_lowercase());
        self.profile_count();
        if let Some(row) = self.profile_row(&name) {
            self.select_profile(row);
        }
    }

    /// `UI_DeletePlayerProfile`: `ui_playerProfileSelected`'s directory
    /// goes, the active profile with it clearing `com_playerProfile`; the
    /// next row is picked, or none when it was the last.
    fn delete_profile(&mut self, gs: &Gamestate, out: &mut Vec<String>, depth: u32) {
        if self.front.profiles.is_empty() {
            return;
        }
        let sel = self.dvar("ui_playerProfileSelected").unwrap_or_default();
        let row = self.profile_row(&sel);
        let removed = row.is_some()
            && self
                .front
                .players_root
                .as_deref()
                .is_some_and(|r| delete_profile(r, &sel));
        let Some(row) = row.filter(|_| removed) else {
            self.push_menu("profile_delete_fail_popmenu", gs, out, depth + 1);
            return;
        };
        let active = self.dvar("com_playerProfile").unwrap_or_default();
        if active.eq_ignore_ascii_case(&sel) {
            self.ui
                .dvars
                .insert("com_playerprofile".into(), String::new());
        }
        let name = self.front.profiles.remove(row);
        self.front.requests.push(Request::ProfileDeleted(name));
        if self.front.profiles.is_empty() {
            self.ui
                .dvars
                .insert("ui_playerprofileselected".into(), String::new());
        } else {
            self.select_profile(row.min(self.front.profiles.len() - 1));
        }
        self.profile_count();
    }

    /// `uiScript StartServer`: the picked map and game type and
    /// `ui_dedicated` clamped to 0..2, for the client to start.
    fn start_server(&mut self) {
        let at = |m: &ScriptMenus, d: &str| usize::try_from(atoi(&m.dvar(d).unwrap_or_default()));
        let map = at(self, "ui_currentNetMap")
            .ok()
            .and_then(|i| self.lists.maps.get(i))
            .map(|a| a.map.clone());
        let gametype = at(self, "ui_netGameType")
            .ok()
            .and_then(|i| self.lists.gametypes.get(i))
            .map(|(g, _)| g.clone());
        let (Some(map), Some(gametype)) = (map, gametype) else {
            log::warn!("cod4 front end: StartServer with no map or game type picked");
            return;
        };
        let dedicated = atoi(&self.dvar("ui_dedicated").unwrap_or_default()).clamp(0, 2);
        // The game type pages' `scr_*` settings, by name.
        let mut names: Vec<String> = SERVER_DVARS.iter().map(|d| d.to_string()).collect();
        let mut scr: Vec<String> = (self.ui.dvars.keys())
            .filter(|d| d.starts_with("scr_"))
            .cloned()
            .collect();
        scr.sort();
        names.extend(scr);
        let cvars = names
            .into_iter()
            .filter_map(|d| {
                let v = self.dvar(&d)?;
                (!v.is_empty()).then_some((d, v))
            })
            .collect();
        self.front.requests.push(Request::StartServer {
            map,
            gametype,
            dedicated,
            cvars,
        });
    }

    /// The front end's feeders' row counts, `None` for another feeder.
    pub(super) fn front_feeder_count(&self, feeder: f32) -> Option<usize> {
        if feeder == FEEDER_MODS {
            Some(self.front.mods.len())
        } else if feeder == FEEDER_PROFILES {
            Some(self.front.profiles.len())
        } else {
            None
        }
    }

    /// A front-end feeder's row text.
    pub(super) fn front_feeder_text(&self, feeder: f32, row: usize) -> Option<String> {
        if feeder == FEEDER_MODS {
            Some(
                self.front
                    .mods
                    .get(row)
                    .map(|(_, d)| d.clone())
                    .unwrap_or_default(),
            )
        } else if feeder == FEEDER_PROFILES {
            Some(self.front.profiles.get(row).cloned().unwrap_or_default())
        } else {
            None
        }
    }

    /// A front-end feeder's pick; false for another feeder.
    pub(super) fn front_feeder_select(&mut self, feeder: f32, row: usize) -> bool {
        if feeder == FEEDER_MODS {
            self.front.mod_pick = row;
        } else if feeder == FEEDER_PROFILES {
            let name = self.front.profiles.get(row).cloned().unwrap_or_default();
            self.ui
                .dvars
                .insert("ui_playerprofileselected".into(), name);
        } else {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lists read an install laid out as 1.7 writes it: `mods/<dir>`
    /// with an optional `description.txt` (its first 0x30 bytes), and
    /// `players/profiles/<name>` with `active.txt` naming one.
    #[test]
    fn cod4_front_lists_read_mods_and_profiles() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        for d in [
            "Mods/ModWarfare",
            "Mods/plain",
            "players/profiles/Alpha",
            "players/profiles/bravo",
        ] {
            std::fs::create_dir_all(r.join(d)).unwrap();
        }
        std::fs::write(r.join("Mods/ModWarfare/description.txt"), "x".repeat(0x40)).unwrap();
        std::fs::write(r.join("players/profiles/active.txt"), "BRAVO\n").unwrap();
        assert_eq!(
            mod_list(r),
            [
                ("ModWarfare".to_string(), "x".repeat(0x30)),
                ("plain".to_string(), "plain".to_string())
            ]
        );
        assert_eq!(profile_list(r), ["Alpha", "bravo"]);
        assert_eq!(active_profile(r).as_deref(), Some("bravo"));
        assert_eq!(
            profile_dir(r, "alpha"),
            Some(r.join("players/profiles/Alpha"))
        );
        std::fs::write(r.join("players/profiles/active.txt"), "charlie").unwrap();
        assert_eq!(active_profile(r), None);
    }
}
