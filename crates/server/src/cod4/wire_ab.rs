//! The join against retail: `play-1.7-walk.txt` and `play-1.7-spec.txt`
//! (`crates/cod4/tests/fixtures/net/`, cod4-netfields.md 8) are two of three
//! `cod4_probe` clients on one retail `cod4_lnxded` 1.7 `war` level on
//! mp_crash (`+set sv_pure 0`): `mate` (slot 0, `--team allies --class
//! assault_mp,0 --script idle`, not committed), `spec` (slot 1, no team,
//! which answers the team menu `spectator`) and `walker` (slot 2, allies
//! and `assault_mp,0` a second after `mate`). The same three join
//! [`Cod4Server`] in process on retail's level clock, each reading its first
//! snapshot when its retail twin did, answering its menus when its twin did
//! (the mate's times are its probe's rule: the team a second past its first
//! snapshot, the class 1.5 s after) and spawning on its twin's spot; the
//! walker says its line when its twin did. The gamestate's configstrings
//! and baselines, the server commands each client read up to [`UNTIL_MS`],
//! the chat lines each read, and the walker's spawn frame (playerstate and
//! entities) are diffed; what still differs is [`GAPS`], each with the
//! reason. The level's scripts run without an error. Every message of ours
//! decodes and re-encodes ([`super::join_tests::reencodes`]). Needs
//! `COD4_DIR`.
//!
//! [`the_walk_replays_on_ours_as_on_retail`] runs the same join on to the
//! capture's end with the walker sending the capture's own cmds from its
//! spawn, and compares every snapshot's pmove words, the teammate's origin and yaw,
//! the spectator's own playerstate and the walker's body as the spectator
//! is sent it (docs/research/cod4-pmove.md 15.1).

use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use vcod_cod4::net::capture::{self, Item, Line, Session};
use vcod_cod4::net::clmsg::{CmdWire, compress_ops};
use vcod_cod4::net::parse::{Client as Parser, Op, Snapshot};
use vcod_common::net::netchan::{Netchan, scramble_client_ops};

use super::*;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../cod4/tests/fixtures/net");

/// The server time the command comparison stops at: the walker's spawn
/// (47900) and the next four seconds, before its script starts walking.
const UNTIL_MS: i32 = 52_000;

/// The run goes on past the walker's `say` (sent 22.6 s after its first
/// snapshot), whose chat lines are compared alone, to the capture's last
/// snapshot (87300).
const RUN_MS: i32 = 87_350;

/// The walk run's `srand` seed (`G_InitGame`, 0x80b56ee): the wall
/// clock's milliseconds as the server's log file was created, found by
/// `recover_game_seed` (cod4-combat.md 10.9).
pub(super) const WALK_SEED: u32 = 1_803_421_502;

/// The walker's spawn: from its first snapshot past this one on, the walker
/// sends the capture's own cmds.
const WALKER_SPAWN_MS: i32 = 47_900;

/// Each tolerated difference as a pattern over the [`diffs`] line's words
/// (`*` any word, else a prefix of it), and why.
const GAPS: &[(&str, &str)] = &[(
    "walker ps origin[2]",
    "`placeSpawnPoint`'s drop onto mp_crash's terrain stops a bit under retail's fraction: \
         the trace kernels' float model (cod4-pmove.md 13.2, 15)",
)];

