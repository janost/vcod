//! A research client for CoD4x's protocol 21 as a server without client
//! authentication speaks it (`sv_noauth 1`, or no Steam library loaded):
//! challenge, a protocol 21 connect, the reliable side channel (stats, the
//! gamestate), the 16-byte client header, and moves until `--secs` past the
//! gamestate. It prints what it reads; `tools/lo_capture.py` keeps the
//! datagrams. It does no authentication and answers no anti-cheat or
//! screenshot request (cod4x-protocol.md 5 and 18). The side channel's
//! packets, the gamestate, the snapshots and the client header go through
//! vcod's codec in its CoD4x 21 dialect (`wire::Dialect::X21`).
//!
//!     cargo run -p vcod-cod4 --example cod4x_probe -- HOST:PORT OUTDIR [--secs 15]
//!         [--protocol 21] [--stats none|skip|fresh] [--qport N] [--cmd "SECS TEXT"]...
//!         [--name NAME] [--until-stats] [--log FILE]
//!     cargo run -p vcod-cod4 --example cod4x_probe -- --gamestate OUTDIR/rel-<n>-1.bin
//!     cargo run -p vcod-cod4 --example cod4x_probe -- --baseline17 CAPTURE.txt ENTITY
//!
//! A `--cmd` text may hold `{sid}` (the serverid) and `{menu:NAME}` (a
//! menu's index), for `mr {sid} {menu:team_marinesopfor} autoassign`.
//! `--stats fresh` (the default) answers the stats request with a fresh
//! profile's block in mode 2 (`openssl` on the path), `skip` with mode 0
//! (no stats, which the stock scripts kick), `none` not at all.
//! `--until-stats` ends the join at the stats request, or `--secs` after the
//! start without one, for a reach check that sends no stats. `--log` writes
//! every datagram as `<ms> <c2s|s2c> <peer> <hex>` (`tools/lo_capture.py`'s
//! lines). On the way out the probe sends `disconnect` in three messages.
//! OUTDIR gets each reliable message the server sent as `rel-<n>-<cmd>.bin`
//! (its body, past the length word) and `gamestate.txt`, the decoded
//! gamestate.

use anyhow::{Context, Result, bail, ensure};
use std::collections::BTreeMap;
use std::io::Write;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};
use vcod_cod4::net::clmsg::{self, ClOp, ClientHeader, CmdWire, UserCmd};
use vcod_cod4::net::delta::EntityState;
use vcod_cod4::net::parse::{Client, GamestateOp, Op};
use vcod_cod4::net::reliable::{self, Inbound, Packet, StatsMsg};
use vcod_cod4::net::wire::Dialect;
use vcod_common::net::netchan::{Netchan, NetchanParams, scramble_client_ops};

const REL_WINDOW: i16 = 20;

/// Client half of the reliable side channel (`ReliableMessage*`).
#[derive(Default)]
struct Rel {
    qport: u16,
    inbound: Inbound,
    /// Our fragments, by sequence, until the server acknowledges them.
    out: BTreeMap<i32, Vec<u8>>,
    next_out: i32,
    last_sent: Option<Instant>,
}

impl Rel {
    fn packet(&self, sequence: i32, data: &[u8]) -> Vec<u8> {
        Packet {
            qport: self.qport,
            sequence,
            ack: self.inbound.next(),
            window: REL_WINDOW,
            data: data.to_vec(),
            ..Packet::default()
        }
        .write()
    }

    /// Takes one server packet; returns whole message bodies it completed.
    fn take(&mut self, d: &[u8]) -> Result<Vec<Vec<u8>>> {
        let p = Packet::read(d)?;
        self.out.retain(|s, _| *s >= p.ack);
        self.inbound.take(&p)
    }

    fn queue(&mut self, body: &[u8]) {
        for c in reliable::frame(body).chunks(reliable::MAX_FRAGMENT) {
            self.out.insert(self.next_out, c.to_vec());
            self.next_out += 1;
        }
    }

    /// The packets due now: unacknowledged fragments every 300 ms, else a
    /// bare acknowledge.
    fn due(&mut self, now: Instant) -> Vec<Vec<u8>> {
        if self
            .last_sent
            .is_some_and(|t| now.duration_since(t) < Duration::from_millis(300))
        {
            return Vec::new();
        }
        self.last_sent = Some(now);
        if self.out.is_empty() {
            return vec![self.packet(reliable::BARE_ACK, &[])];
        }
        self.out.iter().map(|(s, d)| self.packet(*s, d)).collect()
    }
}

