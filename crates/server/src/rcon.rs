//! `SVC_RemoteCommand` (cod_lnxded 0x808c404): the OOB `rcon <password>
//! <command>`, and the `Com_BeginRedirect` buffer its output goes back
//! through as `print\n` packets. Numbers and captures are in
//! docs/research/cod11-server-handshake.md, "rcon".

use std::time::{Duration, Instant};

/// One rcon of any verdict per this window, server-wide (`cmp eax,0x1f3`
/// at 0x808c423). A dropped request does not restart it.
pub const INTERVAL: Duration = Duration::from_millis(500);
/// `SV_OUTPUTBUF_LENGTH` (`push 0x3ff0` at 0x808c526).
pub const OUTPUT_BUF: usize = 0x3ff0;
/// The rebuilt command line's buffer (`0x400` at 0x808c5c7).
const MAX_LINE: usize = 0x400;

pub const NO_PASSWORD: &str = "No rconpassword set on the server.\n";
pub const BAD_PASSWORD: &str = "Bad rconpassword.\n";

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Inside [`INTERVAL`] of the last answered request: no reply at all.
    Limited,
    NoPassword,
    BadPassword,
    /// The command line to execute; `None` when it overflowed the buffer,
    /// which retail answers with an empty `print`.
    Run(Option<String>),
}

/// The global `lasttime`.
#[derive(Default)]
pub struct Rcon {
    last: Option<Instant>,
}

impl Rcon {
    /// `args` is the packet tokenized whole: `rcon`, the password, then the
    /// command's own tokens.
    pub fn check(&mut self, password: &str, args: &[String], now: Instant) -> Verdict {
        if self
            .last
            .is_some_and(|t| now.saturating_duration_since(t) < INTERVAL)
        {
            return Verdict::Limited;
        }
        self.last = Some(now);
        let given = args.get(1).map_or("", String::as_str);
        if password.is_empty() {
            Verdict::NoPassword
        } else if given != password {
            Verdict::BadPassword
        } else {
            Verdict::Run(command_line(args.get(2..).unwrap_or(&[])))
        }
    }
}

/// The command line `SVC_RemoteCommand` rebuilds from argv 2 on: each token
/// quoted when it is empty or holds a byte at or below a space, then one
/// space (0x806dbd4). `None` past the 1 KB buffer, which retail does not run.
pub fn command_line(args: &[String]) -> Option<String> {
    let mut line = String::new();
    for a in args {
        if a.is_empty() || a.bytes().any(|b| b <= b' ') {
            line.push('"');
            line.push_str(a);
            line.push('"');
        } else {
            line.push_str(a);
        }
        line.push(' ');
    }
    (line.len() < MAX_LINE).then_some(line)
}

/// `Com_BeginRedirect`'s buffer: a print that would overflow it flushes what
/// is there as one packet first, and whatever is left goes when the
/// redirect ends, even if that is nothing.
#[derive(Default)]
pub struct Redirect {
    buf: String,
    full: Vec<String>,
}

impl Redirect {
    pub fn print(&mut self, msg: &str) {
        if self.buf.len() + msg.len() > OUTPUT_BUF - 1 {
            self.full.push(std::mem::take(&mut self.buf));
        }
        // `Q_strcat`: what does not fit in an empty buffer is cut.
        let room = OUTPUT_BUF - 1 - self.buf.len();
        let mut end = msg.len().min(room);
        while !msg.is_char_boundary(end) {
            end -= 1;
        }
        self.buf.push_str(&msg[..end]);
    }

    /// The `print\n<text>` payloads, in order.
    pub fn finish(mut self) -> Vec<String> {
        self.full.push(self.buf);
        self.full
            .into_iter()
            .map(|t| format!("print\n{t}"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn one_request_per_half_second_whatever_the_verdict() {
        let t = Instant::now();
        let mut r = Rcon::default();
        let a = args(&["rcon", "wrong", "status"]);
        assert_eq!(r.check("secret", &a, t), Verdict::BadPassword);
        assert_eq!(
            r.check("secret", &a, t + Duration::from_millis(499)),
            Verdict::Limited
        );
        // The dropped one did not move the window.
        let good = args(&["rcon", "secret", "status"]);
        assert_eq!(
            r.check("secret", &good, t + INTERVAL),
            Verdict::Run(Some("status ".into()))
        );
    }

    #[test]
    fn an_empty_password_refuses_everything() {
        let mut r = Rcon::default();
        assert_eq!(
            r.check("", &args(&["rcon", "", "status"]), Instant::now()),
            Verdict::NoPassword
        );
    }

    #[test]
    fn the_command_line_requotes_tokens_with_spaces() {
        assert_eq!(
            command_line(&args(&["say", "hi there", "", "x"])),
            Some("say \"hi there\" \"\" x ".into())
        );
        assert_eq!(command_line(&[]), Some(String::new()));
        assert_eq!(command_line(&args(&[&"a".repeat(1023)])), None);
    }

    #[test]
    fn output_flushes_a_packet_before_it_would_overflow() {
        let mut r = Redirect::default();
        let line = format!("{}\n", "x".repeat(99));
        for _ in 0..200 {
            r.print(&line);
        }
        let out = r.finish();
        // 163 lines of 100 fit under 0x3fef; the 164th starts the next.
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), "print\n".len() + 16300);
        assert_eq!(out[1].len(), "print\n".len() + 3700);
        assert_eq!(Redirect::default().finish(), ["print\n"]);
    }
}