/// Whether `diff` is the [`GAPS`] row `pattern`.
fn gap_matches(pattern: &str, diff: &str) -> bool {
    let mut words = diff.split_whitespace();
    pattern
        .split_whitespace()
        .all(|p| words.next().is_some_and(|w| p == "*" || w.starts_with(p)))
}

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
                    Op::Snapshot(i) => last_time = s.client.snapshots[*i].server_time,
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
    ack: i32,
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
    /// A `requeststats` came: the seven stats packets go again
    /// ([`Self::answer_stats`]).
    stats_due: bool,
    /// The checksum feed of the gamestate a `cp` answered.
    cp_feed: Option<i32>,
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
            stats_due: false,
            cp_feed: None,
        }
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
        // The walker replays from its first own spawned snapshot on.
        let spawned = self.parser.snapshots.last().is_some_and(|s| {
            s.ps.get("pm_type") == Some(0) && s.ps.get("clientNum") == Some(self.slot)
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
                ops.push(ClOp::Move {
                    delta: true,
                    cmds: wired,
                    origin: [1.0e6f32.to_bits(); 3],
                    origin_time: 0,
                });
            }
        } else if svs + FRAME_MS >= self.first_snap {
            let last = self.parser.snapshots.last();
            let snap = last.map_or(svs, |s| s.server_time);
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
            let cmd = UserCmd {
                server_time: (snap + 10).max(self.last_cmd.server_time + 1),
                angles: [
                    vcod_cod4::net::conn::angle2short(view[0]).wrapping_sub(delta(0)),
                    vcod_cod4::net::conn::angle2short(view[1]).wrapping_sub(delta(1)),
                    0u16.wrapping_sub(delta(2)),
                ],
                ..self.last_cmd
            };
            let wire = CmdWire::full(&UserCmd::default(), &cmd);
            self.last_cmd = cmd;
            ops.push(ClOp::Move {
                delta: !self.parser.snapshots.is_empty(),
                cmds: vec![(cmd, wire)],
                origin: [1.0e6f32.to_bits(); 3],
                origin_time: 0,
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

/// The three clients against `Cod4Server` until [`UNTIL_MS`].
pub(super) fn ours(
    install: &vcod_cod4::install::Install,
    retail_cs0: &str,
) -> (Vec<Probe>, Vec<String>) {
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "CoD4Host".into(),
            max_clients: 8,
        },
        7,
    );
    // Retail's command line as its serverinfo reflects it.
    for (k, v) in vcod_common::net::connectionless::info_pairs(retail_cs0) {
        if !matches!(k, "protocol" | "shortversion") {
            sv.set_cvar(k, v);
        }
    }
    sv.set_cvar("sv_pure", "0");
    sv.pin_game_seed(WALK_SEED);
    sv.load(Some(install)).unwrap();
    // Each spawn on its twin's spot: the script's pick is `randomInt` off
    // a clock-seeded stream (`Server::pin_spawns`' rule).
    let pin = |o: [f32; 3], yaw: f32| Some((o, [0.0, yaw, 0.0]));
    let mut pins = vec![VecDeque::new(); crate::server::MAX_CLIENTS];
    pins[0].push_back(pin([207.0, 2291.0, 236.125], -135.0));
    pins[2].push_back(pin([-22.0, 1789.0, 224.25427], 45.0));
    sv.game.level.as_mut().unwrap().rt.host.spawn_pins =
        Some(std::rc::Rc::new(std::cell::RefCell::new(pins)));
    // Team menu 8 (`team_marinesopfor`), class menu 15 (`changeclass`).
    let mut probes = vec![
        Probe::new(
            "mate",
            0,
            28961,
            44150,
            &[
                (1150, "mr {sid} 8 allies"),
                (2600, "mr {sid} 15 assault_mp,0"),
            ],
        ),
        Probe::new("spec", 1, 28962, 44200, &[(1150, "mr {sid} 8 spectator")]),
        Probe::new(
            "walker",
            2,
            28963,
            45200,
            &[
                (1150, "mr {sid} 8 allies"),
                (2650, "mr {sid} 15 assault_mp,0"),
                (22600, "say \"vcod probe here\""),
            ],
        ),
    ];
    probes[2].replay = replay_schedule(&retail("walk"), WALKER_SPAWN_MS);
    let mut now = Instant::now();
    while sv.core.sv_time_ms < RUN_MS {
        let svs = sv.core.sv_time_ms;
        for p in probes.iter_mut() {
            if !p.joined && svs >= p.connect_at {
                p.join(&mut sv, now);
            }
        }
        for p in probes.iter_mut().filter(|p| p.joined) {
            let pkt = p.packet(svs);
            p.log("c2s", svs, &pkt);
            sv.handle_packet(p.addr, &pkt, now);
        }
        now += Duration::from_millis(FRAME_MS as u64);
        sv.tick_frames(now, 1);
        let svs = sv.core.sv_time_ms;
        for (to, pkt) in sv.take_outgoing() {
            if let Some(p) = probes.iter_mut().find(|p| p.addr == to) {
                p.receive(svs, &pkt);
            }
        }
    }
    let aborts = sv.game.level.as_ref().unwrap().rt.aborts();
    (probes, aborts)
}

/// The walker's `clc_move` cmds after its spawn, each packet's under the
/// server time ours sends it at: the tick before the frame of the first
/// retail snapshot whose `commandTime` reaches the packet's last cmd, so
/// each lands in the snapshot it landed in on retail (the capture logs a
/// packet in the order its client sent it, which is not always before the
/// next frame).
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
                        packets.push((l.ms, cmds.iter().map(|(c, _)| *c).collect::<Vec<_>>()));
                    }
                }
            }
            Ok(Some(Item::Server { parsed, .. })) => {
                for op in &parsed.ops {
                    if let Op::Snapshot(i) = op {
                        let sn = &s.client.snapshots[*i];
                        let ct = sn.ps.get("commandTime").unwrap_or(0) as i32;
                        snaps.push((sn.server_time, ct));
                    }
                }
            }
            _ => {}
        }
    }
    packets
        .into_iter()
        .filter(|(_, p)| p.last().is_some_and(|c| c.server_time > from))
        .filter_map(|(ms, p)| {
            let last = p.last()?.server_time;
            let (t, _) = snaps.iter().find(|&&(t, ct)| t > from && ct >= last)?;
            Some((t - FRAME_MS, ms, p))
        })
        .collect()
}

