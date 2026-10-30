//! CoD2's player profiles: `ui/playerprofile.menu`'s list
//! (`FEEDER_PLAYER_PROFILES`) and the `uiScript`s it runs
//! (`UI_RunMenuScript`, Mac 0x1548e8), over the directories under the
//! mod directory's `players/` (`Com_NewPlayerProfile` and kin, Mac
//! 0x111c08; docs/research/cod2-fonts-menus.md 4.1). Loading a profile
//! hands the app [`UiEffect::LoadProfile`], which switches the config
//! file and writes `players/active.txt`.

use std::path::{Path, PathBuf};

use vcod_common::ui_menu::{FEEDER_PLAYER_PROFILES, UiItem};

use super::{HudQuad, Painter, SCROLLBAR_SIZE, Ui, UiEffect, truncate_visible, visible_rows};
use crate::console::shell::Shell;

/// `createPlayerProfile` refuses past this many (`uiInfo + 0x280` full).
const MAX_PROFILES: usize = 64;

/// The profile list: the names `players/` held at the last
/// `addPlayerProfiles`, the order they are shown in, the selected row and
/// the first row drawn.
#[derive(Default)]
pub(super) struct Profiles {
    /// `<mod dir>/players`, `None` off a CoD2 install.
    dir: Option<PathBuf>,
    names: Vec<String>,
    /// Indices into `names`, sorted by `UI_PlayerProfilesQsortCompare`.
    order: Vec<usize>,
    /// `uiInfo + 900`: ascending, which `addPlayerProfiles` sets and
    /// `sortPlayerProfiles` flips.
    ascending: bool,
    pub(super) row: usize,
    pub(super) top: usize,
}

/// The directories under `dir` (`FS_ListFiles("players", "/")`).
pub fn list_profiles(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// The profile `players/active.txt` names, when `players/` has it
/// (`Com_InitPlayerProfiles`, Mac 0x111aa6: the file's first token, matched
/// against the directories in any case).
pub fn active_profile(players: &Path) -> Option<String> {
    let text = std::fs::read_to_string(players.join("active.txt")).ok()?;
    let name = text.split_whitespace().next()?;
    list_profiles(players)
        .into_iter()
        .find(|p| p.eq_ignore_ascii_case(name))
}

/// Whether `name` is one of the profile `uiScript`s.
pub(super) fn is_profile_script(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "addplayerprofiles"
            | "sortplayerprofiles"
            | "selectactiveplayerprofile"
            | "createplayerprofile"
            | "deleteplayerprofile"
            | "loadplayerprofile"
    )
}

impl Profiles {
    fn sort(&mut self) {
        self.order = (0..self.names.len()).collect();
        let names = &self.names;
        let up = self.ascending;
        self.order.sort_by(|&a, &b| {
            let o = names[a].to_lowercase().cmp(&names[b].to_lowercase());
            if up { o } else { o.reverse() }
        });
    }

    fn reload(&mut self) {
        self.names = self.dir.as_deref().map(list_profiles).unwrap_or_default();
        self.sort();
    }

    /// The row showing `name`, in any case.
    fn row_of(&self, name: &str) -> Option<usize> {
        self.order
            .iter()
            .position(|&i| self.names[i].eq_ignore_ascii_case(name))
    }

    fn name_at(&self, row: usize) -> Option<&str> {
        self.order.get(row).map(|&i| self.names[i].as_str())
    }

    pub(super) fn len(&self) -> usize {
        self.names.len()
    }
}

impl Ui {
    /// Where the profiles live: `<mod dir>/players` on a CoD2 install.
    pub fn set_profiles_dir(&mut self, dir: PathBuf) {
        self.profiles.dir = Some(dir);
    }

