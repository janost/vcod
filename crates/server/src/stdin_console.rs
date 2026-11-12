//! The dedicated server's stdin console, as `Sys_ConsoleInput` reads it on
//! all three retail Linux servers: each line typed or piped in goes into the
//! console queue (`Cbuf_AddText`) and runs at the next `Cbuf_Execute` with
//! the console's full privilege, through the dispatch rcon uses. EOF stops
//! the reading, never the server. Retail's reads and their addresses:
//! cod11-server-handshake.md, "The stdin console".

use crate::engine::{Engine, GameModule};
use std::io::BufRead;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

/// The lines a reader thread took off stdin, waiting for the frame loop.
pub struct StdinConsole {
    lines: Receiver<String>,
}

impl StdinConsole {
    /// Reads `input` on its own thread until EOF. Retail ignores `SIGTTIN`
    /// and `SIGTTOU` first, so a server started in the background with the
    /// terminal as its stdin is not stopped by its own read.
    pub fn spawn<R: BufRead + Send + 'static>(input: R) -> Self {
        ignore_tty_signals();
        let (tx, lines) = channel();
        std::thread::spawn(move || read_lines(input, &tx));
        StdinConsole { lines }
    }

    /// A console that has already read all of `input`, on this thread.
    #[cfg(test)]
    pub(crate) fn read_now(input: &[u8]) -> Self {
        let (tx, lines) = channel();
        read_lines(input, &tx);
        StdinConsole { lines }
    }

    /// Queues every line read since the last call, as `Com_EventLoop`'s
    /// `SE_CONSOLE` case does: on the frame loop's thread, ahead of the
    /// frame's `drain_console`.
    pub fn queue<G: GameModule>(&self, e: &mut Engine<G>) {
        for line in self.lines.try_iter() {
            e.push_console(&line);
        }
    }
}

/// Sends each non-blank line of `input` without its line ending, until EOF
/// or until the frame loop is gone. A read error (`EIO` for a background
/// process reading its terminal) waits a second and tries again, as retail
/// polls stdin every frame whatever the last read returned.
fn read_lines(mut input: impl BufRead, tx: &Sender<String>) {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match input.read_until(b'\n', &mut buf) {
            Ok(0) => return,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf);
                let line = line.trim_end_matches(['\n', '\r']);
                if !line.trim().is_empty() && tx.send(line.to_string()).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                log::debug!("stdin: {e}");
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

/// `signal(SIGTTIN, SIG_IGN); signal(SIGTTOU, SIG_IGN)`, the start of
/// `Sys_ConsoleInputInit` (cod_lnxded 0x80c578d, 0x80c5799).
fn ignore_tty_signals() {
    #[cfg(unix)]
    // SAFETY: installs `SIG_IGN`, no handler code runs.
    unsafe {
        libc::signal(libc::SIGTTIN, libc::SIG_IGN);
        libc::signal(libc::SIGTTOU, libc::SIG_IGN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cod2::{Cod2Config, Cod2Server};
    use crate::cod4::{Cod4Config, Cod4Server};
    use std::io::{Cursor, Write};
    use std::net::SocketAddr;
    use std::time::Instant;

    fn read_all(input: &[u8]) -> Vec<String> {
        let (tx, rx) = channel();
        read_lines(Cursor::new(input.to_vec()), &tx);
        drop(tx);
        rx.into_iter().collect()
    }

    /// One command a line, CR LF or LF, blank lines dropped, a last line
    /// without its newline kept.
    #[test]
    fn each_line_is_one_command() {
        assert_eq!(
            read_all(b"status\r\n\n  \nsay \"hi there\"\nmap_restart"),
            ["status", "say \"hi there\"", "map_restart"]
        );
    }

    /// A pipe, which is no terminal: what a service manager or `nohup`
    /// hands the server. Its lines arrive, and its EOF ends the reading
    /// while the server keeps going.
    #[test]
    fn a_pipe_s_lines_arrive_and_its_eof_stops_only_the_reading() {
        let (r, mut w) = std::io::pipe().unwrap();
        let console = StdinConsole::spawn(std::io::BufReader::new(r));
        w.write_all(b"set sv_hostname piped\n").unwrap();
        drop(w);
        let now = Instant::now();
        let mut sv = Cod2Server::new(cod2_cfg(), 7, now);
        sv.load(None).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while sv.core.console.is_empty() && Instant::now() < until {
            console.queue(&mut sv);
            std::thread::sleep(Duration::from_millis(5));
        }
        sv.drain_console(now);
        assert_eq!(sv.cvar("sv_hostname"), "piped");
        // The reader is gone; the queue answers empty from now on.
        std::thread::sleep(Duration::from_millis(20));
        console.queue(&mut sv);
        assert!(sv.core.console.is_empty());
        sv.tick_frames(now, 1);
        assert!(!sv.quit_requested());
    }

    fn getinfo<G: GameModule>(sv: &mut Engine<G>) -> String {
        let from = SocketAddr::from(([127, 0, 0, 1], 5));
        let mut pkt = vec![0xff; 4];
        pkt.extend_from_slice(b"getinfo 1");
        sv.handle_packet(from, &pkt, Instant::now());
        sv.take_outgoing()
            .into_iter()
            .map(|(_, p)| String::from_utf8_lossy(&p).into_owned())
            .collect()
    }

    fn cod2_cfg() -> Cod2Config {
        Cod2Config {
            map: "mp_toujane".into(),
            gametype: "dm".into(),
            hostname: "vcod cod2".into(),
            max_clients: 4,
            gametype_script: None,
        }
    }

    #[test]
    fn cod2_runs_a_stdin_line() {
        let now = Instant::now();
        let mut sv = Cod2Server::new(cod2_cfg(), 7, now);
        sv.load(None).unwrap();
        StdinConsole::read_now(b"set sv_hostname from_stdin\n").queue(&mut sv);
        sv.drain_console(now);
        assert_eq!(sv.cvar("sv_hostname"), "from_stdin");
        assert!(getinfo(&mut sv).contains("\\hostname\\from_stdin"));
    }

    #[test]
    fn cod4_runs_a_stdin_line() {
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod cod4".into(),
                max_clients: 4,
            },
            7,
        );
        StdinConsole::read_now(b"set sv_hostname from_stdin\n").queue(&mut sv);
        <crate::cod4::Cod4Game as GameModule>::drain_console(&mut sv, Instant::now());
        assert_eq!(sv.cvar("sv_hostname"), "from_stdin");
    }
}
