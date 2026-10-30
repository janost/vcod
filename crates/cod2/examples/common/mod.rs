//! What the CoD2 examples share: a UDP session that can log its datagrams,
//! the out-of-band handshake, and the `cp` owed after each gamestate.

use std::net::UdpSocket;
use std::time::{Duration, Instant};

use anyhow::bail;
use vcod_cod2::net::capture::{Capture, Dir};
use vcod_cod2::net::oob::{self, Reply};
use vcod_cod2::net::{Client, Gamestate2};
use vcod_common::pk3::Pk3Fs;

pub struct Session {
    pub sock: UdpSocket,
    pub capture: Option<Capture>,
    pub save_messages: usize,
    pub messages: usize,
}

impl Session {
    pub fn open(addr: &str, capture: bool, save_messages: usize) -> anyhow::Result<Session> {
        let sock = UdpSocket::bind("0.0.0.0:0")?;
        sock.connect(addr)?;
        sock.set_read_timeout(Some(Duration::from_millis(1)))?;
        Ok(Session {
            sock,
            capture: capture.then(Capture::default),
            save_messages,
            messages: 0,
        })
    }

    pub fn send(&mut self, p: &[u8]) -> anyhow::Result<()> {
        if let Some(c) = &mut self.capture
            && self.messages < self.save_messages
        {
            c.push(Dir::ToServer, p);
        }
        self.sock.send(p)?;
        Ok(())
    }

    pub fn recv(&mut self) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; 0x10000];
        let n = self.sock.recv(&mut buf).ok()?;
        buf.truncate(n);
        if let Some(c) = &mut self.capture
            && self.messages < self.save_messages
        {
            c.push(Dir::FromServer, &buf);
        }
        Some(buf)
    }

    /// Send `p` until an OOB reply `want` accepts arrives.
    pub fn ask<T>(&mut self, p: &[u8], want: impl Fn(&Reply) -> Option<T>) -> anyhow::Result<T> {
        for _ in 0..5 {
            self.send(p)?;
            let until = Instant::now() + Duration::from_secs(1);
            while Instant::now() < until {
                let Some(d) = self.recv() else { continue };
                let Some(pl) = oob::payload(&d) else { continue };
                let r = oob::parse_reply(pl);
                if let Reply::Error(e) = &r {
                    bail!("server refused: {e}");
                }
                if let Some(t) = want(&r) {
                    return Ok(t);
                }
            }
        }
        bail!("no answer")
    }

    /// `getchallenge`, then a plain-text `connect` with `userinfo`: the
    /// netchan's client side.
    pub fn connect(&mut self, userinfo: &str) -> anyhow::Result<Client> {
        let challenge = self.ask(&oob::getchallenge(None), |r| match r {
            Reply::Challenge(c) => Some(*c),
            _ => None,
        })?;
        let qport = std::process::id() as u16;
        self.ask(&oob::connect(userinfo, challenge, qport), |r| {
            matches!(r, Reply::Connect).then_some(())
        })?;
        println!("connected, challenge {challenge}");
        Ok(Client::new(qport, challenge))
    }
}

/// The install's iwds under `--game-dir` or `$COD2_DIR`, for `cp`.
pub fn game_fs(game_dir: Option<String>) -> anyhow::Result<Option<Pk3Fs>> {
    let dir = game_dir.or_else(|| std::env::var("COD2_DIR").ok());
    Ok(match dir {
        Some(d) => Some(Pk3Fs::open_iwd(
            &std::path::Path::new(&d).join("main"),
            None,
        )?),
        None => None,
    })
}

/// The `cp` for a gamestate: the referenced iwds with the map's `.d3dbsp`
/// read, or the bare key without an install (a pure server takes both).
pub fn cp_for(fs: Option<&Pk3Fs>, gs: &Gamestate2) -> String {
    let si = gs.configstring(1).to_string();
    let feed = gs.checksum_feed;
    match fs {
        Some(fs) => {
            fs.clear_references();
            let map =
                vcod_common::net::info_value_for_key(gs.configstring(0), "mapname").unwrap_or("");
            fs.touch(&format!("maps/mp/{map}.d3dbsp"));
            vcod_cod2::net::pure::pure_command(fs, feed, &si)
        }
        None => vcod_cod2::net::pure::cp_command(&[], feed),
    }
}
