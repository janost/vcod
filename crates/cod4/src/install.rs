//! A CoD4 install on disk: `zone/<language>/*.ff` fastfiles beside
//! `main/*.iwd`. `COD4_DIR` names it, as `COD2_DIR` names CoD2's.
//!
//! An install may carry an `fs_game` (`mods/<name>`): its `mod.ff` loads
//! after the common zones and its iwds sit over `main/`'s. A zone missing
//! from `zone/<language>/` is looked for under `usermaps/`. The search
//! rules are `docs/research/cod4-fastfile.md` 13.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The environment variable that names a CoD4 1.7 install.
pub const ENV: &str = "COD4_DIR";

/// An install root holding `zone/` (and, for a client install, `main/`).
#[derive(Clone, Debug)]
pub struct Install {
    root: PathBuf,
    /// `fs_game` as set (`mods/ModWarfare`); `None` when empty.
    fs_game: Option<String>,
    /// The level being loaded, whose `usermaps/<map>/` iwds join the
    /// search path under an `fs_game`.
    map: Option<String>,
}

/// The `allocFlags` each zone loads with. A later zone's asset replaces
/// an earlier one of the same type and name only when its flags are at
/// least the earlier zone's (`DB_OverrideAsset`), so the mod's assets win
/// over every zone but a map's `_load`.
pub mod zone_flags {
    pub const LOCALIZED_CODE: u8 = 0;
    pub const LOCALIZED_COMMON: u8 = 1;
    pub const CODE: u8 = 2;
    pub const COMMON: u8 = 4;
    /// The map's zone, and `ui_mp`.
    pub const MAP: u8 = 8;
    pub const MOD: u8 = 16;
    /// `<map>_load`, the loading screen's zone (client only).
    pub const LOAD: u8 = 32;
}

/// Where [`Install::zone_file`] found a zone, `DB_BuildOSPath`'s three
/// modes (iw3mp.exe 0x489f20).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoneOrigin {
    /// `zone/<language>/<name>.ff`.
    Stock,
    /// `<fs_game>/<name>.ff`: only `mod` loads from there.
    Mod,
    /// `usermaps/<map>/<name>.ff`, `<map>` being the name up to `_load`.
    Usermaps,
}

/// A zone on disk and which of [`ZoneOrigin`]'s directories held it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneFile {
    pub path: PathBuf,
    pub origin: ZoneOrigin,
}

impl Install {
    /// The install at `root`, if it has a `zone/` directory.
    pub fn at(root: impl Into<PathBuf>) -> Option<Self> {
        let root = root.into();
        root.join("zone").is_dir().then_some(Self {
            root,
            fs_game: None,
            map: None,
        })
    }

    /// This install under `fs_game` (`mods/ModWarfare`); an empty one
    /// clears it. Backslashes read as slashes.
    pub fn with_fs_game(mut self, fs_game: Option<&str>) -> Self {
        self.fs_game = fs_game
            .map(|g| g.replace('\\', "/").trim_matches('/').to_string())
            .filter(|g| !g.is_empty());
        self
    }

    /// This install loading level `map`, whose `usermaps/<map>/` iwds
    /// [`Self::iwd_dirs`] lists.
    pub fn with_map(mut self, map: Option<&str>) -> Self {
        self.map = map.filter(|m| !m.is_empty()).map(str::to_string);
        self
    }

    /// The `fs_game` set, if any.
    pub fn fs_game(&self) -> Option<&str> {
        self.fs_game.as_deref()
    }

    /// `<root>/<fs_game>` as it is on disk (each component matched
    /// case-blind, as Windows opens `mods/` for `Mods/`), if it exists.
    pub fn game_dir(&self) -> Option<PathBuf> {
        resolve_dir(&self.root, self.fs_game.as_deref()?)
    }

    /// `<fs_game>/mod.ff`, if `fs_game` is set and holds one
    /// (`DB_ModFileExists`).
    pub fn mod_zone(&self) -> Option<PathBuf> {
        let dir = self.game_dir()?;
        find_ci(&dir, "mod.ff").map(|n| dir.join(n))
    }

    /// `usermaps/<map>/` as it is on disk, if it exists.
    pub fn usermap_dir(&self, map: &str) -> Option<PathBuf> {
        resolve_dir(&self.root, &format!("usermaps/{map}"))
    }

