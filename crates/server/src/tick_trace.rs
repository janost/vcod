//! The tick's step log (`VCOD_TICK_TRACE=<path>`): one line per step of
//! `SV_Frame` and the game frames inside it, in the order they ran, named
//! after the retail function the step reproduces, with the slot or entity
//! it ran for. Two runs that differ in step order part on the first line
//! that moved.
//!
//! The sink is per thread, so `bot_match`'s parallel matches each write
//! their own file; [`install`] opens it, [`finish`] flushes and closes it.
//! Off (no sink installed anywhere) a step costs one relaxed load and a
//! branch, and the [`step!`] arguments are never formatted.

use std::cell::RefCell;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set once any thread installs a sink; never cleared, so a thread without
/// one pays a thread-local lookup per step after that.
static ON: AtomicBool = AtomicBool::new(false);

thread_local! {
    static SINK: RefCell<Option<std::io::BufWriter<std::fs::File>>> = const { RefCell::new(None) };
}

/// Writes this thread's steps to `path`, replacing the file.
pub fn install(path: &std::path::Path) -> std::io::Result<()> {
    let f = std::fs::File::create(path)?;
    SINK.with(|s| *s.borrow_mut() = Some(std::io::BufWriter::new(f)));
    ON.store(true, Ordering::Relaxed);
    Ok(())
}

/// Flushes and closes this thread's sink.
pub fn finish() -> std::io::Result<()> {
    match SINK.with(|s| s.borrow_mut().take()) {
        Some(mut w) => w.flush(),
        None => Ok(()),
    }
}

#[inline(always)]
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// One line into this thread's sink, if it has one.
#[cold]
pub fn write(args: std::fmt::Arguments<'_>) {
    SINK.with(|s| {
        if let Some(w) = s.borrow_mut().as_mut() {
            let _ = w.write_fmt(args);
            let _ = w.write_all(b"\n");
        }
    });
}

/// `step!("ClientThink slot {slot}")`: a line in the tick trace, formatted
/// only while a sink is installed.
macro_rules! step {
    ($($arg:tt)*) => {
        if $crate::tick_trace::on() {
            $crate::tick_trace::write(format_args!($($arg)*));
        }
    };
}
pub(crate) use step;
