//! Retrieval's pickup, drop, return and capture and Behind Enemy Lines' team
//! swap, against the retail captures in `tests/fixtures/gametypes/`, and
//! United Offensive's Capture the Flag pickup and capture, Domination flag
//! capture and Headquarters radio capture and neutralize against the UO
//! 1.51b ones (`*-uo.txt`, `client-probes/probe_ctf`, `probe_dom`,
//! `probe_hq`; docs/research/coduo-diff.md 22), and each of the four UO
//! modes played to its map end and the restart after it (`probe_ctfend`,
//! `probe_domend`, `probe_hqend`, `probe_basend`; same section, "Round and
//! map ends"), and the clock ends: `ctf`'s round clock and time limit with
//! a defender's flag return and a rotation to mp_foy (`probe_ctftime`),
//! `tdm`'s killcam and time limit (`probe_tdmend`), `doDamage` on a client
//! and the killcam skipped by use (`probe_tdmskip`), `hq`'s time limit
//! (`probe_hqtime`), `bas`'s round clock (`probe_bastime`) and `dom`'s
//! round clock and time limit (`probe_domtime`).
//!
//! Retail seeds the script's draws from the wall clock at every level
//! (cod11-map-cycle.md 4.8), so ours runs a seed stepped per level too and
//! replays retail's spawn picks off the captures ([`retail_spawns`]); no
//! view may depend on ours' own draws.
//!
//! Each run is stock `re.gsc` or `bel.gsc` under a gsc probe
//! (`client-probes/probe_re.gsc`, `probe_bel.gsc`) that places the players,
//! kills them and tells each client when to press use, and two clients
//! recording through `vcod_common::net::capture::ScriptedCapture`. The
//! retail fixtures are that recorder on `vcod --net-probe --save-scripted`;
//! here it rides our own [`NetClient`]s on ours, with the same probe as the
//! gametype, and the two recordings are compared view by view. The
//! measurements are written up in `docs/research/cod11-gametypes-re-bel.md`,
//! sections 7 and 8.
//!
//! A view is one thing the client was told, as the ordered list of its
//! distinct values: the bold announcements, the sounds, the HUD, the
//! objective block, the other player's head icon, the scoreboard rows, and
//! the probe's own `logPrint` lines off each server's log. The list drops
//! repeats, so a value that holds for a frame longer on one server is not a
//! difference; one that never appears, or appears out of order, is.
//!
//! `GAMETYPES_REPORT=1` prints every view of both sides;
//! `GAMETYPES_DUMP=<dir>` writes our two recordings and our log there, in
//! the fixtures' own format.
//!
//! Needs `COD_DIR`; without the paks the tests return early. The 1.1 ones
//! run only at a 1.1 install (`testing::game_fs_11`), the UO ones only at a
//! UO install (`testing::uo_fs`).

use crate::common;

use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;
use std::time::{Duration, Instant};

use common::{ADDR, ADDR_B, ClientEnd, Queues};
use vcod_common::net::capture::ScriptedCapture;
use vcod_common::net::protocol::{CsRange, PROTOCOL_V22};
use vcod_common::net::{NetClient, NetEvent};
use vcod_server::Server;

const FIXTURES: &str = "tests/fixtures/gametypes";
const PROBES: &str = "../gsc/tests/fixtures/semantics/client-probes";

/// One run's map, install and join times.
struct Setup {
    map: &'static str,
    uo: bool,
    /// When each probe connects, in server ms. 1.1: retail had been up 4 s
    /// when the first probe connected (the recipe starts the probes 4 s
    /// after the server), and retail's first probe sees the second in its
    /// roster 2.4 s into its capture (3 s between the launches, less the
    /// first one's own connect). UO: a probe's first connect is refused as
    /// 1.1 and retried as protocol 22, the first probe's first snapshot
    /// reads about 6.5 s of server time and the second's (which reaches
    /// the server more directly) about 7 s.
    joins: [u64; 2],
}

const V1: Setup = Setup {
    map: "mp_brecourt",
    uo: false,
    joins: [4000, 6400],
};

const UO: Setup = Setup {
    map: "mp_cassino",
    uo: true,
    joins: [6000, 6600],
};

/// Base Assault's first stock map; its tanks are not needed for a breach.
const UO_FOY: Setup = Setup {
    map: "mp_foy",
    ..UO
};

impl Setup {
    /// The fixture name suffix a UO capture carries (`--capture-tag uo`).
    fn tag(&self) -> &'static str {
        if self.uo { "-uo" } else { "" }
    }
}
const FRAME_MS: u64 = 50;
/// The probes ask for the scoreboard this often.
const SCORE_PERIOD_MS: i64 = 2000;

/// Views a run is known to differ from retail in, as `(gametype, role, view,
/// why)`. An entry suppresses that view's comparison and asserts it still
/// differs, so a gap that starts matching fails and has to be deleted.
const GAPS: &[(&str, &str, &str, &str)] = &[];

fn report() -> bool {
    std::env::var("GAMETYPES_REPORT").is_ok_and(|v| v == "1")
}

