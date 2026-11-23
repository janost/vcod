//! `--client-log <path>` (CoD1 only): what each human client sent and was
//! sent, one line per event, flushed as written so a capture script can
//! follow it. It is how a retail client's usercmds and client commands are
//! captured against vcod-server (docs/dev/retail.md, "Retail client
//! captures"); `tools/capture/retail-client/` reads it.
//!
//! Every line is a kind, then `key=value` fields; `text=` (a client
//! command) and `name=` are last and run to the end of the line, escaped by
//! [`escape`].
//! `w` is wall milliseconds since the log opened, `svs` the server's
//! `svs.time`, `st` a usercmd's `serverTime`.
//!
//! - `enter w svs slot proto addr name`: a client entered the world.
//! - `move w svs slot seq ack delta n`: a move op, its `n` cmds next.
//! - `cmd slot st b wb w fl a mv`: one usercmd as decoded (`a` the three
//!   angle shorts, `mv` forward, right, up).
//! - `ccmd w svs slot seq ok text`: a new client command; `ok=0` when the
//!   flood window kept it from the game.
//! - `ran slot st ev`: the events a cmd's pmove raised, `event:parm`.
//! - `snap svs slot msg time ct pm_type health weapon hint hud es ev`: a
//!   snapshot sent; `hint` is `serverCursorHint,Val,String`, `hud` the
//!   archived and current hudelem counts, `es` the eventSequence, `ev` the
//!   four event slots as `event:parm`.
//!
//! Like the tick trace the sink is per thread: the server installs it on
//! the thread that runs the loop, a test on its own.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use vcod_common::net::msg::UserCmd;

static ON: AtomicBool = AtomicBool::new(false);

struct Sink {
    out: std::io::LineWriter<std::fs::File>,
    opened: Instant,
}

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

/// Writes this thread's client log to `path`, replacing the file, headed
/// by `header` as a `#` line.
pub fn install(path: &std::path::Path, header: &str) -> std::io::Result<()> {
    let mut out = std::io::LineWriter::new(std::fs::File::create(path)?);
    writeln!(out, "# {header}")?;
    SINK.with(|s| {
        *s.borrow_mut() = Some(Sink {
            out,
            opened: Instant::now(),
        })
    });
    ON.store(true, Ordering::Relaxed);
    Ok(())
}

/// Closes this thread's sink.
pub fn finish() {
    SINK.with(|s| s.borrow_mut().take());
}

#[inline(always)]
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// One line into this thread's sink, if it has one; `line` gets the wall
/// time in milliseconds.
#[cold]
pub fn write(line: impl FnOnce(f64) -> String) {
    SINK.with(|s| {
        if let Some(sink) = s.borrow_mut().as_mut() {
            let w = sink.opened.elapsed().as_secs_f64() * 1000.0;
            let _ = writeln!(sink.out, "{}", line(w));
        }
    });
}

/// A `cmd` line's fields after the kind.
pub fn cmd_fields(slot: usize, c: &UserCmd) -> String {
    format!(
        "slot={slot} st={} b={} wb={} w={} fl={} a={},{},{} mv={},{},{}",
        c.server_time,
        c.buttons,
        c.wbuttons,
        c.weapon,
        c.flags,
        c.angles[0],
        c.angles[1],
        c.angles[2],
        c.forward,
        c.right,
        c.up
    )
}

/// `text` on one line: backslash, newline, carriage return and other
/// control bytes escaped (`\\`, `\n`, `\r`, `\xNN`).
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c == '\x7f' => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cmd_line_carries_every_field() {
        let c = UserCmd {
            server_time: 1234,
            buttons: 1,
            wbuttons: 4,
            weapon: 12,
            flags: 2,
            angles: [65000, 16384, 0],
            forward: 127,
            right: -127,
            up: 0,
        };
        assert_eq!(
            cmd_fields(3, &c),
            "slot=3 st=1234 b=1 wb=4 w=12 fl=2 a=65000,16384,0 mv=127,-127,0"
        );
    }

    #[test]
    fn command_text_stays_on_one_line() {
        assert_eq!(escape("say a\\b\nc\x14d"), "say a\\\\b\\nc\\x14d");
        assert_eq!(escape("fdc 1"), "fdc 1");
    }
}
