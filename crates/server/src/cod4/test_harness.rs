//! The in-process CoD4 test harness the A/B gates and the CoD4 server
//! tests share: [`Probe`], the 1.7 client a test joins to
//! [`Cod4Server`] over the server's own calls, the captures under
//! `crates/cod4/tests/fixtures/net` ([`retail`], [`side`]), the replay
//! schedules of a capture's cmds, and the kill capture's two-client set-up
//! ([`ours_set_up`]). The frame contract it drives is
//! docs/research/cod4-server-frame.md 2.

use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use vcod_cod4::net::capture::{self, Item, Line, Session};
use vcod_cod4::net::clmsg::{CmdWire, compress_ops};
use vcod_cod4::net::parse::{Client as Parser, Op, Snapshot};
use vcod_common::net::netchan::{Netchan, scramble_client_ops};

use super::*;

pub(super) const FIXTURES: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../cod4/tests/fixtures/net");

/// What one client read: the gamestate, its server commands by sequence
/// (each once, as the client executes it), and its snapshots.
pub(super) struct Side {
    pub(super) gs: vcod_cod4::net::parse::Gamestate,
    pub(super) commands: Vec<(i32, String)>,
    pub(super) snaps: Vec<Snapshot>,
}

pub(super) fn side(lines: &[Line]) -> Side {
    let mut s = Session::new();
    let mut commands = BTreeMap::new();
    let mut last_time = 0;
    for l in lines.iter().filter(|l| l.dir != "auth") {
        if let Ok(Some(Item::Server { parsed, .. })) = s.feed(l) {
            for op in &parsed.ops {
                match op {
                    Op::ServerCommand { sequence, text } => {
                        commands
                            .entry(*sequence)
                            .or_insert((last_time, String::from_utf8_lossy(text).into_owned()));
                    }
                    Op::Snapshot(i) => last_time = s.client.snapshots[*i].server_time.raw(),
                    _ => {}
                }
            }
        }
    }
    Side {
        gs: s.client.gamestate.clone().expect("a gamestate"),
        commands: commands.into_values().collect(),
        snaps: s.client.snapshots.clone(),
    }
}

pub(super) fn retail(name: &str) -> Vec<Line> {
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-{name}.txt")).unwrap();
    capture::parse(&text).unwrap()
}