/// What `JoinProbe::menu_reply` in the client answers: the team menu with the
/// team, UO's `serverinfo_*` with `close`, a single-nation weapon menu with
/// that nation's default rifle, and nothing else (bel's two-nation menu
/// stays open on both).
fn menu_reply(menu: &str, team: &str) -> Option<String> {
    if menu.starts_with("team_") {
        return Some(team.to_string());
    }
    if menu.starts_with("serverinfo_") {
        return Some("close".to_string());
    }
    Some(
        match menu.strip_prefix("weapon_")? {
            "american" => "m1carbine_mp",
            "british" => "enfield_mp",
            "russian" => "mosin_nagant_mp",
            "german" => "kar98k_mp",
            _ => return None,
        }
        .to_string(),
    )
}

/// One recording client: the netchan, the menu answers and the capture.
struct Probe {
    addr: SocketAddr,
    q: Rc<RefCell<Queues>>,
    cl: NetClient<ClientEnd>,
    team: &'static str,
    main_menu: String,
    answered: Vec<i32>,
    cap: ScriptedCapture,
    start: Instant,
    seen: Option<u32>,
    next_score: i64,
    /// The net probe's cmd cadence: about three a frame (`ProbeHold::framed`)
    /// at the newest snapshot's view (`ProbeHold::aim`), as `ScriptedCapture::cmd`
    /// aims them.
    hold: common::ProbeHold,
}

impl Probe {
    fn new(addr: SocketAddr, qport: u16, team: &'static str, now: Instant) -> Probe {
        let q = Rc::new(RefCell::new(Queues::default()));
        let cl = NetClient::start_with_qport(ClientEnd(q.clone()), now, qport);
        Probe {
            addr,
            q,
            cl,
            team,
            main_menu: String::new(),
            answered: Vec::new(),
            cap: ScriptedCapture::default(),
            start: now,
            seen: None,
            next_score: 0,
            hold: Self::hold(),
        }
    }

    fn hold() -> common::ProbeHold {
        common::ProbeHold {
            aim: true,
            framed: true,
            ..Default::default()
        }
    }

    fn ms(&self, now: Instant) -> i64 {
        now.duration_since(self.start).as_millis() as i64
    }

    fn send(&mut self, now: Instant) {
        let ms = self.ms(now);
        if self.cl.state() == vcod_common::net::NetState::Active && ms >= self.next_score {
            self.cl.send_reliable("score");
            self.next_score = ms + SCORE_PERIOD_MS;
        }
        let cmd = self.cap.cmd(ms, self.cl.snapshots().newest());
        if self.cl.snapshots().newest().is_some() {
            let cmds = self.hold.cmds(&self.cl, cmd.buttons);
            self.cl.send_cmds(&cmds);
        } else if let Some(c) = self.cl.send_frame(&cmd) {
            self.hold.last = c.server_time;
        }
    }

    fn receive(&mut self, now: Instant) {
        let ms = self.ms(now);
        for e in self.cl.pump_at(now) {
            match e {
                NetEvent::GamestateReady => {
                    self.hold = Self::hold();
                    self.main_menu.clear();
                    self.answered.clear();
                    let map =
                        vcod_common::net::info_value_for_key(self.cl.configstring(0), "mapname")
                            .unwrap_or("?")
                            .to_string();
                    self.cap.protocol = self.cl.protocol();
                    self.cap.on_gamestate(ms, self.cl.server_id(), &map);
                }
                NetEvent::ServerCommand(t) => self.on_command(&t),
                NetEvent::Dropped(r) => panic!("{} dropped at {ms} ms: {r}", self.team),
                _ => {}
            }
        }
        let cmds = self.cl.take_server_commands();
        self.cap.on_commands(ms, &cmds, self.cl.configstrings());
        if let Some(s) = self.cl.snapshots().newest()
            && self.seen != Some(s.message_num)
        {
            self.seen = Some(s.message_num);
            self.cap.sample(ms, s, self.cl.configstrings());
        }
    }

    fn on_command(&mut self, tokens: &[String]) {
        match tokens.first().map(String::as_str) {
            // A restart that drops `pers[]` sends `B` on UO (coduo-diff.md 22).
            Some("n" | "B") => {
                self.main_menu.clear();
                self.answered.clear();
            }
            Some("v") if tokens.get(1).map(String::as_str) == Some("g_scriptMainMenu") => {
                self.main_menu = tokens.get(2).cloned().unwrap_or_default();
            }
            Some("t") => {
                let Some(idx) = tokens.get(1).and_then(|t| t.parse::<i32>().ok()) else {
                    return;
                };
                if self.answered.contains(&idx) {
                    return;
                }
                // `t` names its menu by script-menu index, as the client's
                // `JoinProbe` reads it: UO opens the weapon menu before
                // naming it in `g_scriptMainMenu`.
                let first = self.cl.protocol().cs.bounds(CsRange::Menu).0;
                let menu = usize::try_from(idx)
                    .ok()
                    .map(|i| self.cl.configstring(first + i))
                    .filter(|m| !m.is_empty())
                    .unwrap_or(&self.main_menu)
                    .to_string();
                if let Some(reply) = menu_reply(&menu, self.team) {
                    let id = self.cl.server_id();
                    self.cl.send_reliable(&format!("mr {id} {idx} {reply}"));
                    self.answered.push(idx);
                }
            }
            _ => {}
        }
    }
}

