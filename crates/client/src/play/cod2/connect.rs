//! Joining a CoD2 server: the options, the handshake on its thread and
//! the CD key challenge (cod2-protocol.md 1).

use super::*;

/// What the session answers the stock script menus with
/// (`maps/mp/gametypes/_menus.gsc`'s `menuresponse` loop).
#[derive(Clone, Debug)]
pub struct Join {
    /// `allies`, `axis`, `autoassign` or `spectator`.
    pub team: String,
    /// A weapon file name the weapon menu allows; `None` takes the
    /// nation's bolt-action rifle.
    pub weapon: Option<String>,
}

impl Join {
    /// The response to script menu `menu` (its configstring name), or
    /// `None` for one a joining player leaves alone.
    pub fn response(&self, menu: &str) -> Option<String> {
        if menu.starts_with("serverinfo_") {
            return Some("close".into());
        }
        if menu.starts_with("team_") {
            return Some(self.team.clone());
        }
        let nation = menu.strip_prefix("weapon_")?;
        if let Some(w) = &self.weapon {
            return Some(w.clone());
        }
        Some(
            match nation {
                "german" => "kar98k_mp",
                "british" => "enfield_mp",
                "american" => "m1garand_mp",
                "russian" => "mosin_nagant_mp",
                _ => return None,
            }
            .into(),
        )
    }
}

/// A connect running on its own thread ([`Cod2Session::connect_in_background`]).
/// Dropped, it stops the connect: the handshake and downloads end at their
/// next step, and a session already connected says `disconnect`.
pub struct Connecting {
    pub addr: String,
    handoff: Arc<std::sync::Mutex<Handoff>>,
    job: std::thread::JoinHandle<()>,
    cancel: Arc<AtomicBool>,
    progress: Arc<std::sync::Mutex<Option<Progress>>>,
}

/// The connect thread's result as [`Connecting`] and its thread hand it
/// over: whichever side comes second sends `disconnect` for a session no
/// one will take.
enum Handoff {
    Running,
    Done(anyhow::Result<Box<Cod2Session>>),
    Abandoned,
}

/// What the client sends and fetches on a connect: its name, and where
/// and whether it downloads the server's iwds (`cl_allowDownload`,
/// `cl_wwwDownload`).
#[derive(Clone, Debug)]
pub struct ConnectOpts {
    pub name: String,
    /// `fs_homepath`, which on the Windows and Mac clients is the install
    /// (`Sys_DefaultHomePath` returns nothing, cod2-protocol.md 17.5):
    /// `main/` and the mod directories sit in it. `None` downloads nothing.
    pub home: Option<PathBuf>,
    pub allow_download: bool,
    pub www_download: bool,
    /// The `rate` and `snaps` userinfo dvars, and `password` (left out
    /// when empty).
    pub rate: String,
    pub snaps: String,
    pub password: String,
    /// `cl_voice` (default 1, flags 0x1003: archived, userinfo;
    /// `CL_InitOnceForAllClients`, Mac 0x1485e2): the server relays voice
    /// only to a client sending it on (cod2-voice.md 3).
    pub voice: bool,
}

impl ConnectOpts {
    /// Retail's defaults (both cvars 1) with `fs`'s install as the homepath.
    pub fn new(name: &str, fs: &Pk3Fs) -> Self {
        ConnectOpts {
            name: name.to_string(),
            home: home_of(fs),
            ..Self::default()
        }
    }

    /// The userinfo string: CoD2's stock keys (cod2-protocol.md 1.2), the
    /// values stripped of `\`, `;` and `"`, which an info string cannot
    /// carry. `connect` adds `protocol`, `challenge` and `qport`.
    pub fn userinfo(&self) -> String {
        let clean = |v: &str| -> String {
            v.chars()
                .filter(|c| !matches!(c, '\\' | ';' | '"'))
                .collect()
        };
        let mut ui = format!(
            "\\name\\{}\\rate\\{}\\snaps\\{}\\cl_punkbuster\\0\\cl_voice\\{}\\cl_wwwDownload\\{}",
            clean(&self.name),
            clean(&self.rate),
            clean(&self.snaps),
            u8::from(self.voice),
            u8::from(self.www_download)
        );
        if !self.password.is_empty() {
            ui.push_str(&format!("\\password\\{}", clean(&self.password)));
        }
        ui
    }
}