/// The first snapshot `slot` reads as its own playing self.
fn spawn_frame(s: &Side, slot: u32) -> Option<&Snapshot> {
    s.snaps
        .iter()
        .find(|f| f.ps.get("pm_type") == Some(0) && f.ps.get("clientNum") == Some(slot))
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

/// Every difference of `pairs` (tag, retail's side, ours), the walker last.
fn diffs(pairs: &[(&str, &Side, &Side)]) -> Vec<String> {
    let mut out = Vec::new();
    let (_, r, o) = pairs[pairs.len() - 1];
    // The gamestate, slot by slot past configstring 1, the walker's.
    let keys: std::collections::BTreeSet<u16> =
        r.gs.configstrings
            .keys()
            .chain(o.gs.configstrings.keys())
            .copied()
            .collect();
    for i in keys.into_iter().filter(|&i| i >= 2) {
        if r.gs.configstring(i) != o.gs.configstring(i) {
            out.push(format!(
                "cs {i}: ours {:?} retail {:?}",
                String::from_utf8_lossy(o.gs.configstring(i)),
                String::from_utf8_lossy(r.gs.configstring(i))
            ));
        }
    }
    // The baselines, number by number and field by field.
    let numbers: std::collections::BTreeSet<u32> =
        r.gs.baselines
            .keys()
            .chain(o.gs.baselines.keys())
            .copied()
            .collect();
    for n in numbers {
        match (r.gs.baselines.get(&n), o.gs.baselines.get(&n)) {
            (Some(a), Some(b)) => {
                for f in vcod_cod4::net::delta::EntityState::table(a.etype()) {
                    if a.0[f.word()] != b.0[f.word()] {
                        out.push(format!(
                            "baseline {n} {}: ours {:#x} retail {:#x}",
                            f.name,
                            b.0[f.word()],
                            a.0[f.word()]
                        ));
                    }
                }
            }
            (a, _) => out.push(format!(
                "baseline {n}: {}",
                if a.is_some() {
                    "retail only"
                } else {
                    "ours only"
                }
            )),
        }
    }
    // The server commands each client read, in order, up to the cut.
    for &(tag, r, o) in pairs {
        let cut = |c: &[(i32, String)]| -> Vec<String> {
            c.iter()
                .filter(|(t, _)| *t < UNTIL_MS)
                .map(|(_, s)| s.clone())
                .collect()
        };
        let (rc, oc) = (cut(&r.commands), cut(&o.commands));
        if std::env::var_os("COD4_WIRE_CMDS").is_some() {
            for i in 0..rc.len().max(oc.len()) {
                eprintln!(
                    "{tag} {i:3} {:60} | {:?}",
                    format!("{:?}", rc.get(i)),
                    oc.get(i)
                );
            }
        }
        for i in 0..rc.len().max(oc.len()) {
            if rc.get(i) != oc.get(i) {
                out.push(format!(
                    "{tag} cmd {i}: ours {:?} retail {:?}",
                    oc.get(i),
                    rc.get(i)
                ));
                break;
            }
        }
    }
    // The chat lines, the whole run.
    for &(tag, r, o) in pairs {
        let chat = |c: &[(i32, String)]| -> Vec<String> {
            c.iter()
                .filter(|(_, s)| s.starts_with("h ") || s.starts_with("i "))
                .map(|(_, s)| s.clone())
                .collect()
        };
        if chat(&r.commands) != chat(&o.commands) {
            out.push(format!(
                "{tag} chat: ours {:?} retail {:?}",
                chat(&o.commands),
                chat(&r.commands)
            ));
        }
    }
    for &(tag, r, o) in pairs {
        hud_diffs(tag, r, o, &mut out);
    }
    // The walker's spawn frame.
    match (spawn_frame(r, 2), spawn_frame(o, 2)) {
        (Some(a), Some(b)) => {
            for f in vcod_cod4::net::fields::PLAYER_STATE {
                if a.ps.words[f.word()] != b.ps.words[f.word()] {
                    out.push(format!(
                        "walker ps {}: ours {:#x} retail {:#x}",
                        f.name,
                        b.ps.words[f.word()],
                        a.ps.words[f.word()]
                    ));
                }
            }
            for (name, x, y) in [
                ("stats", &a.ps.stats[..], &b.ps.stats[..]),
                ("ammo", &a.ps.ammo[..], &b.ps.ammo[..]),
                ("ammoclip", &a.ps.ammoclip[..], &b.ps.ammoclip[..]),
            ] {
                if x != y {
                    out.push(format!("walker ps {name}: ours {y:?} retail {x:?}"));
                }
            }
            // The entities it carries: the teammate's body and the script
            // models, field by field (no PVS cull on ours: the extra
            // entities are one gap).
            for e in &a.entities {
                let Some(f) = b.entities.iter().find(|x| x.number() == e.number()) else {
                    out.push(format!("walker entity {}: retail only", e.number()));
                    continue;
                };
                for t in vcod_cod4::net::delta::EntityState::table(e.etype()) {
                    if e.0[t.word()] != f.0[t.word()] {
                        out.push(format!(
                            "walker entity {} {}: ours {:#x} retail {:#x}",
                            e.number(),
                            t.name,
                            f.0[t.word()],
                            e.0[t.word()]
                        ));
                    }
                }
            }
            let extra = b
                .entities
                .iter()
                .filter(|x| !a.entities.iter().any(|e| e.number() == x.number()))
                .count();
            if extra > 0 {
                out.push(format!("walker entities: {extra} ours only"));
            }
            if a.server_time != b.server_time {
                out.push(format!(
                    "walker spawn time: ours {} retail {}",
                    b.server_time, a.server_time
                ));
            }
        }
        (a, b) => out.push(format!(
            "walker spawn frame: ours {} retail {}",
            b.is_some(),
            a.is_some()
        )),
    }
    out
}

#[test]
fn a_join_on_war_matches_retail() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let (r_spec, r_walk) = (side(&retail("spec")), side(&retail("walk")));
    let cs0 = String::from_utf8_lossy(r_walk.gs.configstring(0)).into_owned();
    let (probes, aborts) = ours(&install, &cs0);
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
    for p in &probes {
        super::join_tests::reencodes(&p.lines);
    }
    let (o_spec, o_walk) = (side(&probes[1].lines), side(&probes[2].lines));
    let mut all = diffs(&[("spec", &r_spec, &o_spec), ("walker", &r_walk, &o_walk)]);
    if std::env::var_os("COD4_WIRE_REPORT").is_some() {
        for (tag, r, o) in [("spec", &r_spec, &o_spec), ("walker", &r_walk, &o_walk)] {
            let first = |s: &Side| s.snaps.first().map(|f| f.server_time);
            eprintln!(
                "{tag} first snapshot: ours {:?} retail {:?}",
                first(o),
                first(r)
            );
        }
        for d in &all {
            eprintln!("{d}");
        }
    }
    // A row no difference matches is a gap closed: it goes.
    let unused: Vec<&str> = GAPS
        .iter()
        .filter(|(g, _)| !all.iter().any(|d| gap_matches(g, d)))
        .map(|(g, _)| *g)
        .collect();
    assert!(unused.is_empty(), "GAPS rows matching nothing: {unused:?}");
    let gap = |d: &str| GAPS.iter().any(|(g, _)| gap_matches(g, d));
    all.retain(|d| !gap(d));
    assert!(
        all.is_empty(),
        "{} differences:\n{}",
        all.len(),
        all.join("\n")
    );
}

