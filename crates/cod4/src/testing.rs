//! Install lookups for tests. Each returns `None` without its environment
//! variable, so CI (no game data) passes vacuously.

use crate::install::Install;

/// The client install `$COD4_DIR` names (`zone/english/`, `main/`).
pub fn cod4_install() -> Option<Install> {
    Install::from_env()
}

/// The retail 1.7 dedicated server's install, `$COD4_LNXDED_DIR`: its own
/// `zone/english/` holds the subset of zones the server ships with.
pub fn cod4_lnxded_install() -> Option<Install> {
    Install::at(std::env::var_os("COD4_LNXDED_DIR")?)
}
