//! Panic containment for the dedicated server: each boundary where one
//! input owns the work catches a panic, so it costs that input rather than
//! the process (docs/dev/server-tick.md, "Panics"). Off unless the thread
//! that ticks turned it on ([`enable`]): an in-process server (the
//! client's) and the tests see a panic unwind as before.

use std::any::Any;
use std::cell::Cell;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
    /// Boundaries this thread is inside of.
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    /// Inside a boundary whose panics are not logged just now.
    static QUIET: Cell<bool> = const { Cell::new(false) };
}

/// Level panics tolerated in [`PANIC_WINDOW`]; one more ends the process.
const MAX_LEVEL_PANICS: usize = 3;
const PANIC_WINDOW: Duration = Duration::from_secs(60);
/// At most one connectionless panic is logged this often.
const OOB_LOG_EVERY: Duration = Duration::from_secs(1);

/// What a client sees when its own packet panicked the server.
pub const PANIC_DROP_REASON: &str = "server error";

/// Turns the boundaries on for this thread, the script VM's builtin one
/// with them. The binary's loop calls it once its level is up.
pub fn enable() {
    ON.set(true);
    vcod_gsc::vm::contain_builtin_panics(true);
}

/// Whether a panic raised now on this thread is caught: the binary's panic
/// hook then logs it and leaves the process (and its tty) alone.
pub fn catching() -> bool {
    DEPTH.get() > 0
}

/// Whether the hook logs a caught panic.
pub fn loud() -> bool {
    !QUIET.get()
}

/// Marks `f` as a scope whose panics are caught, for a thread that catches
/// them itself (a bots' worker, `bots::driver::par`).
pub fn caught_scope<R>(f: impl FnOnce() -> R) -> R {
    DEPTH.set(DEPTH.get() + 1);
    let r = f();
    DEPTH.set(DEPTH.get() - 1);
    r
}

/// Runs `f`. With the boundaries on, a panic in it comes back as its
/// message, logged by the hook unless `quiet`. The caller discards or
/// resets what `f` was changing.
pub(crate) fn catch<R>(quiet: bool, f: impl FnOnce() -> R) -> Result<R, String> {
    if !ON.get() {
        return Ok(f());
    }
    let was = QUIET.replace(quiet);
    let r = caught_scope(|| catch_unwind(AssertUnwindSafe(f)));
    QUIET.set(was);
    r.map_err(|p| message(&*p))
}

/// A panic payload's text.
pub fn message(p: &(dyn Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a non-text panic".into())
}

/// The engine's record of what it caught.
#[derive(Default)]
pub struct Panics {
    /// When the level panics in the window came.
    recent: VecDeque<Instant>,
    /// Set once the level panicked too often; the binary exits on it.
    pub(crate) fatal: Option<String>,
    oob_logged: Option<Instant>,
    oob_suppressed: u32,
}

impl Panics {
    /// Counts a level panic at `now`: whether the level may be loaded again.
    fn note_level(&mut self, now: Instant) -> bool {
        while self
            .recent
            .front()
            .is_some_and(|&t| now.saturating_duration_since(t) >= PANIC_WINDOW)
        {
            self.recent.pop_front();
        }
        self.recent.push_back(now);
        self.recent.len() <= MAX_LEVEL_PANICS
    }

    /// Whether a connectionless panic at `now` stays out of the log.
    pub(crate) fn oob_quiet(&self, now: Instant) -> bool {
        self.oob_logged
            .is_some_and(|t| now.saturating_duration_since(t) < OOB_LOG_EVERY)
    }

    /// A connectionless packet's panic: logged at most once per
    /// [`OOB_LOG_EVERY`], with the count of those left out since.
    pub(crate) fn oob_panicked(&mut self, from: std::net::SocketAddr, msg: &str, now: Instant) {
        if self.oob_quiet(now) {
            self.oob_suppressed += 1;
            return;
        }
        let more = std::mem::take(&mut self.oob_suppressed);
        log::error!(
            "connectionless packet from {from} panicked ({msg}); dropped it ({more} more not logged)"
        );
        self.oob_logged = Some(now);
    }
}

impl<G: super::GameModule> super::Engine<G> {
    /// A panic left the level in an unknown state: the current map loads
    /// again through the console, as `map <name>` would, unless the level
    /// panicked [`MAX_LEVEL_PANICS`] times in [`PANIC_WINDOW`] already, or
    /// no map is named, and then the binary exits ([`Self::fatal`]).
    pub(crate) fn level_panicked(&mut self, what: &str, msg: &str, now: Instant) {
        let level_ms = self.core.sv_time_ms;
        let map = self
            .core
            .configstrings
            .first()
            .and_then(|cs| vcod_common::net::info_value_for_key(cs, "mapname"))
            .filter(|m| !m.is_empty())
            .map(str::to_string);
        let again = self.core.panics.note_level(now);
        match map {
            Some(map) if again => {
                log::error!(
                    "{what} panicked at level time {level_ms} ({msg}); loading {map} again"
                );
                self.push_console(&format!("map {map}"));
            }
            _ => {
                self.core.panics.fatal = Some(format!(
                    "{what} panicked at level time {level_ms} ({msg}), {} times in {} s",
                    self.core.panics.recent.len(),
                    PANIC_WINDOW.as_secs()
                ));
            }
        }
    }

    /// Why the binary has to exit: the level panicked too often, or a
    /// level load failed ([`super::GameModule::take_fatal`]).
    pub fn fatal(&mut self) -> Option<anyhow::Error> {
        if let Some(e) = self.core.panics.fatal.take() {
            return Some(anyhow::Error::msg(e));
        }
        G::take_fatal(self)
    }
}

/// The test-only panics the boundaries' tests arm.
#[cfg(test)]
pub(crate) mod inject {
    use std::cell::Cell;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub(crate) enum Site {
        ClientPacket,
        Frame,
    }

    thread_local! {
        static ARMED: Cell<Option<Site>> = const { Cell::new(None) };
    }

    /// The next time `site` runs on this thread it panics.
    pub(crate) fn arm(site: Site) {
        ARMED.set(Some(site));
    }

    pub(crate) fn fire(site: Site) {
        if ARMED.get() == Some(site) {
            ARMED.set(None);
            panic!("injected panic at {site:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fourth_level_panic_in_a_minute_is_fatal() {
        let t = Instant::now();
        let mut p = Panics::default();
        for k in 0..3 {
            assert!(p.note_level(t + Duration::from_secs(k)));
        }
        assert!(!p.note_level(t + Duration::from_secs(59)));
        let mut p = Panics::default();
        for k in 0..4 {
            assert!(p.note_level(t + Duration::from_secs(30 * k)), "{k}");
        }
    }
}