/// The walker's playerstate words pmove writes (`cod4_moves_ab`'s list).
const WALK_FIELDS: &[&str] = &[
    "commandTime",
    "pm_type",
    "pm_flags",
    "pm_time",
    "origin[0]",
    "origin[1]",
    "origin[2]",
    "velocity[0]",
    "velocity[1]",
    "velocity[2]",
    "delta_angles[0]",
    "delta_angles[1]",
    "delta_angles[2]",
    "groundEntityNum",
    "jumpTime",
    "jumpOriginZ",
    "movementDir",
    "leanf",
    "viewHeightTarget",
    "viewHeightCurrent",
    "viewHeightLerpTime",
    "viewHeightLerpTarget",
    "viewHeightLerpDown",
    "bobCycle",
    "proneDirection",
    "holdBreathScale",
    "sprintState.lastSprintStart",
    "sprintState.lastSprintEnd",
    "sprintState.sprintStartMaxLength",
    "sprintState.sprintButtonUpRequired",
    "sprintState.sprintDelay",
];

/// `eFlags` bits pmove owns: crouch, prone, talk.
const EFLAGS_PMOVE: u32 = 0x20_000c;

/// What the walk replay still gets wrong, per compared word: (word, rows,
/// why). The rows are exact, so a fix that rights one fails the gate until
/// the entry shrinks.
const WALK_GAPS: &[(&str, usize, &str)] = &[
    // The spawn's drop lands a bit under retail (`placeSpawnPoint`), and
    // the free run's z drifts a bit or two on the terrain as
    // `cod4_moves_ab`'s free run does: the trace kernels' float model.
    ("origin[2]", 374, "cod4-pmove.md 13.2, the float model"),
    (
        "velocity[2]",
        20,
        "cod4-pmove.md 13.2, the sign of a zero vz",
    ),
    // From 70300 the step onto a 0.78 brush slope lands a few ulps off and
    // the run never resyncs: every later row's x keeps it.
    ("origin[0]", 341, "cod4-pmove.md 13.2, the steep step"),
];