impl ConnectOpts {
    /// These options with the console's userinfo dvars; an empty `rate` or
    /// `snaps` keeps the default.
    pub fn with_userinfo(self, info: &vcod_common::net::Userinfo) -> Self {
        let or = |v: &str, d: String| if v.is_empty() { d } else { v.to_string() };
        let d = ConnectOpts::default();
        ConnectOpts {
            name: info.name.clone(),
            rate: or(&info.rate, d.rate),
            snaps: or(&info.snaps, d.snaps),
            password: info.password.clone(),
            www_download: info
                .www_download
                .trim()
                .parse::<f32>()
                .is_ok_and(|v| v != 0.0),
            ..self
        }
    }
}

impl Default for ConnectOpts {
    fn default() -> Self {
        ConnectOpts {
            name: String::new(),
            home: None,
            allow_download: true,
            www_download: true,
            rate: "25000".into(),
            snaps: "20".into(),
            password: String::new(),
            voice: true,
        }
    }
}

/// The install `fs`'s `main/` sits in.
pub fn home_of(fs: &Pk3Fs) -> Option<PathBuf> {
    fs.dirs()
        .iter()
        .find(|d| {
            d.file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("main"))
        })
        .and_then(|d| d.parent())
        .map(Path::to_path_buf)
}

impl Connecting {
    /// The file the connect is downloading, for the connect screen.
    pub fn progress(&self) -> Option<Progress> {
        self.progress.lock().ok()?.clone()
    }

    /// The session once the gamestate is in, or why the connect failed;
    /// `None` while it runs.
    pub fn poll(&self) -> Option<anyhow::Result<Cod2Session>> {
        let mut slot = self.handoff.lock().ok()?;
        match std::mem::replace(&mut *slot, Handoff::Running) {
            Handoff::Done(r) => Some(r.map(|s| *s)),
            _ if self.job.is_finished() => Some(Err(anyhow::anyhow!("the connect thread ended"))),
            _ => None,
        }
    }
}

impl Drop for Connecting {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Ok(mut slot) = self.handoff.lock() {
            if let Handoff::Done(Ok(s)) = &mut *slot {
                s.disconnect();
            }
            *slot = Handoff::Abandoned;
        }
    }
}

impl Cod2Session {
    /// Challenge, plain-text connect, then wait for the gamestate and its
    /// downloads; the `cp` waits for the first [`Self::fly`], over the
    /// search path the map loads from (`CL_DownloadsComplete` sends it
    /// after `CL_InitCGame`). Blocks for at most `timeout` until the
    /// gamestate.
    pub fn connect(addr: &str, opts: &ConnectOpts, timeout: Duration) -> anyhow::Result<Self> {
        Self::connect_with(addr, opts, timeout, None, None)
    }

    /// [`Self::connect`] on a thread of its own, for the front end's
    /// `connect`; [`Connecting::progress`] follows its downloads.
    pub fn connect_in_background(addr: &str, opts: ConnectOpts, timeout: Duration) -> Connecting {
        let handoff = Arc::new(std::sync::Mutex::new(Handoff::Running));
        let cancel = Arc::new(AtomicBool::new(false));
        let a = addr.to_string();
        let progress = Arc::new(std::sync::Mutex::new(None));
        let shown = progress.clone();
        let (slot, flag) = (handoff.clone(), cancel.clone());
        let job = std::thread::spawn(move || {
            let r = Self::connect_with(&a, &opts, timeout, Some(shown), Some(&flag));
            let Ok(mut slot) = slot.lock() else {
                return;
            };
            match (&*slot, r) {
                (Handoff::Abandoned, Ok(mut s)) => s.disconnect(),
                (Handoff::Abandoned, Err(_)) => {}
                (_, r) => *slot = Handoff::Done(r.map(Box::new)),
            }
        });
        Connecting {
            addr: addr.to_string(),
            handoff,
            job,
            cancel,
            progress,
        }
    }