/// One in-process client: the join `vcod_cod4::net::conn` makes, over the
/// server's own calls instead of a socket.
pub(super) struct Probe {
    name: &'static str,
    pub(super) addr: SocketAddr,
    pub(super) qport: u16,
    challenge: i32,
    chan: Netchan,
    parser: Parser,
    seq: u32,
    /// The messageAcknowledge each packet carries: the last message read.
    pub(super) ack: i32,
    commands: Vec<Vec<u8>>,
    commands_acked: i32,
    last_cmd: UserCmd,
    pub(super) lines: Vec<Line>,
    /// The server time it connects at and the one its retail twin's
    /// first snapshot reads: no cmd goes out before it.
    pub(super) connect_at: i32,
    first_snap: i32,
    /// The commands it sends past its first snapshot: (ms, text), `{sid}`
    /// standing for the `sv_serverid` an `mr` names.
    answers: VecDeque<(i32, &'static str)>,
    pub(super) joined: bool,
    /// Its slot, and the view it holds from its spawn on.
    slot: u32,
    view: Option<[f32; 2]>,
    /// The capture's own cmds after the spawn, each packet's under the
    /// server time it is sent at ([`replay_schedule`]).
    pub(super) replay: VecDeque<(i32, Vec<UserCmd>)>,
    /// The spawn the replay starts after.
    pub(super) replay_from: i32,
    /// The `pm_type` of its own snapshot that starts the replay: 0, or a
    /// spectator's 4.
    pub(super) replay_pm_type: u32,
    /// Its retail twin's cmd angles by cmd time ([`cmd_angles`]): before
    /// the replay each cmd takes the newest at or before its own time, so
    /// the spawn's `delta_angles` (the spawn's view less the last cmd's)
    /// come out as retail's. Empty: `cod4_probe`'s idle view.
    pub(super) pre_angles: Vec<(i32, [u16; 3])>,
    /// A `requeststats` came: the seven stats packets go again
    /// ([`Self::answer_stats`]).
    stats_due: bool,
    /// The checksum feed of the gamestate a `cp` answered.
    cp_feed: Option<i32>,
    /// Its moves carry the newest snapshot's origin at the newest cmd's
    /// time, as a 1.7 client predicting nothing sends them, and file the
    /// row (`Archive::unpredicted`). Off, they carry what `cod4_probe`
    /// and so every capture sent: an origin far off at time 0, which keeps
    /// origin and velocity on the wire (cod4-netfields.md 13).
    pub(super) echo_origin: bool,
}

impl Probe {
    pub(super) fn new(
        name: &'static str,
        slot: u32,
        port: u16,
        first_snap: i32,
        answers: &[(i32, &'static str)],
    ) -> Probe {
        Probe {
            name,
            addr: SocketAddr::from(([127, 0, 0, 1], port)),
            qport: 4000 + port % 1000,
            challenge: 0,
            chan: Netchan::with_params(0, 0, NetchanParams::COD2),
            parser: Parser::new(),
            seq: 1,
            ack: 0,
            commands: Vec::new(),
            commands_acked: 0,
            last_cmd: UserCmd::default(),
            lines: Vec::new(),
            // The gamestate's fragments take about 3.2 s here (2.9 s on
            // retail's walker); the first cmd waits for its twin's time.
            connect_at: first_snap - 3600,
            first_snap,
            answers: answers.iter().copied().collect(),
            joined: false,
            slot,
            view: None,
            replay: VecDeque::new(),
            replay_from: WALKER_SPAWN_MS,
            replay_pm_type: 0,
            pre_angles: Vec::new(),
            stats_due: false,
            cp_feed: None,
            echo_origin: false,
        }
    }

    /// The snapshots its parser holds, a playerstate sent without origin
    /// filled off its own archive.
    pub(super) fn snapshots(&self) -> &[Snapshot] {
        &self.parser.snapshots
    }

    /// The predicted origin and time a move whose newest cmd is `cmd`
    /// carries ([`Self::echo_origin`]).
    fn move_origin(&mut self, cmd: &UserCmd) -> ([u32; 3], i32) {
        let snap = self.parser.snapshots.last().map(|s| &s.ps);
        let Some(ps) = snap.filter(|_| self.echo_origin) else {
            return ([1.0e6f32.to_bits(); 3], 0);
        };
        let row = vcod_cod4::net::demo::Archive::unpredicted(ps, Some(cmd.angles), cmd.server_time);
        self.parser.archive.add(row);
        (row.origin.map(f32::to_bits), row.server_time)
    }

    /// The seven stats packets again after a `requeststats` (a map
    /// change's, cod4-map-cycle.md 1); their answers stay in the outbox.
    pub(super) fn answer_stats(&mut self, sv: &mut Cod4Server, now: Instant) {
        if !std::mem::take(&mut self.stats_due) {
            return;
        }
        let stats = vcod_cod4::net::stats::Stats::fresh();
        for idx in 0..vcod_cod4::net::stats::STAT_PACKETS {
            let mut p = vec![0xff; 4];
            p.extend_from_slice(b"stats\0");
            p.extend_from_slice(&self.qport.to_le_bytes());
            p.push(idx as u8);
            p.extend_from_slice(stats.packet(idx));
            self.log("c2s", sv.core.sv_time_ms, &p);
            sv.handle_packet(self.addr, &p, now);
        }
    }

    /// Every server command it has read, in sequence order.
    pub(super) fn server_commands(&self) -> Vec<String> {
        self.parser
            .server_commands
            .values()
            .map(|t| String::from_utf8_lossy(t).into_owned())
            .collect()
    }

    /// Configstring `i` as its gamestate and the `d`s since hold it.
    pub(super) fn configstring(&self, i: u16) -> String {
        self.parser
            .gamestate
            .as_ref()
            .map_or_else(String::new, |g| {
                String::from_utf8_lossy(g.configstring(i)).into_owned()
            })
    }

    /// The `sv_serverid` of the gamestate it holds.
    pub(super) fn server_id(&self) -> Option<i32> {
        self.parser
            .gamestate
            .as_ref()?
            .systeminfo("sv_serverid")?
            .parse()
            .ok()
    }

    pub(super) fn log(&mut self, dir: &str, ms: i32, data: &[u8]) {
        self.lines.push(Line {
            ms: ms as u64,
            dir: dir.into(),
            data: data.to_vec(),
        });
    }

    pub(super) fn oob(&mut self, sv: &mut Cod4Server, text: &[u8], now: Instant) -> Vec<Vec<u8>> {
        let mut d = vec![0xff; 4];
        d.extend_from_slice(text);
        let t = sv.core.sv_time_ms;
        self.log("c2s", t, &d);
        sv.handle_packet(self.addr, &d, now);
        let mine: Vec<Vec<u8>> = sv
            .take_outgoing()
            .into_iter()
            .filter(|(to, _)| *to == self.addr)
            .map(|(_, p)| p)
            .collect();
        for p in &mine {
            self.log("s2c", t, p);
        }
        mine
    }

    /// Challenge, connect and the seven stats packets, as the 1.7 client.
    pub(super) fn join(&mut self, sv: &mut Cod4Server, now: Instant) {
        let reply = self.oob(
            sv,
            format!("getchallenge 0 \"{}\"", vcod_cod4::net::conn::KEY_HASH).as_bytes(),
            now,
        );
        let text = String::from_utf8_lossy(&reply[0][4..]).into_owned();
        self.challenge = text.split_whitespace().nth(1).unwrap().parse().unwrap();
        let ui = format!(
            "\\cg_predictItems\\1\\cl_punkbuster\\0\\cl_anonymous\\0\\color\\4\\head\\default\\model\\multi\\snaps\\20\\rate\\25000\\name\\{}\\protocol\\6\\challenge\\{}\\qport\\{}",
            self.name, self.challenge, self.qport
        );
        self.oob(sv, format!("connect \"{ui}\"").as_bytes(), now);
        let stats = vcod_cod4::net::stats::Stats::fresh();
        for idx in 0..vcod_cod4::net::stats::STAT_PACKETS {
            let mut p = b"stats\0".to_vec();
            p.extend_from_slice(&self.qport.to_le_bytes());
            p.push(idx as u8);
            p.extend_from_slice(stats.packet(idx));
            self.oob(sv, &p, now);
        }
        self.chan = Netchan::with_params(self.qport, self.challenge, NetchanParams::COD2);
        self.joined = true;
    }

    /// One move message: the unacked commands, and a cmd once the level is
    /// in and its twin's first snapshot is due.
    pub(super) fn packet(&mut self, svs: i32) -> Vec<u8> {
        let mut pkt = self.seq.to_le_bytes().to_vec();
        self.seq += 1;
        pkt.extend_from_slice(&self.qport.to_le_bytes());
        let Some(gs) = &self.parser.gamestate else {
            pkt.push(0);
            pkt.extend_from_slice(&self.ack.to_le_bytes());
            pkt.extend_from_slice(&0i32.to_le_bytes());
            return pkt;
        };
        let server_id: i32 = gs.systeminfo("sv_serverid").unwrap().parse().unwrap();
        let last = self.parser.last_server_command().to_vec();
        let key = clmsg::usercmd_key(gs.checksum_feed, self.ack, &last);
        let mut ops: Vec<ClOp> = (self.commands_acked as usize..self.commands.len())
            .map(|i| ClOp::Command {
                sequence: i as i32 + 1,
                text: self.commands[i].clone(),
            })
            .collect();
        // The walker replays from its first own spawned snapshot on, a
        // spectator from its first free one.
        let spawned = self.parser.snapshots.last().is_some_and(|s| {
            s.ps.get("pm_type") == Some(self.replay_pm_type)
                && s.ps.get("clientNum") == Some(self.slot)
        });
        let replaying = !self.replay.is_empty() && (spawned || self.view.is_some());
        if replaying && self.view.is_none() {
            self.view = Some([0.0; 2]);
        }
        if replaying {
            let mut cmds = Vec::new();
            while let Some((at, c)) = self.replay.front()
                && *at <= svs
            {
                cmds.extend(c.iter().filter(|c| c.server_time > self.replay_from));
                self.replay.pop_front();
            }
            if !cmds.is_empty() {
                let mut prev = UserCmd::default();
                let wired = cmds
                    .iter()
                    .map(|c| {
                        let w = CmdWire::full(&prev, c);
                        prev = *c;
                        (*c, w)
                    })
                    .collect();
                self.last_cmd = *cmds.last().unwrap();
                let newest = self.last_cmd;
                let (origin, origin_time) = self.move_origin(&newest);
                ops.push(ClOp::Move {
                    delta: true,
                    cmds: wired,
                    origin,
                    origin_time,
                });
            }
        } else if svs + FRAME_MS >= self.first_snap {
            let last = self.parser.snapshots.last();
            let snap = last.map_or(svs, |s| s.server_time.raw());
            // `cod4_probe`'s view: (0, 0) until its spawn, the spawn's view
            // after, each over the snapshot's `delta_angles`.
            let ps = last.map(|s| s.ps.clone()).unwrap_or_default();
            if self.view.is_none()
                && last.is_some()
                && ps.get("pm_type") == Some(0)
                && ps.get("clientNum") == Some(self.slot)
            {
                self.view =
                    Some([0, 1].map(|i| ps.get_f32(&format!("viewangles[{i}]")).unwrap_or(0.0)));
            }
            let view = self.view.unwrap_or([0.0; 2]);
            let delta = |i: usize| {
                vcod_cod4::net::conn::angle2short(
                    ps.get_f32(&format!("delta_angles[{i}]")).unwrap_or(0.0),
                )
            };
            let server_time = (snap + 10).max(self.last_cmd.server_time + 1);
            let angles = self
                .pre_angles
                .iter()
                .rev()
                .find(|(t, _)| *t <= server_time)
                .map_or_else(
                    || {
                        [
                            vcod_cod4::net::conn::angle2short(view[0]).wrapping_sub(delta(0)),
                            vcod_cod4::net::conn::angle2short(view[1]).wrapping_sub(delta(1)),
                            0u16.wrapping_sub(delta(2)),
                        ]
                    },
                    |(_, a)| *a,
                );
            let cmd = UserCmd {
                server_time,
                angles,
                ..self.last_cmd
            };
            let wire = CmdWire::full(&UserCmd::default(), &cmd);
            self.last_cmd = cmd;
            let (origin, origin_time) = self.move_origin(&cmd);
            ops.push(ClOp::Move {
                delta: !self.parser.snapshots.is_empty(),
                cmds: vec![(cmd, wire)],
                origin,
                origin_time,
            });
        }
        ops.push(ClOp::Eof);
        let mut comp = compress_ops(&self.parser.huff, &ops, key, &UserCmd::default());
        let seed = self.challenge as u32 ^ server_id as u32 ^ self.ack as u32;
        scramble_client_ops(seed as u8, &last, &mut comp);
        pkt.push(server_id as u8);
        pkt.extend_from_slice(&self.ack.to_le_bytes());
        pkt.extend_from_slice(&self.parser.server_command_sequence.to_le_bytes());
        pkt.extend_from_slice(&comp);
        pkt
    }

    /// Queues a reliable client command now, `{sid}` the gamestate's
    /// `sv_serverid`; false before the gamestate.
    pub(super) fn command(&mut self, text: &str) -> bool {
        let Some(gs) = &self.parser.gamestate else {
            return false;
        };
        let sid = gs.systeminfo("sv_serverid").unwrap_or_default();
        self.commands.push(text.replace("{sid}", &sid).into_bytes());
        true
    }

    pub(super) fn receive(&mut self, svs: i32, d: &[u8]) {
        self.log("s2c", svs, d);
        if d.len() >= 4 && d[..4] == [0xff; 4] {
            self.stats_due |= d[4..].starts_with(b"requeststats");
            return;
        }
        if d.len() < 4 {
            return;
        }
        let Ok(Some((s, mut msg))) = self.chan.frame(d) else {
            return;
        };
        if msg.len() < 4 {
            return;
        }
        let rack = i32::from_le_bytes(msg[..4].try_into().unwrap());
        let key = match rack {
            1.. => self
                .commands
                .get(rack as usize - 1)
                .cloned()
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        self.chan.decode(s, &key, &mut msg);
        self.commands_acked = self.commands_acked.max(rack);
        let parsed = self.parser.parse(s, &msg[4..]).unwrap();
        self.ack = s as i32;
        // `CL_ConfigstringModified`: a `d <index> <string>` into the table.
        for op in &parsed.ops {
            if let Op::ServerCommand { text, .. } = op
                && let Some(rest) = text.strip_prefix(b"d ")
                && let Some(gs) = self.parser.gamestate.as_mut()
            {
                let at = rest.iter().position(|&b| b == b' ').unwrap_or(rest.len());
                if let Ok(i) = std::str::from_utf8(&rest[..at])
                    .unwrap_or("")
                    .parse::<u16>()
                {
                    let v = rest.get(at + 1..).unwrap_or(&[]).to_vec();
                    gs.configstrings.insert(i, v);
                }
            }
        }
        // One `cp` a gamestate.
        let feed = self.parser.gamestate.as_ref().map(|g| g.checksum_feed);
        if parsed.ops.iter().any(|o| matches!(o, Op::Gamestate)) && self.cp_feed != feed {
            self.cp_feed = feed;
            self.commands
                .push(vcod_cod4::net::pure::cp_command(&[], feed.unwrap()).into_bytes());
        }
        // A menu answer once its time past the first snapshot has come.
        let newest = self.parser.snapshots.last().map(|s| s.server_time);
        let first = self.parser.snapshots.first().map(|s| s.server_time);
        if let (Some(newest), Some(first)) = (newest, first)
            && let Some(&(at, text)) = self.answers.front()
            && newest >= first + at
        {
            self.answers.pop_front();
            let sid = self
                .parser
                .gamestate
                .as_ref()
                .unwrap()
                .systeminfo("sv_serverid")
                .unwrap();
            self.commands.push(text.replace("{sid}", &sid).into_bytes());
        }
    }
}

/// The walker's `clc_move` cmds after its spawn, each packet's under the
/// server time ours sends it at: the tick before the frame of the first
/// retail snapshot whose `commandTime` reaches the packet's last cmd, so
/// each lands in the snapshot it landed in on retail (the capture logs a
/// packet in the order its client sent it, which is not always before the
/// next frame). A packet sent after its client read snapshot N reached the
/// server after frame N: a spawn's own think sets N's `commandTime` past a
/// cmd the server never ran before it.
pub(super) fn replay_schedule(lines: &[Line], from: i32) -> VecDeque<(i32, Vec<UserCmd>)> {
    replay_schedule_timed(lines, from)
        .into_iter()
        .map(|(t, _, p)| (t, p))
        .collect()
}

/// [`replay_schedule`] with each packet's capture time (`Line::ms`, the
/// probe's own clock).
pub(super) fn replay_schedule_timed(lines: &[Line], from: i32) -> Vec<(i32, u64, Vec<UserCmd>)> {
    let mut s = Session::new();
    let mut packets = Vec::new();
    let mut snaps = Vec::new();
    for l in lines.iter().filter(|l| l.dir != "auth") {
        match s.feed(l) {
            Ok(Some(Item::Client { msg, .. })) => {
                for op in &msg.ops {
                    if let ClOp::Move { cmds, .. } = op {
                        let seen = snaps.last().map_or(i32::MIN, |&(t, _)| t);
                        packets.push((
                            l.ms,
                            seen,
                            cmds.iter().map(|(c, _)| *c).collect::<Vec<_>>(),
                        ));
                    }
                }
            }
            Ok(Some(Item::Server { parsed, .. })) => {
                for op in &parsed.ops {
                    if let Op::Snapshot(i) = op {
                        let sn = &s.client.snapshots[*i];
                        let ct = sn.ps.get("commandTime").unwrap_or(0) as i32;
                        snaps.push((sn.server_time.raw(), ct));
                    }
                }
            }
            _ => {}
        }
    }
    packets
        .into_iter()
        .filter(|(_, _, p)| p.last().is_some_and(|c| c.server_time > from))
        .filter_map(|(ms, seen, p)| {
            let last = p.last()?.server_time;
            let (t, _) = snaps
                .iter()
                .find(|&&(t, ct)| t > from && t > seen && ct >= last)?;
            Some((t - FRAME_MS, ms, p))
        })
        .collect()
}

/// Every `clc_move` cmd's angles in the capture, by the cmd's server time.
pub(super) fn cmd_angles(lines: &[Line]) -> Vec<(i32, [u16; 3])> {
    let mut s = Session::new();
    let mut out = Vec::new();
    for l in lines.iter().filter(|l| l.dir != "auth") {
        if let Ok(Some(Item::Client { msg, .. })) = s.feed(l) {
            for op in &msg.ops {
                if let ClOp::Move { cmds, .. } = op {
                    out.extend(cmds.iter().map(|(c, _)| (c.server_time, c.angles)));
                }
            }
        }
    }
    out
}

/// The HUD elements of every snapshot both sides sent at one server time,
/// the whole run (`HudElem_UpdateClient`, cod4-hud.md 5): the first
/// difference of each element and field, into `out`.
pub(super) fn hud_diffs(tag: &str, r: &Side, o: &Side, out: &mut Vec<String>) {
    let mut seen = std::collections::BTreeSet::new();
    let mut frames = 0;
    for a in &r.snaps {
        let Some(b) = o.snaps.iter().find(|b| b.server_time == a.server_time) else {
            continue;
        };
        frames += 1;
        for (half, x, y) in [
            ("archival", &a.ps.hud_archival, &b.ps.hud_archival),
            ("current", &a.ps.hud_current, &b.ps.hud_current),
        ] {
            for (i, (ex, ey)) in x.iter().zip(y).enumerate() {
                for f in &vcod_cod4::net::fields::HUDELEM {
                    let (v, w) = (ex.0[f.word()], ey.0[f.word()]);
                    if v != w && seen.insert((half, i, f.name)) {
                        out.push(format!(
                            "{tag} hud {half}[{i}] {} at {}: ours {w:#x} retail {v:#x}",
                            f.name, a.server_time
                        ));
                    }
                }
            }
        }
    }
    if frames < r.snaps.len() / 2 {
        out.push(format!("{tag} hud: {frames} frames compared"));
    }
}

/// The run's end: the capture's last snapshot (63350) and a frame.
pub(super) const RUN_MS: i32 = 63_400;

/// The two spawns the replays start after.
pub(super) const TARGET_SPAWN_MS: i32 = 45_950;

pub(super) const SHOOTER_SPAWN_MS: i32 = 46_900;

/// The kill run's `srand` seed (`G_InitGame`, 0x80b56ee), found off its
/// two challenges as `wire_ab::WALK_SEED` was (cod4-combat.md 10.9).
pub(super) const KILL_SEED: u32 = 1_807_258_603;

pub(super) fn kill(name: &str) -> Vec<vcod_cod4::net::capture::Line> {
    retail(&format!("kill-{name}"))
}

/// A hook run before each tick's packets: the server, the probes and the
/// server time.
pub(super) type Hook<'a> = dyn FnMut(&mut Cod4Server, &mut [Probe], i32) + 'a;

/// [`ours_with`] with a [`Hook`]; the server comes back too.
pub(super) fn ours_hooked(
    install: &vcod_cod4::install::Install,
    retail_cs0: &str,
    shooter: Option<VecDeque<(i32, Vec<UserCmd>)>>,
    target_extra: &[(i32, &'static str)],
    hook: &mut Hook,
) -> (Vec<Probe>, Vec<String>, Cod4Server) {
    let classes = ["mr {sid} 15 assault_mp,0"; 2];
    ours_set_up(
        install,
        retail_cs0,
        &[],
        classes,
        shooter,
        target_extra,
        hook,
    )
}

/// [`ours_hooked`] with `cvars` set over the capture's serverinfo and each
/// side's class menu answer (target, shooter).
pub(super) fn ours_set_up(
    install: &vcod_cod4::install::Install,
    retail_cs0: &str,
    cvars: &[(&str, &str)],
    classes: [&'static str; 2],
    shooter: Option<VecDeque<(i32, Vec<UserCmd>)>>,
    target_extra: &[(i32, &'static str)],
    hook: &mut Hook,
) -> (Vec<Probe>, Vec<String>, Cod4Server) {
    // Past the capture's end when a request falls later.
    let run_ms = target_extra
        .iter()
        .map(|a| 43_350 + a.0 + 500)
        .max()
        .unwrap_or(0)
        .max(RUN_MS);
    let mut sv = Cod4Server::new(test_cfg("mp_crash", "war"), 7);
    for (k, v) in vcod_common::net::connectionless::info_pairs(retail_cs0) {
        if !matches!(k, "protocol" | "shortversion") {
            sv.set_cvar(k, v);
        }
    }
    // `+devmap`: cheats on, so `setviewpos` runs.
    sv.set_cvar("sv_cheats", "1");
    for (k, v) in cvars {
        sv.set_cvar(k, v);
    }
    sv.pin_game_seed(KILL_SEED);
    sv.load(Some(install)).unwrap();
    let pin =
        |o: [u32; 3], yaw: u32| Some((o.map(f32::from_bits), [0.0, f32::from_bits(yaw), 0.0]));
    let mut pins = vec![VecDeque::new(); crate::server::MAX_CLIENTS];
    pins[0].push_back(pin([0x442e_4000, 0xc4fe_c000, 0x4308_2af3], 0x42b4_0000));
    pins[1].push_back(pin([0xc454_8000, 0x4509_c000, 0x437e_2000], 0xc2b3_fd30));
    sv.game.level.as_mut().unwrap().rt.host.spawn_pins =
        Some(std::rc::Rc::new(std::cell::RefCell::new(pins)));
    let mut target_answers = vec![
        (1050, "mr {sid} 8 allies"),
        (2550, classes[0]),
        (3050, "setviewpos 207 2291 296"),
    ];
    target_answers.extend_from_slice(target_extra);
    target_answers.sort_by_key(|a| a.0);
    let mut probes = vec![
        Probe::new("target", 0, 28961, 43350, &target_answers),
        Probe::new(
            "shooter",
            1,
            28962,
            44300,
            &[
                (1050, "mr {sid} 8 axis"),
                (2550, classes[1]),
                (3050, "setviewpos 123 2110 300"),
            ],
        ),
    ];
    probes[0].replay = replay_schedule(&kill("target"), TARGET_SPAWN_MS);
    probes[0].replay_from = TARGET_SPAWN_MS;
    probes[1].replay =
        shooter.unwrap_or_else(|| replay_schedule(&kill("shooter"), SHOOTER_SPAWN_MS));
    probes[1].replay_from = SHOOTER_SPAWN_MS;
    run_frames(&mut sv, &mut probes, run_ms, &mut Instant::now(), hook);
    let aborts = sv.game.level.as_ref().unwrap().rt.aborts();
    (probes, aborts, sv)
}

/// The walker's spawn: from its first snapshot past this one on, the walker
/// sends the capture's own cmds.
pub(super) const WALKER_SPAWN_MS: i32 = 47_900;

/// The retail captures' server: `CoD4Host`, 8 slots.
pub(super) fn test_cfg(map: &str, gametype: &str) -> Cod4Config {
    Cod4Config {
        map: map.into(),
        gametype: gametype.into(),
        hostname: "CoD4Host".into(),
        max_clients: 8,
    }
}

/// The frame loop to server time `until` (cod4-server-frame.md 2): each
/// probe whose time has come joins, `hook` runs, each joined probe's
/// packet goes in and is logged, the server runs a frame, and what it sent
/// goes to the probe it was for.
pub(super) fn run_frames(
    sv: &mut Cod4Server,
    probes: &mut [Probe],
    until: i32,
    now: &mut Instant,
    hook: &mut Hook,
) {
    while sv.core.sv_time_ms < until {
        let svs = sv.core.sv_time_ms;
        for p in probes.iter_mut() {
            if !p.joined && svs >= p.connect_at {
                p.join(sv, *now);
            }
        }
        hook(sv, probes, svs);
        for p in probes.iter_mut().filter(|p| p.joined) {
            let pkt = p.packet(svs);
            p.log("c2s", svs, &pkt);
            sv.handle_packet(p.addr, &pkt, *now);
        }
        *now += Duration::from_millis(FRAME_MS as u64);
        sv.tick_frames(*now, 1);
        let svs = sv.core.sv_time_ms;
        for (to, pkt) in sv.take_outgoing() {
            if let Some(p) = probes.iter_mut().find(|p| p.addr == to) {
                p.receive(svs, &pkt);
            }
        }
    }
}
