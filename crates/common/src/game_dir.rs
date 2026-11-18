//! The binaries are meant to sit inside the CoD 1.1 install next to
//! `CoDMP.exe`, hence the executable-directory fallback.

use std::path::PathBuf;

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

#[cfg(test)]
mod tests {
    use super::*;

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
        }
    }
}
