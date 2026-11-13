//! CoD1's tty console: the line editor `Sys_ConsoleInput` runs when stdin
//! is a terminal and `ttycon` is on (Q3 1.32's, in 1.1d, 1.5b and UO
//! 1.51b alike). Raw mode, its own echo, a 32-line history on the arrow
//! keys and tab completion over command and cvar names; each log record
//! takes the line down and puts it back (`Sys_Print`'s `tty_Hide` and
//! `tty_Show`). CoD2 1.3 and CoD4 1.7 force `ttycon` off. Addresses and
//! the quirks kept: cod11-server-handshake.md, "The stdin console".

use std::collections::VecDeque;

/// `tty_con`'s 256-byte buffer less its NUL. Retail writes past it.
const LINE_MAX: usize = 255;
/// `TTY_HISTORY`: `Hist_Add` stops counting at 32.
const HISTORY: usize = 32;

/// The line being typed and the lines entered before it, newest first.
#[derive(Default)]
pub struct Editor {
    line: Vec<u8>,
    history: VecDeque<Vec<u8>>,
    /// `hist_current`: the history line on show, `None` for -1.
    current: Option<usize>,
    seq: Seq,
}

/// Where an escape sequence stands: retail reads a control byte's next
/// two bytes at once, and a byte not there yet drops the sequence.
#[derive(Default, Clone, Copy, PartialEq, Debug)]
enum Seq {
    #[default]
    Idle,
    /// A control byte read; `[` or `O` must follow.
    Ctl,
    /// `ESC [` or `ESC O` read; the final byte must follow.
    Csi,
}

/// What a key asks of the reader.
#[derive(Debug, PartialEq)]
pub enum Step {
    None,
    /// Enter: the line to run.
    Line(String),
    /// Tab: complete against the names, then [`Editor::complete`].
    Complete,
    /// `tty_FlushIn`: whatever else is waiting on stdin is thrown away.
    Flush,
}

impl Editor {
    /// One byte off the terminal; `out` takes what goes to stdout.
    pub fn key(&mut self, b: u8, out: &mut Vec<u8>) -> Step {
        match std::mem::take(&mut self.seq) {
            Seq::Ctl if matches!(b, b'[' | b'O') => {
                self.seq = Seq::Csi;
                Step::None
            }
            // `droping ISCTL sequence` (a developer print), then the flush.
            Seq::Ctl => Step::Flush,
            Seq::Csi => match b {
                b'A' => {
                    self.history_prev(out);
                    Step::Flush
                }
                b'B' => {
                    self.history_next(out);
                    Step::Flush
                }
                b'C' | b'D' => Step::None,
                _ => Step::Flush,
            },
            Seq::Idle => self.plain_key(b, out),
        }
    }

    /// The read came back with no more bytes: a sequence still waiting
    /// for one is dropped, as retail's non-blocking read returning -1
    /// drops it.
    pub fn end_of_read(&mut self) {
        self.seq = Seq::Idle;
    }

    fn plain_key(&mut self, b: u8, out: &mut Vec<u8>) -> Step {
        match b {
            // `tty_erase` is read before `tcgetattr` fills the struct, so it
            // is 0; DEL and BS are compared as well.
            0 | 0x08 | 0x7f => {
                if self.line.pop().is_some() {
                    out.extend_from_slice(b"\x08 \x08");
                }
                Step::None
            }
            b'\n' => {
                let line = std::mem::take(&mut self.line);
                self.history.push_front(line.clone());
                self.history.truncate(HISTORY);
                self.current = None;
                out.push(b'\n');
                Step::Line(String::from_utf8_lossy(&line).into_owned())
            }
            b'\t' => Step::Complete,
            // A signed `char` below a space: every byte past 127 too.
            _ if (b as i8) < 0x20 => {
                self.seq = Seq::Ctl;
                Step::None
            }
            _ => {
                if self.line.len() < LINE_MAX {
                    self.line.push(b);
                    out.push(b);
                }
                Step::None
            }
        }
    }

    /// Up: the next older line, if there is one (`Hist_Prev`).
    fn history_prev(&mut self, out: &mut Vec<u8>) {
        let next = self.current.map_or(0, |c| c + 1);
        if next < self.history.len() {
            self.current = Some(next);
            self.erase(out);
            self.line = self.history[next].clone();
            self.draw(out);
        }
    }

