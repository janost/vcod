//! Install lookups for tests. Each returns `None` without its environment
//! variable, so CI (no game data) passes, and panics when the variable is set
//! but names no install ([`vcod_common::testing::install_root`]).

use crate::install::Install;

/// The client install `$COD4_DIR` names (`zone/english/`, `main/`).
pub fn cod4_install() -> Option<Install> {
    Install::at(vcod_common::testing::install_root(
        crate::install::ENV,
        "cod4",
        "zone",
    )?)
}

/// The retail 1.7 dedicated server's install, `$COD4_LNXDED_DIR`: its own
/// `zone/english/` holds the subset of zones the server ships with.
pub fn cod4_lnxded_install() -> Option<Install> {
    Install::at(vcod_common::testing::install_root(
        "COD4_LNXDED_DIR",
        "cod4_lnxded",
        "zone",
    )?)
}
