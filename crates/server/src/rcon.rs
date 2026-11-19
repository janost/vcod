//! `SVC_RemoteCommand` (cod_lnxded 0x808c404): the OOB `rcon <password>
//! <command>`, and the `Com_BeginRedirect` buffer its output goes back
//! through as `print\n` packets. Numbers and captures are in
//! docs/research/cod11-server-handshake.md, "rcon".

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// One rcon of any verdict per this window (`cmp eax,0x1f3` at 0x808c423).
/// Retail keeps one window for the whole server; vcod keeps one per source
/// address. A dropped request does not restart it.
pub const INTERVAL: Duration = Duration::from_millis(500);
/// vcod's: wrong passwords an address may send before it is dropped.
const FAIL_BURST: f32 = 8.0;
/// vcod's: how fast an address's wrong-password count drains.
const FAIL_DRAIN: Duration = Duration::from_secs(30);
/// vcod's: addresses tracked at once. Past it, a request whose password is
/// wrong from an address not tracked is dropped without a reply.
const MAX_TRACKED: usize = 4096;
/// `SV_OUTPUTBUF_LENGTH` (`push 0x3ff0` at 0x808c526).
pub const OUTPUT_BUF: usize = 0x3ff0;
/// The rebuilt command line's buffer (`0x400` at 0x808c5c7).
const MAX_LINE: usize = 0x400;

pub const NO_PASSWORD: &str = "No rconpassword set on the server.\n";
pub const BAD_PASSWORD: &str = "Bad rconpassword.\n";

/// CoD2 1.3's and CoD4 1.7's refusals (`cod2_lnxded` 0x8097188,
/// `cod4_lnxded` 0x8232838, 0x8232878, 0x82327ee): the server's password is
/// `rcon_password`, and the client logs in (cod2-protocol.md 1.6,
/// cod4-handshake.md 14.2).
pub const LOGIN_NO_PASSWORD: &str =
    "The server must set 'rcon_password' for clients to use 'rcon'.\n";
pub const LOGIN_NOT_LOGGED_IN: &str =
    "You must log in with 'rcon login <password>' before using 'rcon'.\n";
pub const LOGIN_INVALID: &str = "Invalid password.\n";

/// CoD2's and CoD4's cvar for a `--set` name: vcod's `rconPassword`
/// (CoD1's, the name docs/usage.md gives every game) is `rcon_password`
/// there; retail would keep it as a dvar nothing reads.
pub fn login_cvar_name(name: &str) -> &str {
    if name.eq_ignore_ascii_case("rconPassword") {
        "rcon_password"
    } else {
        name
    }
}

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

/// `lasttime`, per source address, and each address's wrong passwords.
#[derive(Default)]
pub struct Rcon {
    last: HashMap<IpAddr, Instant>,
    /// A leaky bucket: one per wrong password, draining one per
    /// [`FAIL_DRAIN`]; at [`FAIL_BURST`] the address gets no answer.
    fails: HashMap<IpAddr, (f32, Instant)>,
}

impl Verdict {
    /// The CoD2 and CoD4 refusal a verdict prints, `None` for none: an
    /// empty password given is a client that never ran `rcon login`.
    pub fn login_refusal(&self, args: &[String]) -> Option<&'static str> {
        match self {
            Verdict::NoPassword => Some(LOGIN_NO_PASSWORD),
            Verdict::BadPassword if args.get(1).is_none_or(|p| p.is_empty()) => {
                Some(LOGIN_NOT_LOGGED_IN)
            }
            Verdict::BadPassword => Some(LOGIN_INVALID),
            Verdict::Limited | Verdict::Run(_) => None,
        }
    }
}

impl Rcon {
    /// `args` is the packet tokenized whole: `rcon`, the password, then the
    /// command's own tokens. `from` is the sender's address.
    pub fn check(
        &mut self,
        password: &str,
        args: &[String],
        from: IpAddr,
        now: Instant,
    ) -> Verdict {
        if self
            .last
            .get(&from)
            .is_some_and(|&t| now.saturating_duration_since(t) < INTERVAL)
            || self.fail_level(from, now) >= FAIL_BURST
        {
            return Verdict::Limited;
        }
        let given = args.get(1).map_or("", String::as_str);
        let verdict = if password.is_empty() {
            Verdict::NoPassword
        } else if given != password {
            Verdict::BadPassword
        } else {
            Verdict::Run(command_line(args.get(2..).unwrap_or(&[])))
        };
        if self.last.len() >= MAX_TRACKED || self.fails.len() >= MAX_TRACKED {
            self.prune(now);
        }
        let tracked = self.last.contains_key(&from) || self.fails.contains_key(&from);
        let full = self.last.len() >= MAX_TRACKED || self.fails.len() >= MAX_TRACKED;
        if full && !tracked && !matches!(verdict, Verdict::Run(_)) {
            return Verdict::Limited;
        }
        self.last.insert(from, now);
        if verdict == Verdict::BadPassword {
            let level = self.fail_level(from, now);
            self.fails.insert(from, (level + 1.0, now));
        }
        verdict
    }