/// A baseline's non-zero fields, one per line.
fn baseline_text(num: u32, e: &EntityState) -> String {
    let mut out = format!("baseline {num} etype {}\n", e.etype());
    for f in EntityState::table(e.etype()) {
        let w = e.0[f.word()];
        if w != 0 {
            out.push_str(&format!(
                "    {} ({}) = {w:#x} / {}\n",
                f.name,
                f.bits,
                f32::from_bits(w)
            ));
        }
    }
    out
}

/// A fresh profile's stats block as `SV_ReceiveStats_f` (0x10f5b0) takes
/// mode 2: AES-128-CBC, the key the connection's challenge four times,
/// a fixed IV. Runs the `openssl` CLI, so the probe links no cipher.
fn stats_mode2(challenge: i32) -> Result<Vec<u8>> {
    use std::process::{Command, Stdio};
    let key: String = (0..4)
        .flat_map(|_| challenge.to_le_bytes())
        .map(|b| format!("{b:02x}"))
        .collect();
    let iv: String = [0xeb62114fu32, 0x66996144, 0x7341cfa4, 0xb9551299]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut child = Command::new("openssl")
        .args(["enc", "-aes-128-cbc", "-nopad", "-K", &key, "-iv", &iv])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let plain = vcod_cod4::net::stats::Stats::fresh();
    child
        .stdin
        .take()
        .context("stdin")?
        .write_all(&plain.0[..])?;
    let out = child.wait_with_output()?;
    ensure!(
        out.status.success() && out.stdout.len() == reliable::STATS_LEN,
        "openssl enc failed"
    );
    Ok(out.stdout)
}

/// A gamestate side channel message (`cmd 1`) read into `client`, and its
/// text: the configstrings, baselines, client config and trailer.
fn read_gamestate(client: &mut Client, body: &[u8]) -> Result<String> {
    client.take_side_gamestate(body)?;
    let gs = client.gamestate.as_ref().context("gamestate")?;
    let mut out = String::new();
    for op in &gs.ops {
        match op {
            GamestateOp::Configstrings(sent) => {
                out.push_str(&format!("configstrings {}\n", sent.len()));
                for (i, s) in sent {
                    out.push_str(&format!("cs {i} {:?}\n", String::from_utf8_lossy(s)));
                }
            }
            GamestateOp::Baseline(n, _) => out.push_str(&baseline_text(*n, &gs.baselines[n])),
            GamestateOp::ClientConfig(c) => out.push_str(&format!(
                "clientconfig {} {:?} {:?}\n",
                c.client_num,
                String::from_utf8_lossy(&c.name),
                String::from_utf8_lossy(&c.clan)
            )),
        }
    }
    out.push_str(&format!(
        "configSequence {} clientNum {} checksumFeed {:#x} tail {}\n",
        gs.config_data_sequence, gs.client_num, gs.checksum_feed, gs.tail
    ));
    Ok(out)
}

/// The socket, with every datagram it moves logged when `--log` is set.
struct Wire {
    sock: UdpSocket,
    server: SocketAddr,
    start: Instant,
    log: Option<std::fs::File>,
}

impl Wire {
    fn note(&mut self, dir: &str, d: &[u8]) -> Result<()> {
        if let Some(f) = &mut self.log {
            let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
            let ms = self.start.elapsed().as_millis();
            writeln!(f, "{ms} {dir} {} {hex}", self.server)?;
        }
        Ok(())
    }

    fn send(&mut self, d: &[u8]) -> Result<()> {
        self.note("c2s", d)?;
        self.sock.send_to(d, self.server)?;
        Ok(())
    }

    fn oob(&mut self, t: &str) -> Result<()> {
        let mut p = vec![0xff; 4];
        p.extend_from_slice(t.as_bytes());
        self.send(&p)
    }

    /// One datagram from the server, if one came within the read timeout.
    fn recv(&mut self, buf: &mut [u8]) -> Result<Option<usize>> {
        match self.sock.recv_from(buf) {
            Ok((n, from)) if from == self.server => {
                self.note("s2c", &buf[..n])?;
                Ok(Some(n))
            }
            _ => Ok(None),
        }
    }
}