    /// Down: the next newer line, or an empty one past the newest
    /// (`Hist_Next`), even when no older line was on show.
    fn history_next(&mut self, out: &mut Vec<u8>) {
        self.current = self.current.and_then(|c| c.checked_sub(1));
        self.erase(out);
        self.line = self
            .current
            .map(|c| self.history[c].clone())
            .unwrap_or_default();
        self.draw(out);
    }

    /// `tty_Hide`: a backspace, space, backspace per character.
    pub fn erase(&self, out: &mut Vec<u8>) {
        for _ in &self.line {
            out.extend_from_slice(b"\x08 \x08");
        }
    }

    /// `tty_Show`: the line written back.
    pub fn draw(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.line);
    }

    /// `Field_CompleteCommand` (cod_lnxded 0x806d43c) on the line, with
    /// the tty's cleanup after it: the first token against `names`
    /// (commands, then cvars, as `Cmd_CommandCompletion` and
    /// `Cvar_CommandCompletion` walk them). One match is completed with
    /// a space; several cut the line to the longest prefix they share and
    /// come back as the lines to print (`]` and the line, then each match
    /// indented four).
    pub fn complete(&mut self, names: &[String]) -> Vec<String> {
        let text = String::from_utf8_lossy(&self.line).into_owned();
        let argv = crate::game::say::tokenize(&text);
        let word = argv.first().map_or("", String::as_str);
        let word = word
            .strip_prefix('\\')
            .or(word.strip_prefix('/'))
            .unwrap_or(word);
        if word.is_empty() {
            return Vec::new();
        }
        let matches: Vec<&str> = names
            .iter()
            .map(String::as_str)
            .filter(|n| starts_with_ignore_case(n, word))
            .collect();
        let Some(first) = matches.first() else {
            return Vec::new();
        };
        let mut shared = matches[1..].iter().fold(first.len(), |len, m| {
            first
                .bytes()
                .zip(m.bytes())
                .take(len)
                .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
                .count()
        });
        // A `set` can name a cvar in UTF-8; never cut inside a character.
        while !first.is_char_boundary(shared) {
            shared -= 1;
        }
        let mut line = format!("\\{}", &first[..shared]);
        let mut printed = Vec::new();
        if matches.len() == 1 {
            if argv.len() == 1 {
                line.push(' ');
            } else {
                concat_remaining(&mut line, &text, word, &argv);
            }
        } else {
            concat_remaining(&mut line, &text, word, &argv);
            printed.push(format!("]{line}"));
            printed.extend(matches.iter().map(|m| format!("    {m}")));
        }
        let line = line.strip_prefix('\\').unwrap_or(&line);
        self.line = line.as_bytes()[..line.len().min(LINE_MAX)].to_vec();
        printed
    }

    #[cfg(test)]
    fn line(&self) -> &str {
        std::str::from_utf8(&self.line).unwrap()
    }
}

