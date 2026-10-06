//! SIGINT, SIGTERM and SIGHUP (Ctrl-C and console close on Windows) as a
//! quit request, so an online client leaves through `NetClient::disconnect`
//! instead of holding its server slot until `sv_timeout`.

use std::sync::atomic::{AtomicBool, Ordering};

static REQUESTED: AtomicBool = AtomicBool::new(false);

/// Installs the handler. The first signal sets the flag the main loops poll;
/// a second one exits at once, for a loop that never polls it.
pub fn install() {
    let installed = ctrlc::set_handler(|| {
        if REQUESTED.swap(true, Ordering::Relaxed) {
            std::process::exit(130);
        }
    });
    if let Err(e) = installed {
        log::warn!("no quit signal handler: {e}");
    }
}

/// Whether a quit signal has arrived.
pub fn requested() -> bool {
    REQUESTED.load(Ordering::Relaxed)
}