    /// The handshake, then the gamestate and every download it asks for:
    /// the session comes back once the map can load (`CL_DownloadsComplete`).
    fn connect_with(
        addr: &str,
        opts: &ConnectOpts,
        timeout: Duration,
        progress: Option<Arc<std::sync::Mutex<Option<Progress>>>>,
        cancel: Option<&AtomicBool>,
    ) -> anyhow::Result<Self> {
        let cancelled = || cancel.is_some_and(|c| c.load(Ordering::Relaxed));
        let to = vcod_common::net::master::parse_address(addr)
            .with_context(|| format!("cannot resolve {addr}"))?;
        let sock = UdpSocket::bind("0.0.0.0:0")?;
        sock.connect(to)
            .with_context(|| format!("cannot reach {addr}"))?;
        sock.set_read_timeout(Some(Duration::from_millis(5)))?;
        let key = KeyAuth {
            key: cdkey::lookup(KeyGame::Cod2),
            server: sock.peer_addr().ok(),
        };
        let hash = key.key.as_ref().map(|k| k.hash(KeyGame::Cod2));
        let challenge = ask_keyed(
            &sock,
            &oob::getchallenge(hash.as_deref()),
            Some(&key),
            |r| match r {
                Reply::Challenge(c) => Some(*c),
                _ => None,
            },
        )?;
        // Below 0x8000: retail's `SV_VoicePacket` reads the qport signed and
        // never matches a higher one (cod2-voice.md 3).
        let qport = std::process::id() as u16 & 0x7fff;
        // `cl_wwwDownload` is a userinfo dvar (flags 0x1003,
        // `CL_InitOnceForAllClients`, Mac 0x1485e2).
        let ui = opts.userinfo();
        ask(&sock, &oob::connect(&ui, challenge, qport), |r| {
            matches!(r, Reply::Connect).then_some(())
        })?;
        log::info!("cod2: connected to {addr}, challenge {challenge}");
        let mut s = Cod2Session::new(Some(sock), Client::new(qport, challenge));
        s.opts = opts.clone();
        s.progress = progress;
        let until = Instant::now() + timeout;
        // Past the first gamestate only `cl_timeout` gives up: a download
        // takes as long as it takes.
        while !s.settled() {
            if cancelled() {
                s.disconnect();
                bail!("connect to {addr} cancelled");
            }
            if s.view.gamestates == 0 && Instant::now() > until {
                bail!("no gamestate from {addr}");
            }
            s.poll(None);
            if let Some(why) = s.dropped.take() {
                // `Com_Error`'s `CL_Disconnect`: the server hears of it.
                s.disconnect();
                return Err(ServerError(why).into());
            }
            s.send_due()?;
            std::thread::sleep(Duration::from_millis(2));
        }
        s.level_gamestates = s.view.gamestates;
        s.loaded_map = s.map_name().unwrap_or_default().to_string();
        Ok(s)
    }
}

/// The CD key a `getchallenge` goes out beside, and the server it asks.
struct KeyAuth {
    key: Option<CdKey>,
    server: Option<std::net::SocketAddr>,
}

impl KeyAuth {
    /// `CL_RequestAuthorization` (CoD2MP_s.exe 0x40d130), fire and forget.
    fn request(&self) {
        if let (Some(key), Some(server)) = (&self.key, self.server) {
            cdkey::request_authorization(KeyGame::Cod2, key, server);
        }
    }
}

/// Send `p` until an out-of-band reply `want` accepts comes back.
fn ask<T>(sock: &UdpSocket, p: &[u8], want: impl Fn(&Reply) -> Option<T>) -> anyhow::Result<T> {
    ask_keyed(sock, p, None, want)
}