    /// Where zone `name` loads from: `mod` from the `fs_game` directory;
    /// any other from `zone/<language>/`, else from
    /// `usermaps/<map>/<name>.ff` (iw3mp.exe 0x48a7a0). `None` when none
    /// of those holds it.
    pub fn zone_file(&self, language: &str, name: &str) -> Option<ZoneFile> {
        if name.eq_ignore_ascii_case("mod") {
            return self.mod_zone().map(|path| ZoneFile {
                path,
                origin: ZoneOrigin::Mod,
            });
        }
        let stock = self.zone(language, name);
        if stock.is_file() {
            return Some(ZoneFile {
                path: stock,
                origin: ZoneOrigin::Stock,
            });
        }
        let map = name.find("_load").map_or(name, |i| &name[..i]);
        let dir = self.usermap_dir(map)?;
        let file = find_ci(&dir, &format!("{name}.ff"))?;
        Some(ZoneFile {
            path: dir.join(file),
            origin: ZoneOrigin::Usermaps,
        })
    }

    /// [`Self::zone_file`]'s path, else the stock path (for the error).
    pub fn zone_path(&self, language: &str, name: &str) -> PathBuf {
        self.zone_file(language, name)
            .map_or_else(|| self.zone(language, name), |z| z.path)
    }

    /// The zones `Com_StartupLoad` loads ahead of any map, in load order
    /// with their flags: `ui_mp` only on a client, `mod` only when the
    /// `fs_game` directory holds one.
    pub fn startup_zones(&self, client: bool) -> Vec<(&'static str, u8)> {
        use zone_flags::*;
        let mut out = vec![
            ("code_post_gfx_mp", CODE),
            ("localized_code_post_gfx_mp", LOCALIZED_CODE),
        ];
        if client {
            out.push(("ui_mp", MAP));
        }
        out.push(("common_mp", COMMON));
        out.push(("localized_common_mp", LOCALIZED_COMMON));
        if self.mod_zone().is_some() {
            out.push(("mod", MOD));
        }
        out
    }

    /// The directories whose iwds the search path holds, lowest priority
    /// first: `main/`, then under an `fs_game` the level's
    /// `usermaps/<map>/` and the `fs_game` directory (`FS_Startup` adds
    /// them in that order, each over the last; `DB_AddUserMapDir` only
    /// under a set `fs_game`, cod4-fastfile.md 13).
    pub fn iwd_dirs(&self) -> Vec<PathBuf> {
        let mut out = vec![self.root.join("main")];
        if self.fs_game.is_some() {
            out.extend(self.map.as_deref().and_then(|m| self.usermap_dir(m)));
            out.extend(self.game_dir());
        }
        out
    }

    /// The install `$COD4_DIR` names; `None` when unset or not an install.
    pub fn from_env() -> Option<Self> {
        Self::at(std::env::var_os(ENV)?)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `zone/<language>/`, where the stock fastfiles sit (`english`).
    pub fn zone_dir(&self, language: &str) -> PathBuf {
        self.root.join("zone").join(language)
    }

    /// Every `.ff` in `zone/<language>/`, sorted by name.
    pub fn zones(&self, language: &str) -> Result<Vec<PathBuf>> {
        let dir = self.zone_dir(language);
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).with_context(|| format!("{}", dir.display()))? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("ff"))
            {
                out.push(path);
            }
        }
        out.sort();
        Ok(out)
    }

    /// The zone file `name` (`code_post_gfx_mp`) in `zone/<language>/`.
    pub fn zone(&self, language: &str, name: &str) -> PathBuf {
        self.zone_dir(language).join(format!("{name}.ff"))
    }
}

impl Install {
    /// The localized strings of `zone/<language>/localized_code_post_gfx_mp.ff`,
    /// by name: the engine's own `EXE_*` lines, the connect refusals among
    /// them.
    pub fn code_strings(
        &self,
        language: &str,
    ) -> Result<std::collections::HashMap<String, String>> {
        let path = self.zone(language, "localized_code_post_gfx_mp");
        let zone =
            crate::zone::Zone::open_path(&path).with_context(|| path.display().to_string())?;
        let walked = crate::walk::walk(zone, crate::walk::WalkOptions::default())?;
        Ok(walked
            .assets
            .localize
            .into_iter()
            .filter_map(|l| Some((l.name, l.value?)))
            .collect())
    }
}