/// Runs `probe` as the gametype on ours for `secs` of server time with two
/// recording clients joining as retail's did, and hands back both
/// recordings and the probe's log lines.
fn run_ours(
    setup: &Setup,
    probe: &str,
    teams: [&'static str; 2],
    secs: u64,
    clock: &RetailClock,
    roles: [&str; 2],
) -> Option<([Vec<String>; 2], Vec<String>)> {
    let fs = if setup.uo {
        vcod_common::testing::uo_fs()?
    } else {
        vcod_common::testing::game_fs_11()?
    };
    let map = setup.map;
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    // A seed stepped at every load, as retail's clock-seeded `G_InitGame`
    // draws are (map-cycle doc 4.8); the spawn picks they feed are retail's
    // own, replayed.
    let mut sv = Server::with_seed(common::cfg(map, probe), now, SEED);
    sv.set_cvar("sv_pure", "0");
    let gametype = probe.strip_prefix("probe_").unwrap_or(probe);
    sv.pin_spawns(
        roles
            .iter()
            .enumerate()
            .map(|(slot, r)| retail_spawns(setup, gametype, r, slot, &fs))
            .collect(),
    );
    if setup.uo {
        sv.set_protocol(&PROTOCOL_V22);
    }
    let src = std::fs::read_to_string(format!("{PROBES}/{probe}.gsc")).expect("the probe");
    sv.overlay_script(&format!("maps/mp/gametypes/{probe}"), &src);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");

    let start = now;
    let mut probes: Vec<Probe> = Vec::new();
    let mut log: Vec<String> = Vec::new();
    let mut drained = 0usize;
    let joins = setup.joins;
    let frames = (secs * 1000 + joins[1]) / FRAME_MS;
    let mut server_id = sv.server_id();
    // Ours' level count and level start, the anchor a retail stall's time is
    // read against.
    let (mut level, mut level_start) = (0usize, 0i32);
    let mut stalls = clock.stalls.iter().peekable();
    for _ in 0..frames {
        now += Duration::from_millis(FRAME_MS);
        let t = now.duration_since(start).as_millis() as u64;
        for (i, (at, addr)) in joins.iter().zip([ADDR, ADDR_B]).enumerate() {
            if probes.len() == i && t >= *at {
                probes.push(Probe::new(addr, 0x2001 + i as u16, teams[i], now));
            }
        }
        for p in probes.iter_mut() {
            p.send(now);
            let pending: Vec<Vec<u8>> = p.q.borrow_mut().to_server.drain(..).collect();
            for pkt in pending {
                sv.handle_packet(p.addr, &pkt, now);
            }
        }
        // Retail's host time, replayed ([`RetailClock`]): the console runs
        // first, and the frames a restart or a stall owes run back to back
        // before one snapshot.
        sv.drain_console(now);
        let mut owed = 0u64;
        if sv.server_id() != server_id {
            if sv.server_id() & 0xf0 == server_id & 0xf0 {
                owed = clock.restarts.get(level).copied().unwrap_or(0);
                level += 1;
                level_start = sv
                    .configstring(if setup.uo { 14 } else { 13 })
                    .parse()
                    .expect("the level start");
            }
            server_id = sv.server_id();
        } else if let Some(&&(at_level, after, ms)) = stalls.peek()
            && at_level == level
            && i64::from(sv.game_time_ms() - level_start) >= after
        {
            stalls.next();
            owed = ms;
        }
        now += Duration::from_millis(owed);
        sv.tick_frames(now, 1 + (owed / FRAME_MS) as u32);
        for (to, pkt) in sv.take_outgoing() {
            if let Some(p) = probes.iter().find(|p| p.addr == to) {
                p.q.borrow_mut().to_client.push_back(pkt);
            }
        }
        for p in probes.iter_mut() {
            p.receive(now);
        }
        // A restart starts a new log.
        let lines = sv.script_log();
        if lines.len() < drained {
            drained = 0;
        }
        log.extend(lines[drained..].iter().cloned());
        drained = lines.len();
    }
    assert_eq!(
        sv.script_aborts(),
        Vec::<String>::new(),
        "{probe} aborted on ours"
    );
    let mut it = probes.into_iter().map(|p| p.cap.lines);
    Some(([it.next().unwrap(), it.next().unwrap()], log))
}

/// The gate's server seed. Nothing compared may depend on it: every draw a
/// view reads is either replayed from retail ([`retail_spawns`]) or dropped
/// from the view (hq's radio origin).
const SEED: u64 = 0x5eed_d0c1;

/// A spawn point's `(origin, angles)`, `None` for ours' own pick.
type SpawnPin = Option<([f32; 3], [f32; 3])>;

/// Every gametype's spawn points on `map`, each as `placeSpawnpoint` drops
/// it (`placed_origin`, the server's own drop under the install's build),
/// beside its entity-lump origin and angles.
fn placed_points(
    fs: &vcod_common::pk3::Pk3Fs,
    map: &str,
    uo: bool,
) -> Vec<([f32; 3], [f32; 3], [f32; 3])> {
    let path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&path).unwrap()).unwrap();
    let world = vcod_server::world::World::from_bsp(&bsp, Some(fs));
    let proto = if uo {
        &PROTOCOL_V22
    } else {
        &vcod_common::net::protocol::PROTOCOL_V1
    };
    world
        .collision
        .set_arith(vcod_common::patch::Arith::for_protocol(proto));
    common::spawn_points(fs, map)
        .into_iter()
        .map(|(origin, angles)| {
            let (placed, _) =
                vcod_server::game::builtins::entity::placed_origin(&world.collision, origin);
            (placed, origin, angles)
        })
        .collect()
}