/// What the walker's body, as the spectator is sent it, still gets wrong:
/// (word, rows, why).
const BODY_GAPS: &[(&str, usize, &str)] = &[];

/// How far the walker's origin may stray from retail's: the steep step's
/// 0.0083 (cod4-pmove.md 15.1).
const WALK_MAX_ERROR: f32 = 0.01;

/// One walk replay row's differences: the walker's own words, and its
/// teammate's body (entity 0) where both snapshots carry it.
fn walk_row(r: &Snapshot, o: &Snapshot) -> Vec<(String, u32, u32)> {
    let mut out = Vec::new();
    for &n in WALK_FIELDS.iter().chain(&["eFlags"]) {
        let (mut a, mut b) = (o.ps.get(n).unwrap_or(0), r.ps.get(n).unwrap_or(0));
        if n == "eFlags" {
            (a, b) = (a & EFLAGS_PMOVE, b & EFLAGS_PMOVE);
        }
        if a != b {
            out.push((n.to_string(), a, b));
        }
    }
    let mate = |s: &Snapshot| s.entities.iter().find(|e| e.number() == 0).copied();
    if let (Some(a), Some(b)) = (mate(o), mate(r)) {
        for n in [
            "lerp.pos.trBase[0]",
            "lerp.pos.trBase[1]",
            "lerp.pos.trBase[2]",
            "lerp.apos.trBase[1]",
        ] {
            let n = n.to_string();
            let w = vcod_cod4::net::delta::EntityState::table(1)
                .iter()
                .find(|f| f.name == n)
                .unwrap()
                .word();
            if a.0[w] != b.0[w] {
                out.push((format!("mate {n}"), a.0[w], b.0[w]));
            }
        }
    }
    out
}

