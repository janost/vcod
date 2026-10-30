//! Which game an install is. A CoD1 install's version (1.1, 1.5, United
//! Offensive) is its [`crate::net::protocol::Protocol`]
//! ([`crate::net::protocol::install_protocol`]); the game sits above it.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Game {
    /// Call of Duty (2003): 1.1, 1.5 and United Offensive.
    Cod1,
    /// Call of Duty 2. Detected only.
    Cod2,
    /// Call of Duty 4. Detected only.
    Cod4,
}

impl Game {
    /// The game installed at `root` (the directory that holds `main/`):
    /// CoD4 by a `zone/` or `iw3mp.exe` (its `main/` holds `iw_*.iwd`
    /// too), CoD2 by a `main/iw_*.iwd` or `CoD2MP_s.exe`, else CoD1. Names match case-insensitively, as a
    /// Windows install copied elsewhere may change their case.
    pub fn detect(root: &Path) -> Game {
        // (lowercased name, is a directory, path) of each entry.
        let entries = |dir: &Path| -> Vec<(String, bool, std::path::PathBuf)> {
            std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| {
                    let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                    let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                    (name, is_dir, e.path())
                })
                .collect()
        };
        let top = entries(root);
        let has_file = |n: &str| top.iter().any(|(f, d, _)| !d && f == n);
        let main = top
            .iter()
            .find(|(f, d, _)| *d && f == "main")
            .map(|(_, _, p)| entries(p))
            .unwrap_or_default();
        let iwd = main
            .iter()
            .any(|(f, d, _)| !d && f.starts_with("iw_") && f.ends_with(".iwd"));
        if top.iter().any(|(f, d, _)| *d && f == "zone") || has_file("iw3mp.exe") {
            Game::Cod4
        } else if iwd || has_file("cod2mp_s.exe") {
            Game::Cod2
        } else {
            Game::Cod1
        }
    }

    /// `--game` when given, else what `root` holds.
    pub fn resolve(flag: Option<Game>, root: &Path) -> Game {
        flag.unwrap_or_else(|| Game::detect(root))
    }

    /// The master server its browser asks and its servers heartbeat to:
    /// cod.pm's, on the game's port. Retail's are CoDMP.exe 0x566120
    /// (cod11-front-end.md) and cod2_lnxded `SV_MasterHeartbeat` 0x8096ed6
    /// (cod2-protocol.md).
    pub const fn master(self) -> crate::net::master::Master {
        use crate::net::master::{COD1_MASTER, COD2_MASTER, COD4_MASTER};
        match self {
            Game::Cod1 => COD1_MASTER,
            Game::Cod2 => COD2_MASTER,
            Game::Cod4 => COD4_MASTER,
        }
    }

    /// Fails for a game vcod cannot run yet, before anything opens its data.
    pub fn ensure_supported(self) -> anyhow::Result<()> {
        match self {
            Game::Cod1 => Ok(()),
            Game::Cod2 => anyhow::bail!("CoD2 support is not implemented yet"),
            Game::Cod4 => anyhow::bail!("CoD4 support is not implemented yet"),
        }
    }
}

impl std::str::FromStr for Game {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "cod1" => Ok(Game::Cod1),
            "cod2" => Ok(Game::Cod2),
            "cod4" => Ok(Game::Cod4),
            _ => Err(format!("unknown game {s:?}: cod1, cod2 or cod4")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn install(files: &[&str], dirs: &[&str]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for d in dirs {
            fs::create_dir_all(root.path().join(d)).unwrap();
        }
        for f in files {
            let p = root.path().join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, b"").unwrap();
        }
        root
    }

    #[test]
    fn detects_each_game_by_its_marker() {
        let cases: [(&[&str], &[&str], Game); 9] = [
            (&["main/pak0.pk3", "CoDMP.exe"], &[], Game::Cod1),
            (&["main/pak0.pk3", "uo/pakuo00.pk3"], &[], Game::Cod1),
            (&[], &[], Game::Cod1),
            (&["main/iw_00.iwd"], &[], Game::Cod2),
            (&["CoD2MP_s.exe"], &["main"], Game::Cod2),
            (&["iw3mp.exe"], &[], Game::Cod4),
            (&[], &["zone/english"], Game::Cod4),
            // A CoD4 install's `main/` holds `iw_*.iwd` as CoD2's does.
            (&["main/iw_00.iwd", "iw3mp.exe"], &[], Game::Cod4),
            (&["main/iw_00.iwd"], &["zone/english"], Game::Cod4),
        ];
        for (files, dirs, want) in cases {
            let root = install(files, dirs);
            assert_eq!(Game::detect(root.path()), want, "{files:?} {dirs:?}");
        }
    }

    #[test]
    fn markers_match_by_kind_and_case() {
        // A directory named like the marker file, or a pak that is not an iwd,
        // is no CoD2.
        let root = install(&["main/iw_00.pk3", "zone"], &["CoD2MP_s.exe"]);
        assert_eq!(Game::detect(root.path()), Game::Cod1);
        let root = install(&["MAIN/IW_07.IWD"], &[]);
        assert_eq!(Game::detect(root.path()), Game::Cod2);
    }

    #[test]
    fn the_flag_overrides_detection() {
        let root = install(&["main/iw_00.iwd"], &[]);
        assert_eq!(Game::resolve(Some(Game::Cod1), root.path()), Game::Cod1);
        assert_eq!(Game::resolve(None, root.path()), Game::Cod2);
        assert_eq!("CoD4".parse::<Game>(), Ok(Game::Cod4));
        assert!("cod3".parse::<Game>().is_err());
        assert!(Game::Cod1.ensure_supported().is_ok());
        let e = Game::Cod2.ensure_supported().unwrap_err().to_string();
        assert_eq!(e, "CoD2 support is not implemented yet");
    }
}