    /// `Menu_SetFeederSelection(FEEDER_PLAYER_PROFILES, row)` on every
    /// open menu: the row, and `UI_FeederSelection`'s
    /// `ui_playerProfileSelected` when a list showing the profiles is open
    /// and the row holds one.
    fn select_profile(&mut self, row: usize, out: &mut Vec<UiEffect>) {
        let open = self.open.iter().any(|&m| {
            self.menus[m]
                .items
                .iter()
                .any(|i| i.feeder == Some(FEEDER_PLAYER_PROFILES))
        });
        if !open {
            return;
        }
        self.profiles.row = row;
        if let Some(name) = self.profiles.name_at(row).map(str::to_string) {
            self.set_cvar("ui_playerProfileSelected", &name, out);
        }
    }

    fn profile_count(&mut self, out: &mut Vec<UiEffect>) {
        let n = self.profiles.len().to_string();
        self.set_cvar("ui_playerProfileCount", &n, out);
    }

    /// The profile `uiScript`s (`UI_RunMenuScript`, Mac 0x1548e8). Each
    /// does nothing without [`Self::set_profiles_dir`], so a front end with
    /// no profiles (the tests') never sets `ui_playerProfileCount` and the
    /// main menu opens no profile menu.
    pub(super) fn profile_script(&mut self, name: &str, shell: &Shell, out: &mut Vec<UiEffect>) {
        if self.profiles.dir.is_none() {
            return;
        }
        match name.to_ascii_lowercase().as_str() {
            // The directories again, ascending, the first row selected.
            "addplayerprofiles" => {
                self.profiles.ascending = true;
                self.profiles.reload();
                if self.profiles.len() != 0 {
                    self.select_profile(0, out);
                }
                self.profile_count(out);
            }
            "sortplayerprofiles" => {
                self.profiles.ascending = !self.profiles.ascending;
                if self.profiles.len() != 0 {
                    self.profiles.sort();
                    self.select_profile(0, out);
                }
            }
            "selectactiveplayerprofile" => {
                let active = self.cvar("com_playerProfile", shell).unwrap_or_default();
                if let Some(row) = self.profiles.row_of(&active) {
                    self.select_profile(row, out);
                }
            }
            // `ui_playerProfileNameNew`, cleared, as a new directory: a
            // popup for a full list, a name taken in any case or a
            // directory that could not be made; else the list sorted with
            // the new one selected.
            "createplayerprofile" => {
                let new = self
                    .cvar("ui_playerProfileNameNew", shell)
                    .unwrap_or_default();
                if new.is_empty() {
                    return;
                }
                let new: String = new.chars().take(31).collect();
                self.set_cvar("ui_playerProfileNameNew", "", out);
                if self.profiles.len() == MAX_PROFILES {
                    self.open_menu("profile_create_too_many_popmenu", shell, out);
                    return;
                }
                if self
                    .profiles
                    .names
                    .iter()
                    .any(|n| n.eq_ignore_ascii_case(&new))
                {
                    self.open_menu("profile_exists_popmenu", shell, out);
                    return;
                }
                let made = self
                    .profiles
                    .dir
                    .as_ref()
                    .is_some_and(|d| std::fs::create_dir_all(d.join(&new)).is_ok());
                if !made {
                    log::warn!("Unable to create new profile path: {new}");
                    self.open_menu("profile_create_fail_popmenu", shell, out);
                    return;
                }
                self.profiles.names.push(new.clone());
                self.profiles.sort();
                self.select_profile(0, out);
                self.profile_count(out);
                if let Some(row) = self.profiles.row_of(&new) {
                    self.select_profile(row, out);
                }
            }
            // `ui_playerProfileSelected`'s directory removed with what it
            // holds; the active profile's dvar cleared with it.
            "deleteplayerprofile" => {
                if self.profiles.len() == 0 {
                    return;
                }
                let sel = self
                    .cvar("ui_playerProfileSelected", shell)
                    .unwrap_or_default();
                let found = self
                    .profiles
                    .names
                    .iter()
                    .position(|n| !sel.is_empty() && n.eq_ignore_ascii_case(&sel));
                let removed = found.is_some_and(|i| {
                    self.profiles.dir.as_ref().is_some_and(|d| {
                        std::fs::remove_dir_all(d.join(&self.profiles.names[i])).is_ok()
                    })
                });
                let Some(i) = found.filter(|_| removed) else {
                    self.open_menu("profile_delete_fail_popmenu", shell, out);
                    return;
                };
                let active = self.cvar("com_playerProfile", shell).unwrap_or_default();
                if active.eq_ignore_ascii_case(&sel) {
                    self.set_cvar("com_playerProfile", "", out);
                }
                let row = self.profiles.row_of(&sel).unwrap_or(0);
                self.profiles.names.remove(i);
                if self.profiles.len() == 0 {
                    self.set_cvar("ui_playerProfileSelected", "", out);
                } else {
                    self.profiles.sort();
                    let row = row.min(self.profiles.len() - 1);
                    self.select_profile(row, out);
                }
                self.profile_count(out);
            }
            "loadplayerprofile" => {
                let sel = self
                    .cvar("ui_playerProfileSelected", shell)
                    .unwrap_or_default();
                let active = self.cvar("com_playerProfile", shell).unwrap_or_default();
                if !sel.is_empty()
                    && !sel.eq_ignore_ascii_case(&active)
                    && let Some(name) = self
                        .profiles
                        .names
                        .iter()
                        .find(|n| n.eq_ignore_ascii_case(&sel))
                {
                    out.push(UiEffect::LoadProfile(name.clone()));
                }
            }
            _ => {}
        }
    }