/// The walk past the spawn: the walker replays the capture's own cmds on
/// ours, and every retail snapshot from the spawn on is compared with ours
/// of the same server time, word for word: the playerstate pmove writes
/// and the teammate's origin and yaw. Ours never resyncs to retail, so a word off
/// once may stay off; [`WALK_GAPS`] names each word's rows.
/// `COD4_WALK_REPORT=1` prints every wrong row.
#[test]
fn the_walk_replays_on_ours_as_on_retail() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let r_walk = side(&retail("walk"));
    let cs0 = String::from_utf8_lossy(r_walk.gs.configstring(0)).into_owned();
    let (probes, aborts) = ours(&install, &cs0);
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
    let o_walk = side(&probes[2].lines);
    let report = std::env::var_os("COD4_WALK_REPORT").is_some();
    let mut rows = 0;
    let mut exact = 0;
    let mut close = 0;
    let mut missing = Vec::new();
    let mut fields: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut max_err = (0f32, 0);
    for r in r_walk
        .snaps
        .iter()
        .filter(|s| s.server_time >= WALKER_SPAWN_MS && s.ps.get("clientNum") == Some(2))
    {
        let Some(o) = o_walk.snaps.iter().find(|s| s.server_time == r.server_time) else {
            missing.push(r.server_time);
            continue;
        };
        rows += 1;
        let d = walk_row(r, o);
        let origin = |s: &Snapshot| {
            glam::Vec3::from_array(
                [0, 1, 2].map(|i| s.ps.get_f32(&format!("origin[{i}]")).unwrap_or(0.0)),
            )
        };
        let err = (origin(o) - origin(r)).length();
        if err > max_err.0 {
            max_err = (err, r.server_time);
        }
        if d.is_empty() {
            exact += 1;
            continue;
        }
        let float_only = d.iter().all(|(n, a, b)| {
            n.starts_with("origin[") || (n == "velocity[2]" && (a | b) & 0x7fff_ffff == 0)
        });
        if float_only && err <= WALK_MAX_ERROR {
            close += 1;
        }
        if report {
            let words: Vec<String> = d
                .iter()
                .map(|(n, a, b)| format!("{n} {a:#x} vs {b:#x}"))
                .collect();
            eprintln!("  {}: {}", r.server_time, words.join(", "));
        }
        for (n, a, b) in d {
            let e = fields.entry(n).or_insert_with(|| {
                (
                    0,
                    format!("first at {}: ours {a:#x} retail {b:#x}", r.server_time),
                )
            });
            e.0 += 1;
        }
    }
    eprintln!(
        "walk: {exact} of {rows} rows exact, {close} more off only in the float model, \
         largest origin error {:.4} at {}",
        max_err.0, max_err.1
    );
    let mut unexplained = Vec::new();
    for (n, (count, first)) in &fields {
        let gap = WALK_GAPS.iter().find(|g| g.0 == n);
        eprintln!(
            "  {n}: {count} rows ({}), {first}",
            gap.map_or("not a gap", |g| g.2)
        );
        if gap.map(|g| g.1) != Some(*count) {
            unexplained.push(format!("{n}: {count} rows"));
        }
    }
    for g in WALK_GAPS {
        if !fields.contains_key(g.0) {
            unexplained.push(format!("{}: gap listed, 0 rows", g.0));
        }
    }
    // The walker's body as the spectator is sent it.
    let (r_spec, o_spec) = (side(&retail("spec")), side(&probes[1].lines));
    let body = |s: &Snapshot| s.entities.iter().find(|e| e.number() == 2).copied();
    let (mut bodies, mut body_exact) = (0, 0);
    let mut body_words: BTreeMap<String, usize> = BTreeMap::new();
    for r in r_spec
        .snaps
        .iter()
        .filter(|s| (WALKER_SPAWN_MS..RUN_MS).contains(&s.server_time))
    {
        let Some(o) = o_spec.snaps.iter().find(|s| s.server_time == r.server_time) else {
            missing.push(r.server_time);
            continue;
        };
        let (Some(a), Some(b)) = (body(o), body(r)) else {
            continue;
        };
        bodies += 1;
        let mut same = true;
        for f in vcod_cod4::net::delta::EntityState::table(1) {
            if a.0[f.word()] != b.0[f.word()] {
                same = false;
                *body_words.entry(f.name.to_string()).or_default() += 1;
                if report && f.name == "lerp.apos.trBase[1]" {
                    let yaw = |e: &EntityState| f32::from_bits(e.0[f.word()]);
                    eprintln!(
                        "  body yaw {}: ours {} retail {}",
                        r.server_time,
                        yaw(&a),
                        yaw(&b)
                    );
                }
            }
        }
        body_exact += usize::from(same);
    }
    // The spectator's own playerstate while it floats (stock
    // `scr_game_spectatetype` 1: `ps.speed` 0, VERIFIED live). Its cmds are
    // the in-process probe's, not the capture's, so `commandTime` is not
    // compared.
    let (mut floats, mut float_exact) = (0, 0);
    for r in r_spec.snaps.iter().filter(|s| {
        (WALKER_SPAWN_MS..RUN_MS).contains(&s.server_time) && s.ps.get("clientNum") == Some(1)
    }) {
        let Some(o) = o_spec.snaps.iter().find(|s| s.server_time == r.server_time) else {
            continue;
        };
        floats += 1;
        let d: Vec<_> = walk_row(r, o)
            .into_iter()
            .filter(|(n, _, _)| !n.starts_with("mate") && n != "commandTime")
            .collect();
        if d.is_empty() {
            float_exact += 1;
        } else if report {
            eprintln!("  spec {}: {d:x?}", r.server_time);
        }
    }
    eprintln!("spectator: {float_exact} of {floats} rows exact");
    if float_exact != floats {
        unexplained.push(format!("spectator: {} rows wrong", floats - float_exact));
    }
    eprintln!("walker's body to the spectator: {body_exact} of {bodies} exact");
    for (n, c) in &body_words {
        let gap = BODY_GAPS.iter().find(|g| g.0 == n);
        eprintln!(
            "  body {n}: {c} rows ({})",
            gap.map_or("not a gap", |g| g.2)
        );
        if gap.map(|g| g.1) != Some(*c) {
            unexplained.push(format!("body {n}: {c} rows"));
        }
    }
    for g in BODY_GAPS {
        if !body_words.contains_key(g.0) {
            unexplained.push(format!("body {}: gap listed, 0 rows", g.0));
        }
    }
    assert!(missing.is_empty(), "no snapshot of ours at {missing:?}");
    assert!(
        max_err.0 <= WALK_MAX_ERROR,
        "origin {} off at {}",
        max_err.0,
        max_err.1
    );
    assert!(unexplained.is_empty(), "{}", unexplained.join("\n"));
}

