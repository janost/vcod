//! Retrieval's pickup, drop, return and capture and Behind Enemy Lines' team
//! swap, against the retail captures in `tests/fixtures/gametypes/`.
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
//! Needs `COD_DIR`; without the paks both tests return early.

mod common;

use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;
use std::time::{Duration, Instant};

use common::{ADDR, ADDR_B, ClientEnd, Queues};
use vcod_common::net::capture::ScriptedCapture;
use vcod_common::net::{NetClient, NetEvent};
use vcod_server::Server;

const MAP: &str = "mp_brecourt";
const FIXTURES: &str = "tests/fixtures/gametypes";
const PROBES: &str = "../gsc/tests/fixtures/semantics/client-probes";

/// Retail had been up this long when the first probe connected (the recipe
/// starts the probes 4 s after the server).
const FIRST_JOIN_MS: u64 = 4000;
/// And the second probe this long after the first: retail's first probe
/// sees the second in its roster 2.4 s into its capture (3 s between the
/// launches, less the first one's own connect).
const SECOND_JOIN_MS: u64 = 2400;
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
/// team, a single-nation weapon menu with that nation's default rifle, and
/// nothing else (bel's two-nation menu stays open on both).
fn menu_reply(menu: &str, team: &str) -> Option<String> {
    if menu.starts_with("team_") {
        return Some(team.to_string());
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
        self.cl.send_frame(&cmd);
    }

    fn receive(&mut self, now: Instant) {
        let ms = self.ms(now);
        for e in self.cl.pump_at(now) {
            match e {
                NetEvent::GamestateReady => {
                    self.main_menu.clear();
                    self.answered.clear();
                    let map =
                        vcod_common::net::info_value_for_key(self.cl.configstring(0), "mapname")
                            .unwrap_or("?")
                            .to_string();
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
            Some("n") => {
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
                if let Some(reply) = menu_reply(&self.main_menu, self.team) {
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
    probe: &str,
    teams: [&'static str; 2],
    secs: u64,
) -> Option<([Vec<String>; 2], Vec<String>)> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = Server::new(common::cfg(MAP, probe), now);
    let src = std::fs::read_to_string(format!("{PROBES}/{probe}.gsc")).expect("the probe");
    sv.overlay_script(&format!("maps/mp/gametypes/{probe}"), &src);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");

    let start = now;
    let mut probes: Vec<Probe> = Vec::new();
    let mut log: Vec<String> = Vec::new();
    let mut drained = 0usize;
    let joins = [FIRST_JOIN_MS, FIRST_JOIN_MS + SECOND_JOIN_MS];
    let frames = (secs * 1000 + joins[1]) / FRAME_MS;
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
        sv.tick(now);
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

/// An objective slot without its origin: bel's compass marker starts where
/// the random spawn pick put the allied player and follows a running mean of
/// where it stood.
fn objectives_without_origin(o: &str) -> String {
    o.split(',')
        .filter(|w| w.contains(':'))
        .map(|w| w.rsplit_once(':').map_or(w, |(head, _)| head))
        .collect::<Vec<_>>()
        .join(",")
}

/// Every view of one recording, by name. `bel` drops the objective origins
/// ([`objectives_without_origin`]).
fn views(lines: &[String], gametype: &str) -> Vec<(&'static str, Vec<String>)> {
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
            cmds()
                .filter(|t| t.starts_with("f "))
                .map(str::to_string)
                .collect(),
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
            if gametype == "bel" {
                dedup(
                    tf("objectives")
                        .iter()
                        .map(|o| objectives_without_origin(o)),
                )
            } else {
                tf("objectives")
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
            dedup(ents("m").filter(|e| e.contains("objective"))),
        ),
        // Each body-queue slot once, at its first appearance: a corpse leaves
        // and re-enters the snapshot as the viewer's PVS moves.
        ("corpses", first_seen(ents("c"))),
    ]
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
    gametype: &str,
    roles: [&str; 2],
    ours: [Vec<String>; 2],
    log: Vec<String>,
    end_ms: i64,
) {
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
            let path = format!("{dir}/{MAP}-{gametype}-{role}.txt");
            std::fs::write(&path, ours[i].join("\n") + "\n").expect("write the dump");
        }
        let path = format!("{dir}/{MAP}-{gametype}-log.txt");
        std::fs::write(&path, log.join("\n") + "\n").expect("write the dump");
    }
    for (i, role) in roles.iter().enumerate() {
        let retail = until_teardown(
            &read_lines(&format!("{FIXTURES}/{MAP}-{gametype}-{role}.txt")),
            end_ms,
        );
        let ours = until_teardown(&ours[i], end_ms);
        let (rv, ov) = (views(&retail, gametype), views(&ours, gametype));
        for ((name, r), (_, o)) in rv.iter().zip(&ov) {
            check(role, name, r, o);
        }
    }
    // Two probe threads that wake on one frame log in an order that
    // alternates with the frame's parity, so the state lines are their own
    // view.
    let split = |v: Vec<String>| -> (Vec<String>, Vec<String>) {
        v.into_iter().partition(|l| !l.starts_with("PROBE state"))
    };
    let (rl, rs) = split(log_view(&read_lines(&format!(
        "{FIXTURES}/{MAP}-{gametype}-log.txt"
    ))));
    let (ol, os) = split(log_view(&log));
    check("server", "log", &rl, &ol);
    check("server", "state", &rs, &os);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn retrieval_matches_retail() {
    let Some((ours, log)) = run_ours("probe_re", ["allies", "axis"], 130) else {
        eprintln!("COD_DIR unset: skipping");
        return;
    };
    compare("re", ["attacker", "defender"], ours, log, 125_000);
}

#[test]
fn behind_enemy_lines_matches_retail() {
    let Some((ours, log)) = run_ours("probe_bel", ["axis", "axis"], 130) else {
        eprintln!("COD_DIR unset: skipping");
        return;
    };
    compare("bel", ["first", "second"], ours, log, 125_000);
}
