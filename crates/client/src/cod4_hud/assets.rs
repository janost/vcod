//! The zones the HUD reads: fonts, images, strings, script menus and their
//! startup dvars, loaded on first use beside the map's.

use std::collections::HashMap;
use std::path::Path;

use vcod_cod4::hud::{self as h, HudAssets, KillIcon, MessageWindow};
use vcod_common::localize::Localized;

use super::fonts::Fonts;
use super::{Cod4Hud, image_of};

/// What the zones gave, once loaded.
pub(super) struct Loaded {
    pub(super) fonts: Fonts,
    pub(super) images: HashMap<String, String>,
    pub(super) localized: std::sync::Arc<Localized>,
    pub(super) kill_icons: HashMap<String, KillIcon>,
    pub(super) windows: HashMap<i32, MessageWindow>,
    pub(super) compass: HashMap<String, h::MenuRect>,
    pub(super) rank_icons: HashMap<(i32, i32), String>,
    pub(super) rank_levels: HashMap<i32, String>,
    pub(super) weapon_hud: HashMap<String, h::WeaponHud>,
    pub(super) weapon_info: HashMap<String, h::MenuItem>,
    pub(super) killcam: Vec<h::MenuItem>,
    pub(super) script_menus: HashMap<String, vcod_cod4::scriptmenu::ScriptMenu>,
    tables: HashMap<String, h::Table>,
    pub(super) gametypes: HashMap<String, String>,
    pub(super) ui_lists: UiLists,
    /// The startup configs' dvars by lowercased name, the last `set` of
    /// each (`h::startup_dvars`).
    startup_dvars: HashMap<String, String>,
    /// The configs the Options pages `exec` (`play::cod4_menus::MENU_CFGS`),
    /// by name.
    menu_cfgs: HashMap<String, String>,
    /// The last [`Loaded::ammo_slots`] and the list it was for.
    pub(super) slots: std::cell::RefCell<Option<AmmoSlots>>,
}

type AmmoSlots = (std::sync::Arc<[String]>, std::sync::Arc<[(usize, usize)]>);

impl Loaded {
    /// [`h::ammo_slots`] of the weapon list over the level's weapon defs,
    /// kept while the list stays the same one
    /// ([`crate::play::cod4::weapon_list`] hands out one per change).
    pub(super) fn ammo_slots(
        &self,
        list: &std::sync::Arc<[String]>,
    ) -> std::sync::Arc<[(usize, usize)]> {
        let mut kept = self.slots.borrow_mut();
        match &*kept {
            Some((l, slots)) if std::sync::Arc::ptr_eq(l, list) => slots.clone(),
            _ => {
                let slots: std::sync::Arc<[(usize, usize)]> =
                    h::ammo_slots(list, &self.weapon_hud).into();
                *kept = Some((list.clone(), slots.clone()));
                slots
            }
        }
    }
}

/// The install's startup configs' dvars ([`h::startup_dvars`]) and the
/// Options pages' configs out of its iwds, none when they do not open.
fn startup_dvars(
    install: &vcod_cod4::install::Install,
) -> (HashMap<String, String>, HashMap<String, String>) {
    let dirs = install.iwd_dirs();
    match vcod_common::pk3::Pk3Fs::open_iwd_layers(&dirs[0], &dirs[1..]) {
        Ok(fs) => (
            h::startup_dvars(|n| fs.read(n))
                .into_iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), v))
                .collect(),
            crate::play::cod4_menus::MENU_CFGS
                .iter()
                .filter_map(|&n| {
                    Some((
                        n.to_string(),
                        String::from_utf8_lossy(&fs.read(n)?).into_owned(),
                    ))
                })
                .collect(),
        ),
        Err(e) => {
            log::warn!("cod4 hud: no startup configs: {e:#}");
            Default::default()
        }
    }
}

/// The call vote's lists (cod4-hud.md 12.6).
type UiLists = crate::play::cod4_menus::Lists;

/// The zones' script menus by lowercased name.
pub(crate) type ScriptMenuDefs = HashMap<String, vcod_cod4::scriptmenu::ScriptMenu>;

/// What the script menus draw with (`cod4_menu`).
pub(crate) struct MenuAssets<'a> {
    pub(crate) fonts: &'a Fonts,
    pub(super) l: &'a Loaded,
}

impl MenuAssets<'_> {
    /// A material's HUD texture.
    pub(crate) fn image(&self, material: &str) -> Option<String> {
        image_of(self.l, material)
    }

    /// `UI_SafeTranslateString`: a localized key's text, else the key.
    pub(crate) fn translate<'b>(&'b self, key: &'b str) -> &'b str {
        self.l.localized.get(key).unwrap_or(key)
    }
}

