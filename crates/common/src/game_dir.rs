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