fn starts_with_ignore_case(name: &str, prefix: &str) -> bool {
    name.len() >= prefix.len()
        && name.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// `ConcatRemaining` (0x806d2f8): what followed the completed word in
/// the old line, or, when the word is not found in it, each later
/// argument after a space, quoted when it holds one.
fn concat_remaining(line: &mut String, text: &str, word: &str, argv: &[String]) {
    if let Some(at) = text.find(word) {
        line.push_str(&text[at + word.len()..]);
        return;
    }
    for arg in &argv[1..] {
        line.push(' ');
        if arg.contains(' ') {
            line.push_str(&format!("\"{arg}\""));
        } else {
            line.push_str(arg);
        }
    }
}

#[cfg(unix)]
pub use term::{LogStderr, is_tty, restore, start};

/// The terminal itself: raw mode and its restore, the reader thread and
/// the log writer that works around the line.
#[cfg(unix)]
mod term {
    use super::{Editor, Step};
    use crate::stdin_console::Input;
    use std::io::Write;
    use std::sync::mpsc::{Sender, sync_channel};
    use std::sync::{Mutex, MutexGuard, Once};
    use std::time::Duration;

    /// The editor while the tty console runs, `None` otherwise. The log
    /// writer takes the same lock, so a record and a key never interleave.
    static LINE: Mutex<Option<Editor>> = Mutex::new(None);
    /// The terminal's settings before raw mode (`tty_tc`).
    static SAVED: Mutex<Option<libc::termios>> = Mutex::new(None);

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn is_tty() -> bool {
        // SAFETY: `isatty` reads no memory of ours.
        unsafe { libc::isatty(0) == 1 }
    }

    /// `Sys_ConsoleInputInit`'s tty branch (cod_lnxded 0x80c57ec): raw mode
    /// on fd 0 and the reader thread, which sends each entered line and
    /// asks for the names a tab completes against through `tx`. The old
    /// settings come back at exit, from `atexit`, or on [`restore`].
    pub fn start(tx: Sender<Input>) -> std::io::Result<()> {
        // SAFETY: `termios` is plain data; `tcgetattr` fills it.
        let mut tc: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: `tc` is a valid termios to write.
        if unsafe { libc::tcgetattr(0, &mut tc) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut raw = tc;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO);
        raw.c_iflag &= !(libc::INPCK | libc::ISTRIP);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        *lock(&SAVED) = Some(tc);
        static AT_EXIT: Once = Once::new();
        // SAFETY: registers a plain `extern "C"` function.
        AT_EXIT.call_once(|| unsafe {
            libc::atexit(restore_at_exit);
        });
        // SAFETY: `raw` is a valid termios.
        if unsafe { libc::tcsetattr(0, libc::TCSADRAIN, &raw) } != 0 {
            lock(&SAVED).take();
            return Err(std::io::Error::last_os_error());
        }
        *lock(&LINE) = Some(Editor::default());
        std::thread::spawn(move || read_keys(&tx));
        Ok(())
    }

    /// `Sys_ConsoleInputShutdown`: the terminal's own settings back. Safe
    /// to call more than once, and from a panic hook.
    pub fn restore() {
        let Some(tc) = SAVED.try_lock().ok().and_then(|mut s| s.take()) else {
            return;
        };
        if let Ok(mut line) = LINE.try_lock() {
            *line = None;
        }
        let msg = b"Shutdown tty console\n";
        // SAFETY: writes a static buffer; `tc` is the termios read at start.
        unsafe {
            libc::write(2, msg.as_ptr().cast(), msg.len());
            libc::tcsetattr(0, libc::TCSADRAIN, &tc);
        }
    }

    extern "C" fn restore_at_exit() {
        restore();
    }

    /// Runs `f` on the editor under its lock and writes what it put in
    /// `out` to stdout before letting go.
    fn with_editor<R>(f: impl FnOnce(&mut Editor, &mut Vec<u8>) -> R) -> Option<R> {
        let mut guard = lock(&LINE);
        let ed = guard.as_mut()?;
        let mut out = Vec::new();
        let r = f(ed, &mut out);
        write_stdout(&out);
        Some(r)
    }

    fn write_stdout(bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut o = std::io::stdout().lock();
        let _ = o.write_all(bytes);
        let _ = o.flush();
    }

    /// Reads what the terminal has, a key at a time through the editor,
    /// until hangup. Never logs while holding the editor: the log writer
    /// takes its lock.
    fn read_keys(tx: &Sender<Input>) {
        let mut buf = [0u8; 64];
        loop {
            // SAFETY: reads into `buf`, at most its length.
            let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
            if n == 0 {
                return;
            }
            if n < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() != std::io::ErrorKind::Interrupted {
                    log::debug!("stdin: {e}");
                    std::thread::sleep(Duration::from_secs(1));
                }
                continue;
            }
            for &b in &buf[..n as usize] {
                let Some(step) = with_editor(|ed, out| ed.key(b, out)) else {
                    return;
                };
                match step {
                    Step::None => {}
                    Step::Line(line) => {
                        if !line.trim().is_empty() && tx.send(Input::Line(line)).is_err() {
                            return;
                        }
                    }
                    Step::Complete => {
                        let (reply, names) = sync_channel(1);
                        if tx.send(Input::Names(reply)).is_err() {
                            return;
                        }
                        let names = names.recv().unwrap_or_default();
                        with_editor(|ed, out| {
                            ed.erase(out);
                            write_stdout(&std::mem::take(out));
                            let mut err = std::io::stderr().lock();
                            for l in ed.complete(&names) {
                                let _ = writeln!(err, "{l}");
                            }
                            ed.draw(out);
                        });
                    }
                    Step::Flush => {
                        // SAFETY: discards fd 0's unread input.
                        unsafe { libc::tcflush(0, libc::TCIFLUSH) };
                        break;
                    }
                }
            }
            with_editor(|ed, _| ed.end_of_read());
        }
    }

    /// stderr for the log while the tty console runs: the line comes down
    /// before each record and goes back after it, as `Sys_Print`
    /// (cod_lnxded 0x80c6478) wraps its `fputs` in `tty_Hide` and
    /// `tty_Show`.
    pub struct LogStderr;

    impl Write for LogStderr {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let guard = lock(&LINE);
            let mut out = Vec::new();
            if let Some(ed) = guard.as_ref() {
                ed.erase(&mut out);
                write_stdout(&out);
            }
            let r = std::io::stderr().write_all(buf);
            if let Some(ed) = guard.as_ref() {
                out.clear();
                ed.draw(&mut out);
                write_stdout(&out);
            }
            r.map(|()| buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            std::io::stderr().flush()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `keys` as one read; the lines entered and stdout's bytes.
    fn feed(ed: &mut Editor, keys: &[u8]) -> (Vec<String>, Vec<u8>) {
        let mut out = Vec::new();
        let mut lines = Vec::new();
        for &b in keys {
            match ed.key(b, &mut out) {
                Step::Line(l) => lines.push(l),
                Step::Flush => break,
                Step::None | Step::Complete => {}
            }
        }
        ed.end_of_read();
        (lines, out)
    }

    #[test]
    fn typed_keys_echo_and_enter_ends_the_line() {
        let mut ed = Editor::default();
        let (lines, out) = feed(&mut ed, b"statu\x7fus\n");
        assert_eq!(lines, ["status"]);
        assert_eq!(out, b"statu\x08 \x08us\n");
        assert_eq!(ed.line(), "");
    }

    /// DEL, BS and NUL (the zero `tty_erase`) all erase; an empty line
    /// takes no backspace.
    #[test]
    fn erase_keys() {
        let mut ed = Editor::default();
        let (_, out) = feed(&mut ed, b"\x08abc\x00\x08\x7f\x7f");
        assert_eq!(out, b"abc\x08 \x08\x08 \x08\x08 \x08");
        assert_eq!(ed.line(), "");
    }

    #[test]
    fn up_and_down_walk_the_history() {
        let mut ed = Editor::default();
        feed(&mut ed, b"one\ntwo\n");
        feed(&mut ed, b"x");
        let (_, out) = feed(&mut ed, b"\x1b[A");
        assert_eq!(ed.line(), "two");
        assert_eq!(out, b"\x08 \x08two");
        feed(&mut ed, b"\x1b[A");
        assert_eq!(ed.line(), "one");
        // Past the oldest line Up does nothing.
        let (_, out) = feed(&mut ed, b"\x1bOA");
        assert_eq!((ed.line(), out.as_slice()), ("one", &b""[..]));
        feed(&mut ed, b"\x1b[B");
        assert_eq!(ed.line(), "two");
        // Past the newest line Down empties the line; the typed `x` is lost.
        feed(&mut ed, b"\x1b[B");
        assert_eq!(ed.line(), "");
    }

    /// Down with no history line on show still clears what was typed.
    #[test]
    fn down_on_a_fresh_line_clears_it() {
        let mut ed = Editor::default();
        feed(&mut ed, b"abc");
        let (_, out) = feed(&mut ed, b"\x1b[B");
        assert_eq!(ed.line(), "");
        assert_eq!(out, b"\x08 \x08\x08 \x08\x08 \x08");
    }

    /// Enter clears the walk; an empty line is kept in the history too.
    #[test]
    fn enter_adds_every_line_and_resets_the_walk() {
        let mut ed = Editor::default();
        feed(&mut ed, b"a\n\n");
        feed(&mut ed, b"\x1b[A");
        assert_eq!(ed.line(), "");
        feed(&mut ed, b"\x1b[A");
        assert_eq!(ed.line(), "a");
        feed(&mut ed, b"\n");
        feed(&mut ed, b"\x1b[A");
        assert_eq!(ed.line(), "a");
    }

    #[test]
    fn history_keeps_thirty_two_lines() {
        let mut ed = Editor::default();
        for i in 0..40 {
            feed(&mut ed, format!("l{i}\n").as_bytes());
        }
        for _ in 0..40 {
            feed(&mut ed, b"\x1b[A");
        }
        assert_eq!(ed.line(), "l8");
    }

    /// Up and Down flush what follows them in the read; Left and Right do
    /// nothing and keep it.
    #[test]
    fn arrows_flush_or_pass() {
        let mut ed = Editor::default();
        feed(&mut ed, b"a\n");
        feed(&mut ed, b"\x1b[Axyz");
        assert_eq!(ed.line(), "a");
        feed(&mut ed, b"\x1b[Db\x1b[Cc");
        assert_eq!(ed.line(), "abc");
    }

    /// Any other control byte, or a byte past 127, takes the next one
    /// with it and drops the rest of the read; cut at the read's end it
    /// takes nothing more.
    #[test]
    fn other_control_bytes_drop_the_read() {
        let mut ed = Editor::default();
        feed(&mut ed, b"a\x01bc");
        assert_eq!(ed.line(), "a");
        feed(&mut ed, b"\x1b[3~d");
        assert_eq!(ed.line(), "a");
        feed(&mut ed, "\u{e9}e".as_bytes());
        assert_eq!(ed.line(), "a");
        feed(&mut ed, b"\x15");
        feed(&mut ed, b"x");
        assert_eq!(ed.line(), "ax");
    }

    #[test]
    fn the_line_stops_at_255() {
        let mut ed = Editor::default();
        let (_, out) = feed(&mut ed, &[b'a'; 300]);
        assert_eq!(out.len(), 255);
        assert_eq!(ed.line().len(), 255);
    }

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    fn tab(line: &str, n: &[&str]) -> (String, Vec<String>) {
        let mut ed = Editor::default();
        feed(&mut ed, line.as_bytes());
        let mut out = Vec::new();
        assert_eq!(ed.key(b'\t', &mut out), Step::Complete);
        let printed = ed.complete(&names(n));
        (ed.line().to_string(), printed)
    }

    #[test]
    fn one_match_completes_with_a_space() {
        let n = ["map", "map_restart", "status", "sv_hostname"];
        assert_eq!(tab("sta", &n), ("status ".into(), vec![]));
        assert_eq!(tab("STA", &n), ("status ".into(), vec![]));
        assert_eq!(tab("/sta", &n), ("status ".into(), vec![]));
        assert_eq!(tab("sv_h", &n), ("sv_hostname ".into(), vec![]));
        // The arguments after the word stay.
        assert_eq!(tab("sv_h  x y", &n), ("sv_hostname  x y".into(), vec![]));
    }

    #[test]
    fn several_matches_cut_to_the_shared_prefix_and_print() {
        let n = ["map", "map_restart", "map_rotate", "mapname", "status"];
        let (line, printed) = tab("ma", &n);
        assert_eq!(line, "map");
        assert_eq!(
            printed,
            [
                "]\\map",
                "    map",
                "    map_restart",
                "    map_rotate",
                "    mapname"
            ]
        );
        let (line, printed) = tab("map_r", &n);
        assert_eq!(line, "map_r");
        assert_eq!(printed, ["]\\map_r", "    map_restart", "    map_rotate"]);
        // The first match's case is the one kept.
        let (line, _) = tab("SCR", &["scr_dm_timelimit", "scr_dm_scorelimit"]);
        assert_eq!(line, "scr_dm_");
        // Two names that part inside a UTF-8 character.
        let (line, _) = tab("x", &["x\u{e9}", "x\u{e8}"]);
        assert_eq!(line, "x");
    }

    #[test]
    fn nothing_to_complete_leaves_the_line() {
        let n = ["status"];
        assert_eq!(tab("zz", &n), ("zz".into(), vec![]));
        assert_eq!(tab("", &n), ("".into(), vec![]));
        assert_eq!(tab("  ", &n), ("  ".into(), vec![]));
    }
}