/// A level's HUD zones read off the window's thread
/// ([`Cod4Hud::read_level`]).
pub(crate) struct LevelZones {
    pub(super) map: String,
    pub(super) fs_game: Option<String>,
    pub(super) loaded: Option<Box<Loaded>>,
}

impl Cod4Hud {
    /// The zones' fonts and strings for `map`, out of the install whose
    /// `main/` is `main_dir`.
    #[cfg(test)]
    pub(crate) fn load(&mut self, main_dir: &Path, map: &str) {
        self.load_under(main_dir, map, None);
    }

    /// [`Self::load`] under `fs_game`, the mod's `mod.ff` over the stock
    /// zones; loaded again when `map` or `fs_game` changes: a level's
    /// minimap image and the strings of its gametype scripts ride its own
    /// zone. True when it loaded.
    pub(crate) fn load_under(&mut self, main_dir: &Path, map: &str, fs_game: Option<&str>) -> bool {
        self.load_zones(main_dir, Some(map), fs_game)
    }

    /// The front end's zones (`HudAssets::load_front`) under `fs_game`,
    /// for the main menu (`cod4_front`); loaded again when `fs_game`
    /// changes. True when it loaded.
    pub(crate) fn load_front(&mut self, main_dir: &Path, fs_game: Option<&str>) -> bool {
        self.load_zones(main_dir, None, fs_game)
    }

    /// `map`'s zones over the HUD's, or with none the front end's.
    pub(super) fn load_zones(
        &mut self,
        main_dir: &Path,
        map: Option<&str>,
        fs_game: Option<&str>,
    ) -> bool {
        if self.loaded.is_some()
            && self.fs_game.as_deref() == fs_game
            && self.zones_map.as_deref() == map
        {
            return false;
        }
        self.zones_map = map.map(str::to_string);
        self.load_with(main_dir, fs_game, |i| match map {
            Some(map) => HudAssets::load(i, map),
            None => HudAssets::load_front(i),
        })
    }

    /// The stock HUD zones alone, no map's: what the connect screen draws
    /// with before the gamestate names one.
    pub(crate) fn load_connect(&mut self, main_dir: &Path) -> bool {
        self.zones_map = None;
        self.load_with(main_dir, None, |i| HudAssets::load_zones(i, h::HUD_ZONES))
    }

    /// A level's zones as [`Self::load_under`] reads them, on any
    /// thread, for [`Self::install_level`]: `assets` when the map's load
    /// built them off its own walks (`cod4_fly::load_from`), else read.
    pub(crate) fn read_level(
        main_dir: &Path,
        map: &str,
        fs_game: Option<&str>,
        assets: Option<HudAssets>,
    ) -> LevelZones {
        LevelZones {
            map: map.to_string(),
            fs_game: fs_game.map(str::to_string),
            loaded: Self::read(main_dir, fs_game, |i| match assets {
                Some(a) => Ok(a),
                None => HudAssets::load(i, map),
            }),
        }
    }

    /// `z` in place of the zones held, as [`Self::load_under`] of its map
    /// would have loaded them; false, keeping the held ones, when they are
    /// that map's already.
    pub(crate) fn install_level(&mut self, z: LevelZones) -> bool {
        if self.loaded.is_some()
            && self.fs_game == z.fs_game
            && self.zones_map.as_deref() == Some(&z.map)
        {
            return false;
        }
        self.zones_map = Some(z.map);
        self.fs_game = z.fs_game;
        self.loaded = Some(z.loaded);
        true
    }

    fn load_with(
        &mut self,
        main_dir: &Path,
        fs_game: Option<&str>,
        assets: impl FnOnce(&vcod_cod4::install::Install) -> anyhow::Result<HudAssets>,
    ) -> bool {
        self.fs_game = fs_game.map(str::to_string);
        self.loaded = Some(Self::read(main_dir, fs_game, assets));
        true
    }