/// The point a body at `at` stands on, placed: one within two units across
/// and four up or down of where it dropped. Two gametypes' points can share
/// a spot at different heights and drop to the same floor; the lowest one
/// in the lump is taken, as a pick off the lump's origins was.
fn point_under(points: &[([f32; 3], [f32; 3], [f32; 3])], at: [f32; 3]) -> SpawnPin {
    points
        .iter()
        .filter(|(p, ..)| (p[0] - at[0]).hypot(p[1] - at[1]) < 2.0 && (p[2] - at[2]).abs() < 4.0)
        .min_by(|a, b| a.1[2].total_cmp(&b.1[2]))
        .map(|&(placed, _, angles)| (placed, angles))
}

/// `(x, y, z)` as gsc prints a vector, or a trace's `x,y,z`.
fn parse_vec(v: &str) -> [f32; 3] {
    let v: Vec<f32> = v
        .trim_matches(|c| c == '(' || c == ')')
        .split(',')
        .map(|t| t.trim().parse().expect("a coordinate"))
        .collect();
    [v[0], v[1], v[2]]
}

/// Where retail spawned a role's player (client `slot`) into play, each
/// time, in order. Retail's script draws come from a wall-clock seed
/// (map-cycle doc 4.8), so ours replays the picks rather than its own
/// draws, each at the point as `placeSpawnpoint` dropped it.
///
/// A probe that moves every spawn before a snapshot can show it
/// (`probe_bel`) logs each pick itself, `PROBE spawned <slot> <team> <time>
/// <origin>`, and those lines are the picks. Otherwise each life's pick is
/// the point under its first playing snapshot (`pm_type` 0, or 1 linked)
/// after a non-playing one, a gamestate, a restart or a killcam; a
/// killcam's run, whose playerstate is the killer's archived one, is not a
/// life. Every pick has to resolve: one that does not is a spawn retail
/// never showed, which this gate cannot replay.
fn retail_spawns(
    setup: &Setup,
    gametype: &str,
    role: &str,
    slot: usize,
    fs: &vcod_common::pk3::Pk3Fs,
) -> Vec<SpawnPin> {
    let fixture = |what: &str| {
        read_lines(&format!(
            "{FIXTURES}/{}-{gametype}-{what}{}.txt",
            setup.map,
            setup.tag()
        ))
    };
    let logged: Vec<[f32; 3]> = fixture("log")
        .iter()
        .filter_map(|l| {
            let rest = l.strip_prefix("PROBE spawned ")?;
            let mut w = rest.splitn(4, ' ');
            (w.next()?.parse::<usize>().ok()? == slot).then_some(())?;
            Some(parse_vec(w.nth(2)?))
        })
        .collect();
    if !logged.is_empty() {
        let points = placed_points(fs, setup.map, setup.uo);
        return logged
            .into_iter()
            .map(|at| {
                let pin = point_under(&points, at);
                assert!(
                    pin.is_some(),
                    "{gametype} {role}: no spawn point under {at:?}"
                );
                pin
            })
            .collect();
    }
    let lines = fixture(role);
    // Each life: its first playing trace's spawn point, and whether any of
    // its traces is a killcam.
    let mut lives: Vec<(SpawnPin, bool, String)> = Vec::new();
    let mut points = Vec::new();
    let mut playing = false;
    for line in &lines {
        if line.starts_with("!gamestate") {
            let map = field(line, "mapname").expect("a mapname");
            points = placed_points(fs, map, setup.uo);
        }
        if line.starts_with("!gamestate") || matches!(cmd_text(line), Some("n" | "B")) {
            playing = false;
            continue;
        }
        if !line.starts_with("!trace ") {
            continue;
        }
        let now = matches!(field(line, "pm_type"), Some("0" | "1"));
        // A killcam's first snapshot can come before its HUD does: the
        // run it opened is marked, and the snapshot after the killcam
        // starts the next life.
        if now && field(line, "hud").is_some_and(|h| h.contains("MPSCRIPT_KILLCAM")) {
            if playing {
                lives.last_mut().expect("a life").1 = true;
            }
            playing = false;
            continue;
        }
        if now && !playing {
            let at = parse_vec(field(line, "origin").expect("a trace origin"));
            lives.push((point_under(&points, at), false, line.clone()));
        }
        playing = now;
    }
    lives
        .into_iter()
        .filter(|(_, killcam, _)| !killcam)
        .map(|(pick, _, line)| {
            assert!(
                pick.is_some(),
                "{gametype} {role}: no spawn point under {line}"
            );
            pick
        })
        .collect()
}

/// Retail's host time, read off a role's capture, for ours to replay in its
/// simulated clock. Retail's frame loop catches up the frames a slow
/// `Cbuf_Execute` owes and sends one snapshot after them (cod11-map-cycle.md
/// 4.7), so how long its host took shows in which snapshots a client got.
struct RetailClock {
    /// Each restart's time past a restart that owes no frame: the first
    /// snapshot after the burst's level start (configstring 13, UO's 14)
    /// less that start and the 350 ms of the settle frames and one frame.
    restarts: Vec<u64>,
    /// Each stall inside a level: the level's index (restarts before it),
    /// the last snapshot before it as ms past that level's start, and the
    /// ms owed past one frame. A stall is two consecutive snapshots more
    /// than 1100 ms apart, which the capture's one trace a second rules out
    /// otherwise.
    stalls: Vec<(usize, i64, u64)>,
}