/// The words of an entity's table, by name, where `a` and `b` differ.
fn entity_diff(a: &EntityState, b: &EntityState) -> Vec<(&'static str, u32, u32)> {
    EntityState::table(b.etype())
        .iter()
        .filter(|f| a.0[f.word()] != b.0[f.word()])
        .map(|f| (f.name, a.0[f.word()], b.0[f.word()]))
        .collect()
}

/// The walker's frag (thrown at 66050, cod4-netfields.md 8) as the walker is
/// sent it: the grenade (`ET_MISSILE`) every snapshot it rides, the burst's
/// event entity (`ET_GENERAL` with `EV_GRENADE_EXPLODE`) and the script's
/// earthquake (17 + 61), each by type, word for word but the entity number
/// (cod4-combat.md 10); what differs is [`FRAG_GAPS`].
/// `COD4_FRAG_REPORT=1` prints every row.
#[test]
fn the_frag_flies_on_ours_as_on_retail() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let r_walk = side(&retail("walk"));
    let cs0 = String::from_utf8_lossy(r_walk.gs.configstring(0)).into_owned();
    let (probes, aborts) = ours(&install, &cs0);
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
    let o_walk = side(&probes[2].lines);
    let report = std::env::var_os("COD4_FRAG_REPORT").is_some();
    type Is = fn(&EntityState) -> bool;
    let kinds: [(&str, Is); 3] = [
        ("missile", |e| e.etype() == 4),
        ("burst", |e| {
            e.etype() == 0
                && e.0[EntityState::table(0)
                    .iter()
                    .find(|f| f.name == "events[0]")
                    .unwrap()
                    .word()]
                    == 45
        }),
        ("quake", |e| e.etype() == 17 + 61),
    ];
    let mut words: BTreeMap<String, usize> = BTreeMap::new();
    let mut rows: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for r in r_walk
        .snaps
        .iter()
        .filter(|s| (65_000..71_000).contains(&s.server_time))
    {
        let o = o_walk.snaps.iter().find(|s| s.server_time == r.server_time);
        for (kind, is) in kinds {
            let a = o.and_then(|o| o.entities.iter().find(|e| is(e)).copied());
            let b = r.entities.iter().find(|e| is(e)).copied();
            let e = rows.entry(kind).or_default();
            match (a, b) {
                (None, None) => continue,
                (Some(a), Some(b)) => {
                    e.0 += 1;
                    let d: Vec<_> = entity_diff(&a, &b)
                        .into_iter()
                        .filter(|(n, _, _)| *n != "number")
                        .collect();
                    if d.is_empty() {
                        e.1 += 1;
                    }
                    if report && !d.is_empty() {
                        let f: Vec<String> = d
                            .iter()
                            .map(|(n, x, y)| format!("{n} {x:#x} vs {y:#x}"))
                            .collect();
                        eprintln!("  {kind} {}: {}", r.server_time, f.join(", "));
                    }
                    for (n, _, _) in d {
                        *words.entry(format!("{kind} {n}")).or_default() += 1;
                    }
                }
                (a, b) => {
                    e.0 += 1;
                    if report {
                        eprintln!(
                            "  {kind} {}: ours {} retail {}",
                            r.server_time,
                            a.is_some(),
                            b.is_some()
                        );
                    }
                    *words.entry(format!("{kind} present")).or_default() += 1;
                }
            }
        }
    }
    for (k, (n, exact)) in &rows {
        eprintln!("{k}: {exact} of {n} rows exact");
    }
    let mut unexplained = Vec::new();
    for (n, c) in &words {
        let gap = FRAG_GAPS.iter().find(|g| g.0 == n);
        eprintln!("  {n}: {c} rows ({})", gap.map_or("not a gap", |g| g.2));
        if gap.map(|g| g.1) != Some(*c) {
            unexplained.push(format!("{n}: {c} rows"));
        }
    }
    for g in FRAG_GAPS {
        if !words.contains_key(g.0) {
            unexplained.push(format!("{}: gap listed, 0 rows", g.0));
        }
    }
    assert!(unexplained.is_empty(), "{}", unexplained.join("\n"));
}