/// [`ask`], each send after the key's authorization when `auth` is given
/// (`CL_CheckForResend`, CoD2MP_s.exe 0x40e316). A `needcdkey` without a
/// key ends it with `EXE_ERR_INVALID_CD_KEY`, where retail stops before it
/// sends; with one it sends the key again and keeps asking (0x40f07b),
/// and a timeout then reads `EXE_AWAITINGCDKEYAUTH`, the connect
/// screen's line.
fn ask_keyed<T>(
    sock: &UdpSocket,
    p: &[u8],
    auth: Option<&KeyAuth>,
    want: impl Fn(&Reply) -> Option<T>,
) -> anyhow::Result<T> {
    ask_keyed_paced(sock, p, auth, want, Duration::from_secs(1))
}

/// [`ask_keyed`] with its five sends `wait` apart.
fn ask_keyed_paced<T>(
    sock: &UdpSocket,
    p: &[u8],
    auth: Option<&KeyAuth>,
    want: impl Fn(&Reply) -> Option<T>,
    wait: Duration,
) -> anyhow::Result<T> {
    let mut buf = vec![0u8; 0x10000];
    let mut awaiting_key = false;
    for _ in 0..5 {
        if let Some(a) = auth {
            a.request();
        }
        sock.send(p)?;
        let until = Instant::now() + wait;
        while Instant::now() < until {
            let Ok(n) = sock.recv(&mut buf) else {
                continue;
            };
            let Some(pl) = oob::payload(&buf[..n]) else {
                continue;
            };
            let r = oob::parse_reply(pl);
            if let Reply::Error(e) = &r {
                return Err(ServerError(e.clone()).into());
            }
            if let (Reply::NeedCdKey, Some(a)) = (&r, auth) {
                if a.key.is_none() {
                    cdkey::warn_no_key();
                    return Err(ServerError("EXE_ERR_INVALID_CD_KEY".into()).into());
                }
                awaiting_key = true;
                a.request();
            }
            if let Some(t) = want(&r) {
                return Ok(t);
            }
        }
    }
    if awaiting_key {
        return Err(ServerError("EXE_AWAITINGCDKEYAUTH".into()).into());
    }
    bail!("no answer")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A loopback server answering each `getchallenge` with `reply`: what
    /// the challenge ask comes back with, with and without a key.
    fn challenge_with(reply: &'static str, key: Option<CdKey>) -> anyhow::Result<i32> {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            server
                .set_read_timeout(Some(Duration::from_secs(6)))
                .unwrap();
            while let Ok((n, from)) = server.recv_from(&mut buf) {
                if buf[..n].starts_with(b"\xff\xff\xff\xffgetchallenge") {
                    let _ = server.send_to(&oob::packet(reply), from);
                }
            }
        });
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.connect(addr).unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(5)))
            .unwrap();
        let auth = KeyAuth {
            server: sock.peer_addr().ok(),
            key,
        };
        let want = |r: &Reply| match r {
            Reply::Challenge(c) => Some(*c),
            _ => None,
        };
        ask_keyed_paced(
            &sock,
            &oob::getchallenge(None),
            Some(&auth),
            want,
            Duration::from_millis(10),
        )
    }

    #[test]
    fn a_key_leaves_a_loopback_challenge_unchanged() {
        let key = || CdKey::parse("AAAA-AAAA-AAAA-AAAA");
        assert_eq!(challenge_with("challengeResponse 9", None).unwrap(), 9);
        assert_eq!(challenge_with("challengeResponse 9", key()).unwrap(), 9);
        let why = |r: anyhow::Result<i32>| match r.unwrap_err().downcast::<ServerError>() {
            Ok(ServerError(w)) => w,
            Err(e) => format!("{e:#}"),
        };
        assert_eq!(
            why(challenge_with("needcdkey", None)),
            "EXE_ERR_INVALID_CD_KEY"
        );
        assert_eq!(
            why(challenge_with("needcdkey", key())),
            "EXE_AWAITINGCDKEYAUTH"
        );
    }
}