fn retail_clock(setup: &Setup, gametype: &str, role: &str) -> RetailClock {
    let slot = if setup.uo { 14 } else { 13 };
    let lines = read_lines(&format!(
        "{FIXTURES}/{}-{gametype}-{role}{}.txt",
        setup.map,
        setup.tag()
    ));
    let prefix = format!("d {slot} ");
    let mut clock = RetailClock {
        restarts: Vec::new(),
        stalls: Vec::new(),
    };
    let mut start = 0i64;
    let mut restarted = false;
    let mut last: Option<i64> = None;
    for line in &lines {
        if let Some(t) = cmd_text(line).and_then(|c| c.strip_prefix(&prefix)) {
            start = t.parse().expect("a level start");
            restarted = true;
        } else if line.starts_with("!trace ")
            && let Some(t) = field(line, "serverTime")
        {
            let at: i64 = t.parse().expect("a serverTime");
            if std::mem::take(&mut restarted) {
                clock
                    .restarts
                    .push(u64::try_from(at - start - 350).unwrap_or(0));
            } else if let Some(prev) = last
                && at - prev > 1100
            {
                let level = clock.restarts.len();
                clock
                    .stalls
                    .push((level, prev - start, (at - prev - 50) as u64));
            }
            last = Some(at);
        }
    }
    clock
}

// ------------------------------------------------------------------ views

/// `key=value` off a `!trace` line; a value never holds a space.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split(' ')
        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

fn ms_of(line: &str) -> i64 {
    field(line, "ms").and_then(|v| v.parse().ok()).unwrap_or(0)
}

fn cmd_text(line: &str) -> Option<&str> {
    line.strip_prefix("!cmd ")?
        .split_once(" text=")
        .map(|(_, t)| t)
}

/// The lines up to the first teardown: a probe's own disconnect is the run
/// ending, not the gametype.
fn until_teardown(lines: &[String], end_ms: i64) -> Vec<String> {
    lines
        .iter()
        .take_while(|l| !cmd_text(l).is_some_and(|t| t.contains("MPSCRIPT_DISCONNECTED")))
        .filter(|l| ms_of(l) <= end_ms)
        .cloned()
        .collect()
}

fn dedup(v: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in v {
        if out.last() != Some(&s) {
            out.push(s);
        }
    }
    out
}

/// The `ents` list's entries. A script model's origin is itself
/// comma-separated, so a piece that does not open with `<num>:<kind>:` is
/// the tail of the entry before it.
fn split_ents(list: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for piece in list.split(',').filter(|p| *p != "-") {
        let mut w = piece.split(':');
        let opens = w.next().is_some_and(|n| n.parse::<u32>().is_ok())
            && w.next().is_some_and(|k| k.len() == 1);
        match out.last_mut() {
            Some(last) if !opens => {
                last.push(',');
                last.push_str(piece);
            }
            _ => out.push(piece.to_string()),
        }
    }
    out
}

/// Each roster entry's own deduped history, client by client.
fn per_client(rosters: Vec<&str>) -> Vec<String> {
    let mut by_num: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for r in rosters {
        for e in r.split(',').filter(|e| *e != "-") {
            let num = e.split(':').next().unwrap_or("").to_string();
            let h = by_num.entry(num).or_default();
            if h.last().map(String::as_str) != Some(e) {
                h.push(e.to_string());
            }
        }
    }
    by_num.into_values().flatten().collect()
}