    /// `from`'s wrong passwords, drained to `now`.
    fn fail_level(&self, from: IpAddr, now: Instant) -> f32 {
        self.fails.get(&from).map_or(0.0, |&(n, at)| {
            let drained =
                now.saturating_duration_since(at).as_secs_f32() / FAIL_DRAIN.as_secs_f32();
            (n - drained).max(0.0)
        })
    }

    /// Forgets the addresses whose window has passed and whose bucket is
    /// empty.
    fn prune(&mut self, now: Instant) {
        self.last
            .retain(|_, t| now.saturating_duration_since(*t) < INTERVAL);
        let drained = |&(n, at): &(f32, Instant)| {
            n - now.saturating_duration_since(at).as_secs_f32() / FAIL_DRAIN.as_secs_f32() > 0.0
        };
        self.fails.retain(|_, v| drained(v));
    }
}

/// The line every game logs before it answers: `Rcon from %s:\n%s` or
/// `Bad rcon from %s:\n%s` with the third token (1.1d 0x80d5ae3 and
/// 0x80d5acd, CoD4 1.7 cod4-handshake.md 14.2); none for a dropped request.
pub fn log_line(verdict: &Verdict, from: std::net::SocketAddr, args: &[String]) -> Option<String> {
    let shown = args.get(2).map_or("", String::as_str);
    match verdict {
        Verdict::Limited => None,
        Verdict::Run(_) => Some(format!("Rcon from {from}:\n{shown}")),
        Verdict::NoPassword | Verdict::BadPassword => {
            Some(format!("Bad rcon from {from}:\n{shown}"))
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
        assert_eq!(r.check("secret", &a, A, t), Verdict::BadPassword);
        assert_eq!(
            r.check("secret", &a, A, t + Duration::from_millis(499)),
            Verdict::Limited
        );
        // The dropped one did not move the window.
        let good = args(&["rcon", "secret", "status"]);
        assert_eq!(
            r.check("secret", &good, A, t + INTERVAL),
            Verdict::Run(Some("status ".into()))
        );
    }

    const A: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(198, 51, 100, 2));
    const B: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(203, 0, 113, 9));

    #[test]
    fn a_flood_of_bad_passwords_from_one_address_leaves_another_s_window() {
        let t = Instant::now();
        let mut r = Rcon::default();
        let bad = args(&["rcon", "guess", "status"]);
        let good = args(&["rcon", "secret", "status"]);
        for ms in (0..2000).step_by(5) {
            let at = t + Duration::from_millis(ms);
            r.check("secret", &bad, A, at);
            if ms % 600 == 0 {
                assert_eq!(
                    r.check("secret", &good, B, at),
                    Verdict::Run(Some("status ".into())),
                    "{ms} ms"
                );
            }
        }
    }

    #[test]
    fn a_burst_of_wrong_passwords_drops_the_address_until_it_drains() {
        let t = Instant::now();
        let mut r = Rcon::default();
        let bad = args(&["rcon", "guess", "status"]);
        let good = args(&["rcon", "secret", "status"]);
        let mut at = t;
        // The bucket drains a little between the tries, so the last fills it.
        for _ in 0..=FAIL_BURST as u32 {
            assert_eq!(r.check("secret", &bad, A, at), Verdict::BadPassword);
            at += INTERVAL;
        }
        assert_eq!(r.check("secret", &good, A, at), Verdict::Limited);
        assert_eq!(r.check("secret", &bad, A, at + INTERVAL), Verdict::Limited);
        let later = at + FAIL_DRAIN * 2;
        assert_eq!(
            r.check("secret", &good, A, later),
            Verdict::Run(Some("status ".into()))
        );
    }

    #[test]
    fn spoofed_sources_stay_bounded_and_the_password_still_gets_in() {
        let t = Instant::now();
        let mut r = Rcon::default();
        let bad = args(&["rcon", "guess", "status"]);
        for i in 0..20_000u32 {
            let ip = IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + i));
            r.check("secret", &bad, ip, t);
        }
        assert!(r.last.len() <= MAX_TRACKED && r.fails.len() <= MAX_TRACKED);
        assert_eq!(
            r.check("secret", &args(&["rcon", "secret", "status"]), B, t),
            Verdict::Run(Some("status ".into()))
        );
    }

    #[test]
    fn every_answered_request_logs_its_verdict_and_third_token() {
        let from: std::net::SocketAddr = "198.51.100.2:28960".parse().unwrap();
        let a = args(&["rcon", "pw", "status"]);
        assert_eq!(
            log_line(&Verdict::Run(None), from, &a).as_deref(),
            Some("Rcon from 198.51.100.2:28960:\nstatus")
        );
        for v in [Verdict::BadPassword, Verdict::NoPassword] {
            assert_eq!(
                log_line(&v, from, &a).as_deref(),
                Some("Bad rcon from 198.51.100.2:28960:\nstatus")
            );
        }
        assert_eq!(log_line(&Verdict::Limited, from, &a), None);
    }

    #[test]
    fn an_empty_password_refuses_everything() {
        let mut r = Rcon::default();
        assert_eq!(
            r.check("", &args(&["rcon", "", "status"]), A, Instant::now()),
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
