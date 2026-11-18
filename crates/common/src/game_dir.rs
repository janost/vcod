//! The binaries are meant to sit inside the CoD 1.1 install next to
//! `CoDMP.exe`, hence the executable-directory fallback.

use std::path::{Path, PathBuf};

/// `$COD_DIR`, else the executable's directory, else the working directory.
pub fn default_game_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("COD_DIR") {
        return PathBuf::from(dir);
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// What to do about an install that is not where the binary looked.
pub const INSTALL_HINT: &str =
    "pass --game-dir <install> or set COD_DIR to the folder that holds main/ (zone/ for CoD4)";

/// How `--game-dir` was picked, for an error about it: the flag when
/// `flag` is set, else [`default_game_dir`]'s source.
pub fn chosen_by(flag: bool) -> &'static str {
    if flag {
        "--game-dir"
    } else if std::env::var_os("COD_DIR").is_some() {
        "COD_DIR"
    } else {
        "the executable's directory"
    }
}

/// Fails unless `root` holds the directory `game` reads first: `zone/`
/// for CoD4, else `mod_dir` (`main`, or `uo`). `chosen` is [`chosen_by`].
pub fn check_install(
    root: &Path,
    game: crate::game::Game,
    mod_dir: &str,
    chosen: &str,
) -> anyhow::Result<()> {
    let sub = match game {
        crate::game::Game::Cod4 => "zone",
        _ => mod_dir,
    };
    let dir = root.join(sub);
    if dir.is_dir() {
        return Ok(());
    }
    anyhow::bail!("no {sub}/ in {} ({chosen}): {INSTALL_HINT}", root.display())
}

/// The hint for a map that was not found under `main/` while `root`
/// holds United Offensive's paks beside it.
pub fn uo_map_hint(root: &Path, mod_dir: &str) -> Option<&'static str> {
    let uo_paks = std::fs::read_dir(root.join("uo")).ok()?.flatten().any(|e| {
        e.file_name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .starts_with("pakuo")
    });
    (mod_dir == "main" && uo_paks).then_some("United Offensive maps need --mod-dir uo")
}

/// `name` as a relative path that cannot leave the directory it is joined to, or `None`.
pub fn confined_rel_path(name: &str) -> Option<PathBuf> {
    // Retail's file layer refuses `..` and `::` anywhere in a path it
    // creates (CoD4 1.7 0x8186c78, docs/research/cod4-handshake.md 14.1);
    // vcod adds a root, a drive prefix, any colon and control characters.
    if name.contains("..")
        || name.starts_with(['/', '\\'])
        || name.bytes().any(|b| b < b' ' || b == 0x7f || b == b':')
    {
        return None;
    }
    let parts: Vec<&str> = name
        .split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    (!parts.is_empty()).then(|| parts.iter().collect())
}

/// `dir/name` when `name` passes [`confined_rel_path`].
pub fn confined(dir: &Path, name: &str) -> Option<PathBuf> {
    Some(dir.join(confined_rel_path(name)?))
}

/// `dir/name` when `name` is one component that passes [`confined_rel_path`]:
/// a profile's directory.
pub fn confined_component(dir: &Path, name: &str) -> Option<PathBuf> {
    if name.contains(['/', '\\']) {
        return None;
    }
    confined(dir, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_install_names_the_directory_and_the_fix() {
        use crate::game::Game;
        let root = tempfile::tempdir().unwrap();
        let e = check_install(root.path(), Game::Cod1, "main", "COD_DIR").unwrap_err();
        let e = e.to_string();
        assert!(e.starts_with("no main/ in "), "{e}");
        assert!(e.contains("(COD_DIR)") && e.contains("--game-dir"), "{e}");
        let e = check_install(root.path(), Game::Cod4, "main", "--game-dir").unwrap_err();
        assert!(e.to_string().starts_with("no zone/ in "), "{e}");
        std::fs::create_dir_all(root.path().join("main")).unwrap();
        assert!(check_install(root.path(), Game::Cod1, "main", "x").is_ok());
        assert!(check_install(root.path(), Game::Cod1, "uo", "x").is_err());
    }

    #[test]
    fn uo_paks_beside_main_give_the_mod_dir_hint() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(uo_map_hint(root.path(), "main"), None);
        std::fs::create_dir_all(root.path().join("uo")).unwrap();
        std::fs::write(root.path().join("uo/PakUO00.pk3"), b"").unwrap();
        assert!(uo_map_hint(root.path(), "main").is_some());
        assert_eq!(uo_map_hint(root.path(), "uo"), None);
    }

    #[test]
    fn confined_rel_path_keeps_names_under_their_directory() {
        let ok = |n: &str, want: &str| assert_eq!(confined_rel_path(n), Some(PathBuf::from(want)));
        ok("games_mp.log", "games_mp.log");
        ok("sub/vw3.cfg", "sub/vw3.cfg");
        ok("mods/m", "mods/m");
        ok("sub\\vw3.cfg", "sub/vw3.cfg");
        ok("sub//./x.cfg", "sub/x.cfg");
        ok("~~zz_mod.iwd", "~~zz_mod.iwd");
        for bad in [
            "",
            ".",
            "/",
            "..",
            "../x",
            "sub/../../x",
            "a..b.cfg",
            "..\\x",
            "/abs/x.cfg",
            "\\abs\\x.cfg",
            "\\\\host\\share\\x",
            "C:x",
            "C:\\x.cfg",
            "c:/x.cfg",
            "main/a.iwd:s",
            "a::b",
            "a\0b",
            "a\nb",
        ] {
            assert_eq!(confined_rel_path(bad), None, "{bad:?}");
            assert_eq!(confined_component(Path::new("d"), bad), None, "{bad:?}");
        }
        let d = Path::new("d");
        assert_eq!(confined(d, "a/b"), Some(d.join("a").join("b")));
        assert_eq!(confined_component(d, "Able"), Some(d.join("Able")));
        assert_eq!(confined_component(d, "a/b"), None);
        assert_eq!(confined_component(d, "a\\b"), None);
    }
}