    pub(super) fn read(
        main_dir: &Path,
        fs_game: Option<&str>,
        assets: impl FnOnce(&vcod_cod4::install::Install) -> anyhow::Result<HudAssets>,
    ) -> Option<Box<Loaded>> {
        main_dir
            .parent()
            .and_then(vcod_cod4::install::Install::at)
            .map(|i| i.with_fs_game(fs_game))
            .ok_or_else(|| anyhow::anyhow!("{} is no CoD4 install", main_dir.display()))
            .and_then(|i| Ok((assets(&i)?, startup_dvars(&i))))
            .inspect_err(|e| log::warn!("cod4 hud: {e:#}"))
            .ok()
            .and_then(|(a, (startup_dvars, menu_cfgs))| {
                let fonts = Fonts::load(&a);
                if fonts.is_none() {
                    log::warn!("cod4 hud: the zones lack the UI fonts");
                }
                let mut localized = Localized::default();
                for (k, v) in &a.localize {
                    localized.insert(k, v);
                }
                Some(Box::new(Loaded {
                    fonts: fonts?,
                    images: a.images,
                    localized: std::sync::Arc::new(localized),
                    kill_icons: a.kill_icons,
                    windows: a.windows,
                    compass: a.compass,
                    rank_icons: a.rank_icons,
                    rank_levels: a.rank_levels,
                    weapon_hud: a.weapon_hud,
                    weapon_info: a.weapon_info,
                    killcam: a.killcam,
                    script_menus: a.script_menus,
                    tables: a.tables,
                    ui_lists: UiLists {
                        maps: a.arenas,
                        gametypes: a
                            .gametype_order
                            .iter()
                            .map(|g| {
                                let key = a.gametypes.get(&g.to_ascii_lowercase());
                                (g.clone(), key.cloned().unwrap_or_else(|| g.clone()))
                            })
                            .collect(),
                    },
                    gametypes: a.gametypes,
                    startup_dvars,
                    menu_cfgs,
                    slots: Default::default(),
                }))
            })
    }

    /// The zones' script menus and string tables, once loaded
    /// (`play::cod4_menus`).
    pub(crate) fn script_menus(&self) -> Option<(&ScriptMenuDefs, &HashMap<String, h::Table>)> {
        let l = self.loaded.as_ref()?.as_ref()?;
        Some((&l.script_menus, &l.tables))
    }

    /// The startup configs' dvars, once loaded: what the menus read
    /// under the server's (`play::cod4_menus`).
    pub(crate) fn startup_dvars(&self) -> Option<&HashMap<String, String>> {
        Some(&self.loaded.as_ref()?.as_ref()?.startup_dvars)
    }

    /// The Options pages' configs, once loaded.
    pub(crate) fn menu_cfgs(&self) -> Option<&HashMap<String, String>> {
        Some(&self.loaded.as_ref()?.as_ref()?.menu_cfgs)
    }

    /// The call vote's map and game type lists, once loaded.
    pub(crate) fn ui_lists(&self) -> Option<&UiLists> {
        Some(&self.loaded.as_ref()?.as_ref()?.ui_lists)
    }

    /// The fonts, images and strings the script menus draw with.
    pub(crate) fn menu_assets(&self) -> Option<MenuAssets<'_>> {
        let l = self.loaded.as_ref()?.as_ref()?;
        Some(MenuAssets { fonts: &l.fonts, l })
    }

    pub(super) fn localized(&self) -> Option<&Localized> {
        Some(&self.loaded.as_ref()?.as_ref()?.localized)
    }

    /// The zones' strings, shared with the script menus' `locString()`
    /// and `keyBinding()`.
    pub(crate) fn strings(&self) -> Option<std::sync::Arc<Localized>> {
        Some(self.loaded.as_ref()?.as_ref()?.localized.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map change reloads the zones: the next level's minimap image is
    /// in, where the stale load had only the last map's, and the selector's
    /// `@PLATFORM_PRESS_TO_SET_AIRSTRIKE` reads as its text on both. The
    /// selector seen in play after a rotation to mp_backlot drew no map under a
    /// raw key (C4-70).
    #[test]
    fn a_new_map_reloads_the_zones_and_the_selector_reads_localized() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let main = install.root().join("main");
        let shell = crate::console::shell::Shell::new();
        let mut hud = Cod4Hud::default();
        assert!(hud.load_under(&main, "mp_crash", None));
        assert!(hud.texture_of("compass_map_mp_crash").is_some());
        assert!(!hud.load_under(&main, "mp_crash", None), "same map: kept");
        assert!(hud.load_under(&main, "mp_backlot", None));
        assert!(hud.texture_of("compass_map_mp_backlot").is_some());
        let texts = hud.fullscreen_texts(&shell);
        println!("{texts:?}");
        assert!(!texts.is_empty());
        assert!(
            texts
                .iter()
                .all(|t| !t.contains('@') && !t.contains("PLATFORM_")),
            "{texts:?}"
        );
    }
}