/// `base/rel` with each component of `rel` matched case-blind where the
/// exact name is missing; `None` when a component is not there.
fn resolve_dir(base: &Path, rel: &str) -> Option<PathBuf> {
    let mut at = base.to_path_buf();
    for part in rel.split(['/', '\\']).filter(|p| !p.is_empty()) {
        if part == "." || part == ".." {
            return None;
        }
        let name = find_ci(&at, part)?;
        at.push(name);
    }
    at.is_dir().then_some(at)
}

/// `rel` under `base` with each component that exists on disk, matched
/// case-blind, spelled as it is there, and the rest as given: where a
/// server's `mods/modwarfare/z.iwd` lands in an install holding
/// `Mods/ModWarfare/`.
pub fn resolve_ci(base: &Path, rel: &Path) -> PathBuf {
    let mut at = base.to_path_buf();
    let mut found = true;
    for part in rel.iter() {
        let name = part
            .to_str()
            .filter(|_| found)
            .and_then(|p| find_ci(&at, p));
        found = name.is_some();
        at.push(name.unwrap_or_else(|| part.to_os_string()));
    }
    at
}

/// The entry of `dir` named `name`, exactly or else case-blind.
fn find_ci(dir: &Path, name: &str) -> Option<std::ffi::OsString> {
    if dir.join(name).exists() {
        return Some(name.into());
    }
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .find(|n| n.to_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

#[cfg(test)]
mod tests {
    use super::{Install, ZoneOrigin, zone_flags};

    /// A fake install: a stock map zone, a mod with `mod.ff` under `Mods/`
    /// and a usermap with its `_load` zone.
    #[test]
    fn zone_search_follows_db_build_os_path() {
        let root = std::env::temp_dir().join(format!("vcod-cod4-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["zone/english", "main", "Mods/Demo", "usermaps/mp_x"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in [
            "zone/english/mp_crash.ff",
            "Mods/Demo/mod.ff",
            "usermaps/mp_x/mp_x.ff",
            "usermaps/mp_x/mp_x_load.ff",
        ] {
            std::fs::write(root.join(f), b"").unwrap();
        }
        let stock = Install::at(&root).unwrap();
        assert!(stock.zone_file("english", "mod").is_none());
        assert_eq!(stock.startup_zones(false).len(), 4);
        let i = stock.clone().with_fs_game(Some("mods\\demo"));
        assert_eq!(i.fs_game(), Some("mods/demo"));
        assert_eq!(i.game_dir(), Some(root.join("Mods/Demo")));
        let m = i.zone_file("english", "mod").unwrap();
        assert_eq!(
            (m.origin, m.path),
            (ZoneOrigin::Mod, root.join("Mods/Demo/mod.ff"))
        );
        assert_eq!(
            i.startup_zones(false).last(),
            Some(&("mod", zone_flags::MOD))
        );
        let c = i.zone_file("english", "mp_crash").unwrap();
        assert_eq!(c.origin, ZoneOrigin::Stock);
        let l = i.zone_file("english", "mp_x_load").unwrap();
        assert_eq!(
            (l.origin, l.path),
            (
                ZoneOrigin::Usermaps,
                root.join("usermaps/mp_x/mp_x_load.ff")
            )
        );
        assert!(i.zone_file("english", "mp_y").is_none());
        // `usermaps/<map>/`'s iwds sit between `main/`'s and the mod's,
        // and only under an `fs_game`.
        assert_eq!(
            i.clone().with_map(Some("mp_x")).iwd_dirs(),
            [
                root.join("main"),
                root.join("usermaps/mp_x"),
                root.join("Mods/Demo")
            ]
        );
        assert_eq!(
            stock.clone().with_map(Some("mp_x")).iwd_dirs(),
            [root.join("main")]
        );
        assert!(i.clone().with_fs_game(Some("../x")).game_dir().is_none());
        assert!(i.with_fs_game(Some("")).fs_game().is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_strings_hold_the_cd_key_refusals() {
        let Some(install) = super::Install::from_env() else {
            return;
        };
        let s = install.code_strings("english").unwrap();
        for k in [
            "EXE_AWAITINGCDKEYAUTH",
            "EXE_ERR_INVALID_CD_KEY",
            "EXE_ERR_CDKEY_IN_USE",
            "EXE_ERR_BAD_CDKEY",
        ] {
            assert!(s.contains_key(k), "{k} missing; {} strings", s.len());
        }
    }
}