/// What the frag replay still gets wrong: (kind and word, rows, why).
const FRAG_GAPS: &[(&str, usize, &str)] = &[];

/// A capture's `challengeResponse`.
fn challenge_of(lines: &[Line]) -> Option<i32> {
    lines.iter().filter(|l| l.dir == "s2c").find_map(|l| {
        let text = String::from_utf8_lossy(l.data.get(4..)?);
        text.strip_prefix("challengeResponse ")?.trim().parse().ok()
    })
}

/// The challenges come off the level's `rand()` (cod4-combat.md 10.9): on
/// [`WALK_SEED`], after the map's pickups drew 17, the spectator's and the
/// walker's draws are retail's. A challenge is `(rand() << 16) ^ rand() ^
/// svs.time`, and the in-process probes ask theirs at another `svs.time`
/// (under 2^17 on both), so the bits above it are compared.
#[test]
fn the_challenges_come_off_the_levels_rand() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let r_walk = retail("walk");
    let cs0 = String::from_utf8_lossy(side(&r_walk).gs.configstring(0)).into_owned();
    let (probes, _) = ours(&install, &cs0);
    for (probe, name) in [(&probes[1], "spec"), (&probes[2], "walk")] {
        let (o, r) = (challenge_of(&probe.lines), challenge_of(&retail(name)));
        assert_eq!(
            o.map(|c| c >> 17),
            r.map(|c| c >> 17),
            "{name}: {o:?} {r:?}"
        );
    }
}

/// Recovers a run's `srand` seed: retail seeds `G_InitGame`'s with the
/// wall clock's milliseconds (`Sys_MillisecondsRaw`, 0x81d67ac), so the
/// search walks `COD4_SEED_FROM..COD4_SEED_TO` (Unix ms, around the run's
/// `games_mp.log`, which `G_InitGame` opens) for a stream whose first 64
/// draws give every capture in `COD4_SEED_CAPTURES` (default `walk,spec`)
/// its challenge with a `svs.time` under 2^18. Run by hand (`cargo test -p
/// vcod-server --lib -- --ignored recover_game_seed --nocapture`); it finds
/// [`WALK_SEED`] alone, and with `kill-target,kill-shooter` `combat_ab`'s
/// `KILL_SEED`.
#[test]
#[ignore]
fn recover_game_seed() {
    let ms = |k: &str| -> u64 { std::env::var(k).expect(k).parse().unwrap() };
    let (from, to) = (ms("COD4_SEED_FROM"), ms("COD4_SEED_TO"));
    let names = std::env::var("COD4_SEED_CAPTURES").unwrap_or("walk,spec".into());
    let challenges: Vec<u32> = names
        .split(',')
        .map(|n| challenge_of(&retail(n)).expect("a challenge") as u32)
        .collect();
    for t in from..to {
        let mut g = vcod_common::rng::GlibcRand::new(t as u32);
        let d: Vec<u32> = (0..65).map(|_| g.next_u31()).collect();
        let found: Vec<(usize, u32)> = challenges
            .iter()
            .filter_map(|&c| {
                (0..64)
                    .map(|k| (k, (d[k] << 16) ^ d[k + 1] ^ c))
                    .find(|&(_, svs)| svs < 1 << 18)
            })
            .collect();
        if found.len() == challenges.len() {
            println!("ms {t} seed {} (draw, svs.time) {found:?}", t as u32);
        }
    }
}