fn wait_oob(w: &mut Wire, buf: &mut [u8], prefix: &str, t: Duration) -> Result<Option<String>> {
    let end = Instant::now() + t;
    while Instant::now() < end {
        if let Some(n) = w.recv(buf)?
            && n >= 4
            && buf[..4] == [0xff; 4]
        {
            let s = String::from_utf8_lossy(&buf[4..n]).into_owned();
            println!("oob << {s:?}");
            if s.starts_with(prefix) || s.starts_with("error") {
                return Ok(Some(s));
            }
        }
    }
    Ok(None)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--gamestate") {
        // Offline: decode a saved `rel-<n>-1.bin`.
        let body = std::fs::read(args.get(2).context("FILE")?)?;
        print!(
            "{}",
            read_gamestate(&mut Client::with_dialect(Dialect::X21), &body)?
        );
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("--baseline17") {
        // A 1.7 capture's baseline, to set beside a protocol 21 one.
        let text = std::fs::read_to_string(args.get(2).context("CAPTURE")?)?;
        let n: u32 = args.get(3).context("ENTITY")?.parse()?;
        let mut session = vcod_cod4::net::capture::Session::new();
        for l in vcod_cod4::net::capture::parse(&text)? {
            session.feed(&l)?;
            if let Some(e) = session
                .client
                .gamestate
                .as_ref()
                .and_then(|g| g.baselines.get(&n))
            {
                print!("{}", baseline_text(n, e));
                return Ok(());
            }
        }
        bail!("no baseline {n}");
    }
    let server: SocketAddr = args.get(1).context("HOST:PORT")?.parse()?;
    let outdir = std::path::PathBuf::from(args.get(2).context("OUTDIR")?);
    std::fs::create_dir_all(&outdir)?;
    let opt = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
    };
    let secs: f32 = opt("--secs").map_or(Ok(15.0), |s| s.parse())?;
    let protocol: i32 = opt("--protocol").map_or(Ok(21), |s| s.parse())?;
    let qport: u16 = opt("--qport").map_or(Ok(0x1234), |s| s.parse())?;
    let stats = opt("--stats").map_or("fresh", String::as_str).to_string();
    let name = opt("--name").map_or_else(|| format!("vcodx{qport}"), String::clone);
    let until_stats = args.iter().any(|a| a == "--until-stats");
    let mut cmds: Vec<(f32, String)> = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if a == "--cmd" {
            let v = args.get(i + 1).context("--cmd value")?;
            let (t, text) = v.split_once(' ').context("--cmd \"SECS TEXT\"")?;
            cmds.push((t.parse()?, text.to_string()));
        }
    }

    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.set_read_timeout(Some(Duration::from_millis(10)))?;
    let log = opt("--log").map(std::fs::File::create).transpose()?;
    let mut w = Wire {
        sock,
        server,
        start: Instant::now(),
        log,
    };
    let mut buf = vec![0u8; 65536];
    w.oob("getchallenge 0 \"97c8407fc937bc7c1eddecf36de89e75\"")?;
    let c = wait_oob(
        &mut w,
        &mut buf,
        "challengeResponse",
        Duration::from_secs(3),
    )?
    .context("no challenge")?;
    let challenge: i32 = c.split_whitespace().nth(1).context("challenge")?.parse()?;
    let ui = format!(
        "\\cg_predictItems\\1\\cl_punkbuster\\0\\cl_anonymous\\0\\color\\4\\head\\default\\model\\multi\
         \\snaps\\20\\rate\\25000\\name\\{name}\\protocol\\{protocol}\\challenge\\{challenge}\\qport\\{qport}"
    );
    println!("userinfo {ui:?}");
    w.oob(&format!("connect \"{ui}\""))?;
    let c = wait_oob(&mut w, &mut buf, "connectResponse", Duration::from_secs(3))?
        .context("no connectResponse")?;
    if !c.starts_with("connectResponse") {
        bail!("refused: {c:?}");
    }

    let mut chan = Netchan::with_params(qport, challenge, NetchanParams::COD2);
    let mut client = Client::with_dialect(Dialect::X21);
    let mut rel = Rel {
        qport,
        ..Rel::default()
    };
    let mut ack: i32 = 0;
    let mut out_seq: u32 = 1;
    let mut nrel = 0;
    let mut gs_at: Option<Instant> = None;
    let mut last_snap_at = Instant::now();
    let mut next_send = Instant::now();
    let mut reliable_seq: i32 = 0;
    let mut reliable: BTreeMap<i32, String> = BTreeMap::new();
    let mut sent: BTreeMap<i32, String> = BTreeMap::new();
    let (mut parsed, mut exact) = (0usize, 0usize);
    let start = Instant::now();
    let mut last_cmd = UserCmd::default();
    let mut stats_asked = false;
    // Messages left to send once the run is over, each carrying `disconnect`.
    let mut leaving: Option<u8> = None;
    loop {
        let over = gs_at.is_some_and(|t| t.elapsed().as_secs_f32() > secs)
            || (until_stats && (stats_asked || start.elapsed().as_secs_f32() > secs))
            || start.elapsed() > Duration::from_secs_f32(secs + 30.0);
        if over && leaving.is_none() {
            reliable_seq += 1;
            reliable.insert(reliable_seq, "disconnect".into());
            sent.insert(reliable_seq, "disconnect".into());
            println!("command {reliable_seq}: disconnect");
            leaving = Some(3);
        }
        if leaving == Some(0) {
            break;
        }
        // Receive.
        while let Some(n) = w.recv(&mut buf)? {
            let d = buf[..n].to_vec();
            if n < 4 {
                continue;
            }
            let word = i32::from_le_bytes(d[..4].try_into().unwrap());
            if word == -1 {
                println!("oob << {:?}", String::from_utf8_lossy(&d[4..]));
                continue;
            }
            if Packet::is_side(&d) {
                for body in rel.take(&d)? {
                    nrel += 1;
                    let cmd = i32::from_le_bytes(body[..4].try_into().unwrap_or([0; 4]));
                    println!("rel << message {nrel}: cmd {cmd}, {} bytes", body.len());
                    std::fs::write(outdir.join(format!("rel-{nrel}-{cmd}.bin")), &body)?;
                    stats_asked |= cmd == 9;
                    match cmd {
                        reliable::CMD_STATS if stats == "fresh" => {
                            let m = StatsMsg {
                                mode: 2,
                                block: stats_mode2(challenge)?,
                            };
                            rel.queue(&m.write());
                            println!("rel >> stats, mode 2, a fresh profile");
                        }
                        reliable::CMD_STATS if stats == "skip" => {
                            // Mode 0: no stats (`SV_ReceiveStats_f` 0x10f5b0).
                            rel.queue(&StatsMsg::default().write());
                            println!("rel >> stats, mode 0");
                        }
                        reliable::CMD_GAMESTATE => match read_gamestate(&mut client, &body) {
                            Ok(text) => {
                                let gs = client.gamestate.as_ref().context("gamestate")?;
                                println!(
                                    "gamestate: {} configstrings, {} baselines, client {}",
                                    gs.configstrings.len(),
                                    gs.baselines.len(),
                                    gs.client_num
                                );
                                std::fs::write(outdir.join("gamestate.txt"), text)?;
                                gs_at = Some(Instant::now());
                            }
                            Err(e) => println!("gamestate parse failed: {e:#}"),
                        },
                        _ => {}
                    }
                }
                continue;
            }
            // The server keys its messages on our command it acknowledges.
            let key = |_: &Netchan, a| {
                sent.get(&a)
                    .map_or_else(Vec::new, |t| t.as_bytes().to_vec())
            };
            let Some((seq, msg)) = chan.process_keyed(&d, key)? else {
                continue;
            };
            ack = seq as i32;
            if msg.len() < 4 {
                continue;
            }
            let rack = i32::from_le_bytes(msg[..4].try_into().unwrap());
            reliable.retain(|s, _| *s > rack);
            match client.parse(seq, &msg[4..]) {
                Ok(p) => {
                    parsed += 1;
                    exact += usize::from(p.exact);
                    for op in &p.ops {
                        match op {
                            Op::ServerCommand { sequence, text } => println!(
                                "s2c {seq}: cmd {sequence} {:?}",
                                String::from_utf8_lossy(text)
                            ),
                            Op::Snapshot(i) => {
                                last_snap_at = Instant::now();
                                let s = &client.snapshots[*i];
                                println!(
                                    "s2c {seq}: snapshot t {} delta {} ents {} clients {}",
                                    s.server_time,
                                    s.delta_num,
                                    s.entities.len(),
                                    s.clients.len()
                                );
                            }
                            Op::ClientConfig { sequence, config } => println!(
                                "s2c {seq}: client config {sequence}: {} {:?}",
                                config.client_num,
                                String::from_utf8_lossy(&config.name)
                            ),
                            o => println!("s2c {seq}: {o:?}"),
                        }
                    }
                }
                Err(e) => {
                    let plain = client.huff.decompress(&msg[4..]);
                    let hex: String = plain.iter().take(64).map(|b| format!("{b:02x}")).collect();
                    println!("s2c {seq}: parse failed: {e:#}; plain {hex}");
                }
            }
        }
        // Send.
        let now = Instant::now();
        if leaving.is_none() {
            for p in rel.due(now) {
                w.send(&p)?;
            }
        }
        if now < next_send {
            continue;
        }
        next_send = now + Duration::from_millis(50);
        if let Some(n) = &mut leaving {
            *n -= 1;
        }
        if let Some(t) = gs_at {
            let el = t.elapsed().as_secs_f32();
            for (at, text) in cmds.iter_mut() {
                if el >= *at && !text.is_empty() {
                    // `{sid}` and `{menu:NAME}` as the 1.7 probe's `mr` lines.
                    let gs = client.gamestate.as_ref().context("gamestate")?;
                    let sid = gs.systeminfo("sv_serverid").unwrap_or_default();
                    *text = text.replace("{sid}", &sid);
                    while let Some(i) = text.find("{menu:") {
                        let end = text[i..].find('}').context("{menu:NAME}")? + i;
                        let name = text[i + 6..end].to_string();
                        let n = (0..32u16)
                            .find(|&k| gs.configstring(1970 + k) == name.as_bytes())
                            .with_context(|| format!("no menu {name}"))?;
                        text.replace_range(i..=end, &n.to_string());
                    }
                    reliable_seq += 1;
                    reliable.insert(reliable_seq, text.clone());
                    sent.insert(reliable_seq, text.clone());
                    println!("command {reliable_seq}: {text}");
                    text.clear();
                }
            }
        }
        let server_id: i32 = client
            .gamestate
            .as_ref()
            .and_then(|g| g.systeminfo("sv_serverid"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let last = client
            .server_commands
            .get(&client.server_command_sequence)
            .cloned()
            .unwrap_or_default();
        let feed = client.gamestate.as_ref().map_or(0, |g| g.checksum_feed);
        let key = clmsg::usercmd_key(feed, ack, &last);
        let mut ops: Vec<ClOp> = reliable
            .range(1..)
            .map(|(s, t)| ClOp::Command {
                sequence: *s,
                text: t.as_bytes().to_vec(),
            })
            .collect();
        if client.gamestate.is_some() && leaving.is_none() {
            let t = client.snapshots.last().map_or(0, |s| {
                (s.server_time + last_snap_at.elapsed().as_millis() as i32).raw()
            });
            let c = UserCmd {
                server_time: t.max(last_cmd.server_time + 1),
                ..UserCmd::default()
            };
            ops.push(ClOp::Move {
                delta: !client.snapshots.is_empty(),
                cmds: vec![(c, CmdWire::full(&UserCmd::default(), &c))],
                origin: [1.0e6f32.to_bits(); 3],
                origin_time: 0,
            });
            last_cmd = c;
        }
        ops.push(ClOp::Eof);
        let mut comp = clmsg::compress_ops(&client.huff, &ops, key, &UserCmd::default());
        let head = ClientHeader {
            sequence: out_seq,
            qport,
            server_id,
            message_ack: ack,
            reliable_ack: client.server_command_sequence,
            config_ack: client.config_data_ack,
        };
        out_seq += 1;
        scramble_client_ops(head.seed(challenge), &last, &mut comp);
        let mut pkt = Vec::new();
        head.write(Dialect::X21, &mut pkt);
        pkt.extend_from_slice(&comp);
        w.send(&pkt)?;
    }
    println!(
        "result: stats_requested {stats_asked} gamestate {}",
        client.gamestate.is_some()
    );
    println!(
        "done: {} snapshots; {parsed} server messages parsed, {exact} to their last byte",
        client.total_snapshots()
    );
    if let Some(sn) = client.snapshots.last() {
        let o: Vec<f32> = [0, 1, 2]
            .iter()
            .filter_map(|i| sn.ps.get(&format!("origin[{i}]")))
            .map(f32::from_bits)
            .collect();
        println!(
            "last playerstate origin {o:?}, {} entities",
            sn.entities.len()
        );
    }
    Ok(())
}
