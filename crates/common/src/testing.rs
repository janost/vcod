//! Test helpers for every crate. Always compiled because the client crate's
//! tests call into it.

use crate::pk3::Pk3Fs;
use std::path::PathBuf;

/// `$COD_DIR`, else the test binary's directory, which never holds the game.
pub fn game_dir() -> PathBuf {
    crate::game_dir::default_game_dir()
}

/// `None` when the game is not installed; callers return early so the run
/// stays green.
pub fn game_fs() -> Option<Pk3Fs> {
    let dir = game_dir().join("main");
    if !dir.is_dir() {
        return None;
    }
    Pk3Fs::open(&dir).ok()
}

/// `maps/mp/mp_pavlov.bsp` from the retail paks.
pub fn real_bsp() -> Option<Vec<u8>> {
    game_fs()?.read("maps/mp/mp_pavlov.bsp")
}

/// A committed capture under this crate's `tests/fixtures/`, by relative path.
pub fn fixture(rel: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()))
}