fn first_seen(v: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in v {
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// The progress bar's width is the frame it was sampled on.
fn normalize_hud(h: &str) -> String {
    h.split(';')
        .map(|e| {
            let parts: Vec<&str> = e.split(':').collect();
            if parts.len() > 6 && parts[5] == "white" {
                format!("{}:{}:*", parts[..5].join(":"), parts[5])
            } else {
                e.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// An objective slot without its origin: hq's radio is a random pick of the
/// map's seven, and bel's compass marker has its own view (`marker`).
fn objectives_without_origin(o: &str) -> String {
    o.split(',')
        .filter(|w| w.contains(':'))
        .map(|w| w.rsplit_once(':').map_or(w, |(head, _)| head))
        .collect::<Vec<_>>()
        .join(",")
}

/// An objective list with each slot's entity number other than
/// `ENTITYNUM_NONE` as `E`: vcod spawns none of a UO map's vehicles, which
/// take entity numbers ahead of the gametype's objects (coduo-diff.md 20).
fn objectives_without_entnum(o: &str) -> String {
    o.split(',')
        .map(|w| {
            let mut f: Vec<&str> = w.split(':').collect();
            if f.len() >= 5 && f[3] != "1023" {
                f[3] = "E";
            }
            f.join(":")
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Every view of one recording, by name. `bel` and `hq` drop the objective
/// origins (bel's go to `marker`) ([`objectives_without_origin`]); a UO run drops the entity
/// numbers its unspawned vehicles shift ([`objectives_without_entnum`]).
fn views(lines: &[String], gametype: &str, uo: bool) -> Vec<(&'static str, Vec<String>)> {
    let cmds = || lines.iter().filter_map(|l| cmd_text(l));
    let traces = || lines.iter().filter(|l| l.starts_with("!trace"));
    let tf = |k: &'static str| dedup(traces().filter_map(move |l| field(l, k).map(str::to_string)));
    let ents = |kind: &'static str| {
        traces().flat_map(move |l| {
            split_ents(field(l, "ents").unwrap_or("-"))
                .into_iter()
                .filter(|e| e.split(':').nth(1) == Some(kind))
                .collect::<Vec<_>>()
        })
    };
    vec![
        (
            "announcements",
            cmds()
                .filter(|t| t.starts_with("c "))
                .map(str::to_string)
                .collect(),
        ),
        (
            "prints",
            sort_join_runs(
                cmds()
                    .filter(|t| t.starts_with("f "))
                    .map(str::to_string)
                    .collect(),
            ),
        ),
        (
            "cvars",
            cmds()
                .filter(|t| t.starts_with("v ") && !t.starts_with("v probe_use"))
                .map(str::to_string)
                .collect(),
        ),
        (
            "sounds",
            lines
                .iter()
                .filter_map(|l| l.strip_prefix("!sound "))
                .filter_map(|l| field(l, "alias").map(str::to_string))
                .collect(),
        ),
        (
            "teamscores",
            cmds()
                .filter(|t| t.starts_with("d 5 ") || t.starts_with("d 6 "))
                .map(str::to_string)
                .collect(),
        ),
        // The last one only: the probe asks every 2 s, so whether a score
        // that held for under that is seen is the asking phase. The log's
        // state lines carry every score change.
        (
            "scoreboard",
            lines
                .iter()
                .rev()
                .filter_map(|l| cmd_text(l))
                .find(|t| t.starts_with("b "))
                .map(scoreboard_rows)
                .into_iter()
                .collect(),
        ),
        (
            "vitals",
            dedup(traces().map(|l| {
                ["pm_type", "health", "weapon", "ammoclip"]
                    .iter()
                    .map(|k| format!("{k}={}", field(l, k).unwrap_or("?")))
                    .collect::<Vec<_>>()
                    .join(" ")
            })),
        ),
        (
            "hud",
            dedup(traces().filter_map(|l| field(l, "hud").map(normalize_hud))),
        ),
        (
            "objectives",
            dedup(tf("objectives").iter().map(|o| {
                let o = if uo {
                    objectives_without_entnum(o)
                } else {
                    o.clone()
                };
                if gametype == "bel" || gametype.starts_with("hq") {
                    objectives_without_origin(&o)
                } else {
                    o
                }
            })),
        ),
        // bel's compass marker with its origin: where the replayed spawn
        // pick put the allied player, then a running mean of where he stood.
        (
            "marker",
            if gametype == "bel" {
                tf("objectives")
            } else {
                Vec::new()
            },
        ),
        // Per client: when the second one shows up against the first one's
        // spawn is the two probes' launch race, not the server.
        (
            "clients",
            per_client(traces().filter_map(|l| field(l, "clients")).collect()),
        ),
        ("players", dedup(ents("p"))),
        (
            "models",
            if gametype.starts_with("hq") {
                // Which of the seven radios a snapshot holds is the random
                // pick's PVS; what each one shows as is the script's.
                first_seen(
                    ents("m")
                        .filter(|e| e.contains("objective"))
                        .map(|e| e.split_once(':').map_or(e.clone(), |x| x.1.to_string()))
                        .map(|e| e.rsplit_once(':').map_or(e.clone(), |x| x.0.to_string())),
                )
            } else if uo {
                // Per snapshot, as a sorted set without the entity numbers
                // the unspawned vehicles shift.
                dedup(traces().map(|l| {
                    let mut v: Vec<String> = split_ents(field(l, "ents").unwrap_or("-"))
                        .into_iter()
                        .filter(|e| e.split(':').nth(1) == Some("m") && e.contains("objective"))
                        .map(|e| e.split_once(':').map_or(e.clone(), |(_, r)| r.to_string()))
                        .collect();
                    v.sort();
                    v.join(",")
                }))
            } else {
                dedup(ents("m").filter(|e| e.contains("objective")))
            },
        ),
        // Each body-queue slot once, at its first appearance: a corpse leaves
        // and re-enters the snapshot as the viewer's PVS moves.
        ("corpses", first_seen(ents("c"))),
    ]
}

/// Each run of adjacent `MPSCRIPT_JOINED_*` prints in a fixed order: after
/// a restart that drops `pers[]` both probes answer the team menu off the
/// same message, and whose answer the server reads first is the two
/// clients' race (retail's dom run read axis first, its ctf run allies).
fn sort_join_runs(mut prints: Vec<String>) -> Vec<String> {
    let joined = |p: &String| p.contains("MPSCRIPT_JOINED_");
    let mut i = 0;
    while i < prints.len() {
        let n = prints[i..].iter().take_while(|p| joined(p)).count();
        prints[i..i + n].sort();
        i += n.max(1);
    }
    prints
}

/// `b <n> <axis> <allies>` and its rows without the ping.
fn scoreboard_rows(t: &str) -> String {
    let w: Vec<&str> = t.split(' ').collect();
    let mut out = w[..4.min(w.len())].join(" ");
    for row in w.get(4..).unwrap_or(&[]).chunks(5) {
        if let [slot, score, _ping, deaths, icon] = row {
            out.push_str(&format!(" {slot}:{score}:{deaths}:{icon}"));
        }
    }
    out
}

/// The probe's `PROBE` lines and the stock scripts' `A;`/`K;`/`W;`/`L;`
/// records up to `PROBE done`, with every server clock (an integer of four
/// digits or more) blanked and every coordinate rounded to the unit: a
/// player set down a unit and a half over a 2-degree grade settles a few
/// hundredths of a unit apart on the two servers.
fn log_view(lines: &[String]) -> Vec<String> {
    let keep = |l: &str| {
        l.starts_with("PROBE ") || ["A;", "K;", "W;", "L;"].iter().any(|p| l.starts_with(p))
    };
    let mut done = false;
    let upto_done = lines.iter().map(|l| l.trim()).take_while(|l| {
        let more = !done;
        done |= l.starts_with("PROBE done");
        more
    });
    dedup(upto_done.filter(|l| keep(l)).map(|l| {
        l.split(' ')
            .map(|w| {
                if w.len() >= 4 && w.chars().all(|c| c.is_ascii_digit()) {
                    return "T".to_string();
                }
                let core = w.trim_matches(|c| c == '(' || c == ')' || c == ',');
                match core
                    .contains('.')
                    .then(|| core.parse::<f32>().ok())
                    .flatten()
                {
                    Some(v) => w.replace(core, &format!("{}", v.round() as i32)),
                    None => w.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }))
}

/// The `getTime()` of each `PROBE spawned` pick of client `slot` (a probe
/// that logs them, `probe_bel`), the first against the role's first
/// playing snapshot, then each against its `PROBE placed`. Retail runs the
/// thread a menu answer wakes on the new frame's clock, so the first pick
/// reads that snapshot's time, and its placing `wait 0.05` wakes on the
/// same one; a pick off `respawn`'s thread is placed 50 later
/// (cod11-gametypes-re-bel.md 8.7).
fn spawn_clock(log: &[String], slot: usize, role: &[String]) -> Vec<String> {
    let probe = |l: &str, tag: &str| -> Option<i64> {
        let w: Vec<&str> = l.strip_prefix(tag)?.split(' ').collect();
        (w.first()? == &slot.to_string()).then_some(())?;
        w.get(2)?.parse().ok()
    };
    let first_play = role.iter().find_map(|l| {
        let l = l.strip_prefix("!trace ")?;
        l.contains(" pm_type=0 ").then_some(())?;
        l.split(' ')
            .find_map(|w| w.strip_prefix("serverTime="))?
            .parse::<i64>()
            .ok()
    });
    let mut out = Vec::new();
    for (i, l) in log.iter().enumerate() {
        let Some(t) = probe(l, "PROBE spawned ") else {
            continue;
        };
        if out.is_empty() {
            out.push(match first_play {
                Some(p) => format!("first pick {:+} of the first playing snapshot", t - p),
                None => "never playing".to_string(),
            });
        }
        let placed = log[i + 1..].iter().find_map(|l| probe(l, "PROBE placed "));
        out.push(match placed {
            Some(p) => format!("placed {:+}", p - t),
            None => "unplaced".to_string(),
        });
    }
    out
}

fn read_lines(path: &str) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {path}: {e}"))
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Compares every view of every role and the log, honouring [`GAPS`].
fn compare(
    setup: &Setup,
    gametype: &str,
    roles: [&str; 2],
    ours: [Vec<String>; 2],
    log: Vec<String>,
    end_ms: i64,
) {
    let retail_log = read_lines(&format!(
        "{FIXTURES}/{}-{gametype}-log{}.txt",
        setup.map,
        setup.tag()
    ));
    let mut failures = Vec::new();
    let mut check = |role: &str, view: &str, retail: &[String], ours: &[String]| {
        if report() {
            println!("== {gametype} {role} {view}\n  retail:");
            retail.iter().for_each(|r| println!("    {r}"));
            println!("  ours:");
            ours.iter().for_each(|r| println!("    {r}"));
        }
        let gap = GAPS
            .iter()
            .find(|(g, r, v, _)| *g == gametype && *r == role && *v == view);
        match (gap, retail == ours) {
            (Some((.., why)), true) => failures.push(format!(
                "{gametype} {role} {view} now matches retail; drop it from GAPS ({why})"
            )),
            (None, false) => failures.push(format!(
                "{gametype} {role} {view} differs:\n  retail {retail:#?}\n  ours   {ours:#?}"
            )),
            _ => {}
        }
    };
    if let Ok(dir) = std::env::var("GAMETYPES_DUMP") {
        for (i, role) in roles.iter().enumerate() {
            let path = format!("{dir}/{}-{gametype}-{role}{}.txt", setup.map, setup.tag());
            std::fs::write(&path, ours[i].join("\n") + "\n").expect("write the dump");
        }
        let path = format!("{dir}/{}-{gametype}-log{}.txt", setup.map, setup.tag());
        std::fs::write(&path, log.join("\n") + "\n").expect("write the dump");
    }
    for (i, role) in roles.iter().enumerate() {
        let retail = until_teardown(
            &read_lines(&format!(
                "{FIXTURES}/{}-{gametype}-{role}{}.txt",
                setup.map,
                setup.tag()
            )),
            end_ms,
        );
        let ours = until_teardown(&ours[i], end_ms);
        let (rv, ov) = (
            views(&retail, gametype, setup.uo),
            views(&ours, gametype, setup.uo),
        );
        for ((name, r), (_, o)) in rv.iter().zip(&ov) {
            check(role, name, r, o);
        }
        check(
            role,
            "spawn clock",
            &spawn_clock(&retail_log, i, &retail),
            &spawn_clock(&log, i, &ours),
        );
    }
    // Two probe threads that wake on one frame log in an order that
    // alternates with the frame's parity, so the state lines are their own
    // view.
    let split = |v: Vec<String>| -> (Vec<String>, Vec<String>) {
        v.into_iter().partition(|l| !l.starts_with("PROBE state"))
    };
    let (rl, rs) = split(log_view(&read_lines(&format!(
        "{FIXTURES}/{}-{gametype}-log{}.txt",
        setup.map,
        setup.tag()
    ))));
    let (ol, os) = split(log_view(&log));
    check("server", "log", &rl, &ol);
    check("server", "state", &rs, &os);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn retrieval_matches_retail() {
    let clock = retail_clock(&V1, "re", "attacker");
    let roles = ["attacker", "defender"];
    let Some((ours, log)) = run_ours(&V1, "probe_re", ["allies", "axis"], 130, &clock, roles)
    else {
        eprintln!("COD_DIR unset: skipping");
        return;
    };
    compare(&V1, "re", roles, ours, log, 125_000);
}

#[test]
fn behind_enemy_lines_matches_retail() {
    let clock = retail_clock(&V1, "bel", "first");
    let roles = ["first", "second"];
    let Some((ours, log)) = run_ours(&V1, "probe_bel", ["axis", "axis"], 130, &clock, roles) else {
        eprintln!("COD_DIR unset: skipping");
        return;
    };
    compare(&V1, "bel", roles, ours, log, 125_000);
}

/// One UO gametype under its probe, allies first, then axis, compared up to
/// `end_ms` of the first probe's clock.
fn uo_gametype(setup: &Setup, gametype: &str, secs: u64, end_ms: i64) {
    let probe = format!("probe_{gametype}");
    let clock = retail_clock(setup, gametype, "attacker");
    let roles = ["attacker", "defender"];
    let Some((ours, log)) = run_ours(setup, &probe, ["allies", "axis"], secs, &clock, roles) else {
        eprintln!("COD_DIR not at a UO install: skipping");
        return;
    };
    compare(setup, gametype, roles, ours, log, end_ms);
}

#[test]
fn uo_capture_the_flag_matches_retail() {
    uo_gametype(&UO, "ctf", 75, 70_000);
}

#[test]
fn uo_domination_matches_retail() {
    uo_gametype(&UO, "dom", 75, 70_000);
}

#[test]
fn uo_base_assault_matches_retail() {
    uo_gametype(&UO_FOY, "bas", 80, 75_000);
}

#[test]
fn uo_headquarters_matches_retail() {
    // Before the next radio, which shows 45 s after the neutralize, close to
    // where retail's second probe leaves.
    uo_gametype(&UO, "hq", 110, 90_000);
}

// The endings: each probe plays its gametype to a map end and through the
// restart exitLevel(false) falls back to with no rotation (coduo-diff.md 22,
// "Round and map ends").

#[test]
fn uo_capture_the_flag_drop_return_and_map_end_match_retail() {
    uo_gametype(&UO, "ctfend", 75, 70_000);
}

#[test]
fn uo_domination_bonus_and_map_end_match_retail() {
    uo_gametype(&UO, "domend", 100, 95_000);
}

#[test]
fn uo_headquarters_waves_and_map_end_match_retail() {
    uo_gametype(&UO, "hqend", 180, 175_000);
}

#[test]
fn uo_base_assault_plant_defuse_and_map_end_match_retail() {
    uo_gametype(&UO_FOY, "basend", 160, 155_000);
}

#[test]
fn uo_capture_the_flag_touch_return_time_limit_and_rotation_match_retail() {
    uo_gametype(&UO, "ctftime", 90, 80_000);
}

#[test]
fn uo_team_deathmatch_killcam_and_time_limit_match_retail() {
    uo_gametype(&UO, "tdmend", 50, 45_000);
}

// `doDamage` on a client: the callback's arguments, a dead or killcam
// target, a kill with and without an attacker; then the killcam skipped by
// a use tap (coduo-diff.md 22).
#[test]
fn uo_team_deathmatch_do_damage_and_killcam_skip_match_retail() {
    uo_gametype(&UO, "tdmskip", 40, 35_000);
}

#[test]
fn uo_base_assault_round_clock_and_round_limit_match_retail() {
    uo_gametype(&UO_FOY, "bastime", 80, 75_000);
}

#[test]
fn uo_headquarters_time_limit_matches_retail() {
    uo_gametype(&UO, "hqtime", 50, 45_000);
}

#[test]
fn uo_domination_round_clock_and_time_limit_match_retail() {
    uo_gametype(&UO, "domtime", 100, 95_000);
}