    /// A click on the profile list selects the row under the cursor; a
    /// second within the double-click time runs the list's `doubleClick`.
    pub(super) fn profile_click(&mut self, item: &UiItem, double: bool) -> bool {
        let row = ((self.cursor[1] - item.rect[1] - 1.0) / item.element_height.max(1.0)).floor();
        let row = row.max(0.0) as usize + self.profiles.top;
        if row >= self.profiles.len() {
            return false;
        }
        let again = double && self.profiles.row == row;
        self.profiles.row = row;
        again
    }

    /// The selected row's name, for the click's `ui_playerProfileSelected`.
    pub(super) fn profile_selected(&self) -> Option<String> {
        self.profiles.name_at(self.profiles.row).map(str::to_string)
    }

    /// `Item_ListBox_Paint` for `FEEDER_PLAYER_PROFILES`: each name
    /// (`UI_FeederItemText`), the selected row outlined.
    pub(super) fn profile_rows(&self, p: &Painter, item: &UiItem, out: &mut Vec<HudQuad>) {
        let [x, y0, w, _] = item.rect;
        let (x, mut y) = (x + 1.0, y0 + 1.0);
        let eh = item.element_height.max(1.0);
        let col = item.columns.first().copied();
        for row in (self.profiles.top..self.profiles.len()).take(visible_rows(item)) {
            let name = self.profiles.name_at(row).unwrap_or_default();
            let text = truncate_visible(name, col.map_or(0, |c| c.max_chars));
            p.text(
                &text,
                x + 4.0 + col.map_or(0.0, |c| c.pos) + item.text_align_x,
                y + eh + item.text_align_y,
                item.text_scale,
                item.fore,
                out,
            );
            if row == self.profiles.row {
                p.fill(
                    [x, y, w - SCROLLBAR_SIZE - 4.0, eh - 1.0],
                    item.outline_color,
                    out,
                );
            }
            y += eh;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_active_profile_is_a_directory_named_in_active_txt() {
        let dir = std::env::temp_dir().join(format!("vcod-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Able")).unwrap();
        std::fs::create_dir_all(dir.join("baker")).unwrap();
        assert_eq!(list_profiles(&dir), ["Able", "baker"]);
        assert_eq!(active_profile(&dir), None);
        std::fs::write(dir.join("active.txt"), "able").unwrap();
        assert_eq!(active_profile(&dir).as_deref(), Some("Able"));
        std::fs::write(dir.join("active.txt"), "charlie").unwrap();
        assert_eq!(active_profile(&dir), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
