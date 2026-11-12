//! A CoD2 voice capture (docs/research/cod2-voice.md 6): a listener joins
//! and idles, then a talker joins; before its gamestate the talker sends
//! one packet with a byte length (the client's) and one with a short length
//! (what `SV_PreGameUserVoice` reads), then, once in the world, a packet of
//! two frames every 200 ms for 3 s. Every out-of-band `v` either client
//! hears is printed as `heard <client> <talker> <hex>`; nothing else is
//! sent but idle cmds.
//!
//!   voice_probe <ip:port> [--game-dir DIR] [--talker-qport N]

#[path = "common/mod.rs"]
mod common;

use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use vcod_cod2::net::Client;
use vcod_cod2::net::voice;

struct Peer {
    s: common::Session,
    cl: Client,
    qport: u16,
    name: &'static str,
    last_send: Instant,
    snap_at: Instant,
    active: bool,
}

impl Peer {
    /// A client on `qport`: two clients of one address need two.
    fn join(addr: &str, name: &'static str, qport: u16) -> anyhow::Result<Peer> {
        let mut s = common::Session::open(addr, false, 0)?;
        let ui = format!("\\name\\{name}\\rate\\25000\\snaps\\20\\cl_punkbuster\\0\\cl_voice\\1");
        let cl = s.connect_qport(&ui, qport)?;
        Ok(Peer {
            s,
            qport: cl.netchan.qport,
            cl,
            name,
            last_send: Instant::now() - Duration::from_secs(1),
            snap_at: Instant::now(),
            active: false,
        })
    }

    /// Reads what waits, answers a gamestate's `cp`, sends an idle cmd
    /// every 33 ms; prints each voice packet heard.
    fn step(&mut self, fs: Option<&vcod_common::pk3::Pk3Fs>, t0: Instant) -> anyhow::Result<()> {
        while let Some(d) = self.s.recv() {
            if let Some(pl) = vcod_cod2::net::oob::payload(&d) {
                if let Some(body) = voice::oob_body(pl) {
                    for (talker, f) in voice::parse_server(body) {
                        let hex: String = f.iter().map(|b| format!("{b:02x}")).collect();
                        println!(
                            "{:>5} heard {} talker {talker} {hex}",
                            t0.elapsed().as_millis(),
                            self.name
                        );
                    }
                }
                continue;
            }
            let Some(p) = self.cl.receive(&d)? else {
                continue;
            };
            if p.gamestate {
                let gs = self.cl.parser.gamestate.as_ref().unwrap();
                let cp = common::cp_for(fs, gs);
                self.cl.add_reliable(&cp);
            }
            if p.snapshot.is_some() {
                self.snap_at = Instant::now();
                self.active = true;
            }
        }
        if self.last_send.elapsed() >= Duration::from_millis(33) {
            self.last_send = Instant::now();
            let pkt = match self.cl.parser.gamestate.is_some() {
                true => {
                    let base = self.cl.parser.snapshot().map_or(0, |f| f.server_time);
                    let cmd = self
                        .cl
                        .idle_cmd(base + self.snap_at.elapsed().as_millis() as i32);
                    self.cl.build_packet(&[cmd])?
                }
                false => self.cl.build_packet(&[])?,
            };
            self.s.send(&pkt)?;
        }
        Ok(())
    }

    fn voice(&mut self, raw: &[u8]) -> anyhow::Result<()> {
        self.s.send(raw)
    }

    fn leave(&mut self) -> anyhow::Result<()> {
        self.cl.add_reliable("disconnect");
        for _ in 0..3 {
            let pkt = self.cl.build_packet(&[])?;
            self.s.send(&pkt)?;
        }
        Ok(())
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let addr = args.first().context("usage: voice_probe <ip:port>")?;
    let game_dir = args
        .iter()
        .position(|a| a == "--game-dir")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let fs = common::game_fs(game_dir)?;
    let t0 = Instant::now();
    // `--talker-qport N`: the talker's qport (0x2222 by default; one of
    // 0x8000 or more shows `SV_VoicePacket`'s signed read).
    let talker_qport = match args.iter().position(|a| a == "--talker-qport") {
        Some(i) => args.get(i + 1).context("--talker-qport N")?.parse()?,
        None => 0x2222,
    };
    let mut ear = Peer::join(addr, "ear", 0x1111)?;
    while !ear.active {
        if t0.elapsed() > Duration::from_secs(20) {
            bail!("the listener never got a snapshot");
        }
        ear.step(fs.as_ref(), t0)?;
        std::thread::sleep(Duration::from_millis(1));
    }
    println!("{:>5} ear active", t0.elapsed().as_millis());
    let mut talker = Peer::join(addr, "talker", talker_qport)?;
    let q = talker.qport.to_le_bytes();
    // Before the gamestate: the client's byte length, then a short one.
    let byte_len = voice::client_packet(talker.qport, &[vec![0x11, 0x22, 0x33]]);
    let mut short_len = vec![0xff, 0xff, 0xff, 0xff, b'v', 0, q[0], q[1], 1, 3, 0];
    short_len.extend_from_slice(&[0x44, 0x55, 0x66]);
    talker.voice(&byte_len)?;
    talker.voice(&short_len)?;
    println!(
        "{:>5} talker sent two pregame packets",
        t0.elapsed().as_millis()
    );
    let mut talk_from = None;
    let mut last_talk = Instant::now();
    let mut n = 0u8;
    loop {
        ear.step(fs.as_ref(), t0)?;
        talker.step(fs.as_ref(), t0)?;
        if talker.active && talk_from.is_none() {
            println!("{:>5} talker active", t0.elapsed().as_millis());
            talk_from = Some(Instant::now());
        }
        if let Some(from) = talk_from {
            if from.elapsed() > Duration::from_secs(4) {
                break;
            }
            if from.elapsed() < Duration::from_secs(3)
                && last_talk.elapsed() >= Duration::from_millis(200)
            {
                last_talk = Instant::now();
                n = n.wrapping_add(1);
                let frames = [vec![0xa0 | (n & 0xf); 10], vec![n; 3]];
                let p = voice::client_packet(talker.qport, &frames);
                talker.voice(&p)?;
                println!("{:>5} talker sent frames {n}", t0.elapsed().as_millis());
            }
        }
        if t0.elapsed() > Duration::from_secs(40) {
            bail!("timed out");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    talker.leave()?;
    ear.leave()?;
    Ok(())
}
