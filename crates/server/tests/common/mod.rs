//! vcod's own NetClient against the server, in process, over two queues.
//! `now` is advanced by hand. Shared by the tests that drive a client; each
//! of them uses part of it, hence the crate-level allow.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{UserCmd, NULL_USERCMD};
use vcod_common::net::{NetClient, NetEvent, Transport};
use vcod_server::Server;

pub const ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 31337);

/// A second client's address, for the tests that need two on one server.
pub const ADDR_B: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 31338);

#[derive(Default)]
pub struct Queues {
    pub to_server: VecDeque<Vec<u8>>,
    pub to_client: VecDeque<Vec<u8>>,
}

pub struct ClientEnd(pub Rc<RefCell<Queues>>);

impl Transport for ClientEnd {
    fn try_recv(&mut self, buf: &mut [u8]) -> Option<usize> {
        let p = self.0.borrow_mut().to_client.pop_front()?;
        buf[..p.len()].copy_from_slice(&p);
        Some(p.len())
    }
    fn send(&mut self, data: &[u8]) {
        self.0.borrow_mut().to_server.push_back(data.to_vec());
    }
}

/// One exchange each way, then one client pump.
pub fn step(
    sv: &mut Server,
    q: &Rc<RefCell<Queues>>,
    cl: &mut NetClient<ClientEnd>,
    now: Instant,
) -> Vec<NetEvent> {
    let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
    for p in pending {
        sv.handle_packet(ADDR, &p, now);
    }
    sv.tick(now);
    for (to, p) in sv.take_outgoing() {
        assert_eq!(to, ADDR);
        q.borrow_mut().to_client.push_back(p);
    }
    cl.pump_at(now)
}

/// Like `step`, but the server's reply never reaches the client — a lost
/// snapshot packet, not a lost ack. The client's next real ack then names a
/// frame it received without having received the one right before it, the
/// only case where a server-side message_num that has drifted from the
/// packet's actual netchan sequence is observable on the wire (see
/// `frames_delta_against_the_acked_base_and_fall_back_when_acks_stop`).
pub fn step_dropping_reply(
    sv: &mut Server,
    q: &Rc<RefCell<Queues>>,
    cl: &mut NetClient<ClientEnd>,
    now: Instant,
) -> Vec<NetEvent> {
    let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
    for p in pending {
        sv.handle_packet(ADDR, &p, now);
    }
    sv.tick(now);
    sv.take_outgoing();
    cl.pump_at(now)
}

/// Steps the server with two clients attached, routing each packet by the
/// address it came from and each reply by the address it is for. `step`
/// cannot: it asserts every reply is for the one client it knows.
pub fn step_pair(
    sv: &mut Server,
    a: (&Rc<RefCell<Queues>>, &mut NetClient<ClientEnd>),
    b: (&Rc<RefCell<Queues>>, &mut NetClient<ClientEnd>),
    now: Instant,
) -> (Vec<NetEvent>, Vec<NetEvent>) {
    for (addr, q) in [(ADDR, a.0), (ADDR_B, b.0)] {
        let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
        for p in pending {
            sv.handle_packet(addr, &p, now);
        }
    }
    sv.tick(now);
    for (to, p) in sv.take_outgoing() {
        let q = if to == ADDR { a.0 } else { b.0 };
        q.borrow_mut().to_client.push_back(p);
    }
    (a.1.pump_at(now), b.1.pump_at(now))
}

/// Two clients through the stock menus onto one server, stepped together so
/// each is live while the other joins. Each takes its own `(team, weapon)`:
/// the weapon menu is per nationality, so two clients on opposite teams
/// cannot answer it with the same weapon.
pub fn join_pair(
    sv: &mut Server,
    qa: &Rc<RefCell<Queues>>,
    qb: &Rc<RefCell<Queues>>,
    now: &mut Instant,
    a: (&str, &str),
    b: (&str, &str),
) -> (NetClient<ClientEnd>, NetClient<ClientEnd>) {
    // Distinct qports: the server keys a peer by ip and qport, so two clients
    // sharing one read as a single client reconnecting and the second is
    // refused. A real client is one per process and gets this for free.
    let mut ca = NetClient::start_with_qport(ClientEnd(qa.clone()), *now, 0x2001);
    let mut cb = NetClient::start_with_qport(ClientEnd(qb.clone()), *now, 0x2002);
    let (mut ja, mut jb) = (Join::new(a.0, a.1), Join::new(b.0, b.1));
    for _ in 0..600 {
        *now += Duration::from_millis(50);
        ca.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        cb.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        let (ea, eb) = step_pair(sv, (qa, &mut ca), (qb, &mut cb), *now);
        for (events, join, cl) in [(ea, &mut ja, &mut ca), (eb, &mut jb, &mut cb)] {
            for e in events {
                match e {
                    NetEvent::ServerCommand(tokens) => join.on_server_command(&tokens, cl, *now),
                    NetEvent::Dropped(r) => panic!("dropped mid-join: {r}"),
                    _ => {}
                }
            }
        }
        if ja.settled(*now) && jb.settled(*now) {
            break;
        }
    }
    (ca, cb)
}

pub fn connect(
    sv: &mut Server,
    q: &Rc<RefCell<Queues>>,
    now: &mut Instant,
) -> NetClient<ClientEnd> {
    let mut cl = NetClient::start(ClientEnd(q.clone()), *now);
    for _ in 0..40 {
        *now += Duration::from_millis(250);
        let events = step(sv, q, &mut cl, *now);
        if events.contains(&NetEvent::GamestateReady) {
            return cl;
        }
        assert!(
            !events.iter().any(|e| matches!(e, NetEvent::Dropped(_))),
            "{events:?}"
        );
    }
    panic!(
        "no gamestate within 10 s of simulated time; state {:?}",
        cl.state()
    );
}

// --------------------------------------------------------------- the fixtures

pub fn cfg(map: &str, gametype: &str) -> vcod_server::ServerConfig {
    vcod_server::ServerConfig {
        map: map.into(),
        hostname: "vcod test".into(),
        max_clients: 8,
        gametype: gametype.into(),
        test_entities: 0,
        trace: false,
    }
}

/// One server frame, and one usercmd, in ms. A client sends cmds faster than
/// the server ticks; the replay sends two per frame because a tap one frame
/// long can land on the very frame a semi-automatic weapon's `weaponTime`
/// expires, where the latch swallows it (combat doc, section 1.4) and the
/// capture's own tap, 32 ms out of every 183, does not.
pub const FRAME_MS: i64 = 50;
pub const CMD_MS: i64 = 25;

/// How long a `wait_ready` step holds its input before it starts looking for
/// a ready weapon. The capture's own floor: every `wait_ready` step of both
/// fixtures reports `waited_ready_ms` around 505, even the ones that had
/// nothing to wait for. Without it the wait ends on the snapshot that arrived
/// before the step's first cmd was even simulated, and a step opens with the
/// weapon still busy from the one before it.
pub const WAIT_FLOOR_MS: i64 = 500;

pub struct Step {
    pub label: String,
    pub base: UserCmd,
    pub pulse_buttons: u8,
    pub pulse_wbuttons: u8,
    pub pulses: u32,
    pub pulse_period_ms: i64,
    pub hold_ms: i64,
    pub walks: bool,
    pub wait_ready: bool,
    /// The weapon byte the probe sent through the step, `cmd.weapon`: retail
    /// reads a byte that differs from `ps.weapon` as a request to holster
    /// (`cod11-combat.md` section 1.8), so the replay has to send the same
    /// one. Defaults to the joined weapon's CS 7 index for a capture taken
    /// before the key existed.
    pub weapon: u8,
    /// Bits held down for the first `press_ms` of the step rather than
    /// tapped: a grenade is cooked by holding the trigger and thrown by the
    /// release, which the tap machinery above cannot express.
    pub press_buttons: u8,
    pub press_ms: i64,
    /// A `cmd.weapon` the step asks for `switch_ms` into itself, held on
    /// every cmd after that: retail's pickup half reads the byte again on the
    /// frame the putaway ends, so a byte sent once leaves the old weapon in
    /// hand (`cod11-combat.md` section 1.8).
    pub switch_weapon: u8,
    pub switch_ms: i64,
    /// Retail's per-snapshot trace: (weaponstate, weapAnim, torsoAnim,
    /// eventSequence, events).
    pub trace: Vec<Trace>,
}

#[derive(Clone, Copy)]
pub struct Trace {
    /// ms into the step; retail's is the probe's clock, ours the frame grid.
    pub ms: i64,
    pub weaponstate: i32,
    pub weap_anim: i32,
    pub torso_anim: i32,
    pub event_sequence: i32,
    pub events: [i32; 4],
    /// `eventParms[i]`; `None` on a capture whose trace lines carry no such
    /// column, which is every one taken so far.
    pub event_parms: Option<[i32; 4]>,
    /// `fWeaponPosFrac` and `aimSpreadScale`; `None` on a capture taken
    /// before the trace carried them.
    pub pos_frac: Option<f32>,
    pub spread: Option<f32>,
    /// `grenadeTimeLeft` and `weaponDelay`; `None` on a capture taken before
    /// the trace carried them.
    pub grenade_time_left: Option<i32>,
    pub weapon_delay: Option<i32>,
}

pub fn parse_fixture(text: &str, default_weapon: u8) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(label) = line
            .strip_prefix("[step ")
            .and_then(|l| l.strip_suffix(']'))
        {
            steps.push(Step {
                label: label.into(),
                base: NULL_USERCMD,
                pulse_buttons: 0,
                pulse_wbuttons: 0,
                pulses: 0,
                pulse_period_ms: 0,
                hold_ms: 0,
                walks: false,
                wait_ready: false,
                weapon: default_weapon,
                press_buttons: 0,
                press_ms: 0,
                switch_weapon: 0,
                switch_ms: 0,
                trace: Vec::new(),
            });
            continue;
        }
        let step = steps.last_mut().expect("a line before any [step]");
        let kv = |rest: &str| -> BTreeMap<String, String> {
            rest.split_whitespace()
                .filter_map(|t| t.split_once('='))
                .map(|(k, v)| (k.into(), v.into()))
                .collect()
        };
        if let Some(rest) = line.strip_prefix("!input ") {
            let m = kv(rest);
            let i = |k: &str| m[k].parse::<i64>().unwrap();
            step.base.buttons = i("buttons") as u8;
            step.base.wbuttons = i("wbuttons") as u8;
            step.base.up = i("up") as i8;
            step.base.forward = i("forward") as i8;
            step.base.right = i("right") as i8;
            step.base.angles[1] = i("yaw") as i32;
            // The `throw_down` step aims 45 degrees below the horizon; every
            // capture taken before the column existed left it level.
            if let Some(p) = m.get("pitch") {
                step.base.angles[0] = p.parse::<i64>().unwrap() as i32;
            }
            for (key, into) in [
                ("press_buttons", &mut step.press_buttons),
                ("switch_weapon", &mut step.switch_weapon),
            ] {
                if let Some(v) = m.get(key) {
                    *into = v.parse::<u8>().unwrap();
                }
            }
            for (key, into) in [
                ("press_ms", &mut step.press_ms),
                ("switch_ms", &mut step.switch_ms),
            ] {
                if let Some(v) = m.get(key) {
                    *into = v.parse::<i64>().unwrap();
                }
            }
            step.pulse_buttons = i("pulse_buttons") as u8;
            step.pulse_wbuttons = i("pulse_wbuttons") as u8;
            step.pulses = i("pulses") as u32;
            step.pulse_period_ms = i("pulse_period_ms");
            step.hold_ms = i("hold_ms");
            step.walks = i("walks") != 0;
            step.wait_ready = i("wait_ready") != 0;
            if let Some(w) = m.get("weapon") {
                step.weapon = w.parse::<u8>().unwrap();
            }
        } else if let Some(rest) = line.strip_prefix("!trace ") {
            let m = kv(rest);
            let i = |k: &str| m[k].parse::<i32>().unwrap();
            let f = |k: &str| m.get(k).map(|v| v.parse::<f32>().unwrap());
            step.trace.push(Trace {
                ms: m["ms"].parse::<i64>().unwrap(),
                weaponstate: i("weaponstate"),
                weap_anim: i("weapAnim"),
                torso_anim: i("torsoAnim"),
                event_sequence: i("eventSequence"),
                events: [
                    i("events[0]"),
                    i("events[1]"),
                    i("events[2]"),
                    i("events[3]"),
                ],
                event_parms: m.contains_key("eventParms[0]").then(|| {
                    [
                        i("eventParms[0]"),
                        i("eventParms[1]"),
                        i("eventParms[2]"),
                        i("eventParms[3]"),
                    ]
                }),
                pos_frac: f("fWeaponPosFrac"),
                spread: f("aimSpreadScale"),
                grenade_time_left: m.get("grenadeTimeLeft").map(|v| v.parse().unwrap()),
                weapon_delay: m.get("weaponDelay").map(|v| v.parse().unwrap()),
            });
        }
        // `!observed` and the settled field lines are not compared here.
    }
    steps
}

/// When each of a step's taps goes down, in ms from the step's start: one
/// usercmd long, at the first cmd at or after the capture's own tap time. The
/// capture holds the bit 32 ms out of every `pulse_period_ms`, which no cmd
/// grid divides evenly, so the tap is placed rather than sampled -- sampling
/// it drops the taps that fall between two cmds.
fn taps(step: &Step) -> Vec<i64> {
    (0..i64::from(step.pulses))
        .map(|k| (k * step.pulse_period_ms + CMD_MS - 1) / CMD_MS * CMD_MS)
        .collect()
}

/// Replays the capture's taps against our server: one trace per snapshot
/// per step, the way the probe traced retail's.
/// The weapon byte a cmd carries: the step's switch once it has been asked
/// for, the step's own byte otherwise, and our own `ps.weapon` where the
/// capture recorded a 0. A 0 is not neutral -- retail reads a `cmd.weapon`
/// differing from `ps.weapon` as a request to holster -- and the capture's 0
/// means the probe was holding nothing, which is a fact about retail's
/// playerstate and not an input to replay.
fn weapon_byte(step: &Step, t: i64, ours_now: u8) -> u8 {
    if step.switch_weapon != 0 && t >= step.switch_ms {
        step.switch_weapon
    } else if step.weapon != 0 {
        step.weapon
    } else {
        ours_now
    }
}

/// One snapshot of a replayed step: the playerstate the client decoded and
/// the entities that came with it. A gate over the weapon channel reads the
/// first, one over the missiles reads the second.
pub struct Sample {
    pub ms: i64,
    pub server_time: i32,
    pub ps: vcod_common::net::msg::PlayerState,
    pub entities: std::collections::BTreeMap<u32, vcod_common::net::msg::EntityState>,
    /// How many blasts the server left for the radius damage pass on the
    /// tick this snapshot came from.
    pub explosions: usize,
}

/// Replays a capture's steps against our server, one sample per snapshot per
/// step, the way the probe traced retail's. `place` puts the client where the
/// capture's header says it stood before the first step: a gate about where a
/// grenade lands needs the same geometry in front of the thrower, and the
/// gametype spawn is weighted-random.
pub fn replay(
    map: &str,
    gametype: &str,
    steps: &[Step],
    join: (&str, &str),
    fs: vcod_common::pk3::Pk3Fs,
    place: Option<([f32; 3], f32)>,
) -> Vec<Vec<Sample>> {
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let fs = std::rc::Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(cfg(map, gametype), now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs).expect("load the scripts");
    let q = std::rc::Rc::new(RefCell::new(Queues::default()));
    let (mut cl, _join) = self::join(&mut sv, &q, &mut now, join.0, join.1);
    // The client sends absolute view angles: `NetClient::send_frame`
    // subtracts the playerstate's `delta_angles` off them, so the spawn yaw
    // `place_client` leaves there is cancelled unless the capture's own yaw
    // is offset by it. A capture's `!input yaw` is relative to the facing its
    // header records.
    let mut yaw_short = 0;
    if let Some((origin, yaw)) = place {
        sv.place_client(0, origin, yaw);
        yaw_short = (yaw * 65536.0 / 360.0) as i32;
    }
    let p = &vcod_common::net::protocol::PROTOCOL_V1;
    let mut out = Vec::new();
    for step in steps {
        let mut samples = Vec::new();
        // The held input, carrying the weapon byte the probe sent: a byte that
        // differs from `ps.weapon` is a holster request, so a replay that left
        // it 0 would not be running the capture's input.
        let mut base = step.base;
        base.weapon = step.weapon;
        base.angles[1] = base.angles[1].wrapping_add(yaw_short);
        let held = |cl: &vcod_common::net::NetClient<ClientEnd>| -> u8 {
            cl.snapshots()
                .newest()
                .map_or(0, |s| s.ps.field_i32(p, "weapon") as u8)
        };
        if step.weapon == 0 {
            base.weapon = held(&cl);
        }
        if step.wait_ready {
            for i in 0..100 {
                now += Duration::from_millis(FRAME_MS as u64);
                cl.send_frame(&base);
                self::step(&mut sv, &q, &mut cl, now);
                let ready = cl
                    .snapshots()
                    .newest()
                    .is_some_and(|s| s.ps.field_i32(p, "weaponstate") == 0);
                if i * FRAME_MS >= WAIT_FLOOR_MS && ready {
                    break;
                }
            }
        }
        // The state the step opens in, before its first tap: retail's own
        // first sample is the one that arrived with the tap still in flight,
        // so without it a shot on the first frame falls outside every
        // window a gate looks at.
        if let Some(s) = cl.snapshots().newest() {
            samples.push(sample_of(s, 0, sv.pending_explosions().len()));
        }
        let frames = (step.hold_ms / FRAME_MS).max(1);
        let taps = taps(step);
        for i in 0..frames {
            let ours_now = held(&cl);
            for half in 0..2 {
                let t = i * FRAME_MS + half * CMD_MS;
                let mut cmd = base;
                cmd.weapon = weapon_byte(step, t, ours_now);
                if t < step.press_ms {
                    cmd.buttons |= step.press_buttons;
                }
                if taps.contains(&t) {
                    cmd.buttons |= step.pulse_buttons;
                    cmd.wbuttons |= step.pulse_wbuttons;
                }
                now += Duration::from_millis(CMD_MS as u64);
                cl.pump_at(now);
                cl.send_frame(&cmd);
            }
            self::step(&mut sv, &q, &mut cl, now);
            let blasts = sv.pending_explosions().len();
            if let Some(s) = cl.snapshots().newest() {
                samples.push(sample_of(s, (i + 1) * FRAME_MS, blasts));
            }
        }
        out.push(samples);
    }
    out
}

fn sample_of(s: &vcod_common::net::snapshot::Snapshot, ms: i64, explosions: usize) -> Sample {
    Sample {
        ms,
        server_time: s.server_time,
        ps: s.ps.clone(),
        entities: s.entities.clone(),
        explosions,
    }
}

/// Every `# key value` clause in a fixture's header block with this key. The
/// callers insist on exactly one: the header is prose as well as data, so a
/// second clause opening with the same word would otherwise decide the join
/// silently, by being first.
pub fn header_values<'a>(text: &'a str, key: &str) -> Vec<&'a str> {
    text.lines()
        .take_while(|l| l.starts_with('#'))
        .flat_map(|l| l.trim_start_matches('#').split(','))
        .map(str::trim)
        .filter_map(|clause| clause.strip_prefix(key)?.strip_prefix(' '))
        .map(|v| v.trim_end_matches('.'))
        .collect()
}

/// The one clause with this key, or a panic naming `path` and what it found.
pub fn header_value<'a>(text: &'a str, key: &str, path: &str) -> &'a str {
    let found = header_values(text, key);
    assert_eq!(
        found.len(),
        1,
        "{path}: the header has {} clauses opening with {key:?}, expected 1: {found:?}",
        found.len()
    );
    found[0]
}

/// Every `key=value` on a capture's `# grenade` header line, and `None` for
/// a capture that has none. Not [`header_value`]: that splits a clause on
/// commas, and this line's `origin=` carries two of them.
pub fn grenade_header(text: &str) -> Option<BTreeMap<String, String>> {
    let line = text.lines().find(|l| l.starts_with("# grenade "))?;
    Some(
        line.trim_start_matches("# grenade ")
            .split_whitespace()
            .filter_map(|t| t.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
}

pub fn header_vec3(header: &BTreeMap<String, String>, key: &str) -> [f32; 3] {
    let mut out = [0.0; 3];
    let raw = header
        .get(key)
        .unwrap_or_else(|| panic!("the `# grenade` header carries no {key}"));
    for (i, v) in raw.split(',').enumerate().take(3) {
        out[i] = v.parse().expect("a number in the grenade header");
    }
    out
}

/// Where a capture's own script stood and looked when it started, for a
/// replay that has to throw from the same spot. `None` for a capture that
/// records none, which is every one but the grenade's.
pub fn captured_place(text: &str) -> Option<([f32; 3], f32)> {
    let h = grenade_header(text)?;
    Some((header_vec3(&h, "origin"), header_vec3(&h, "viewangles")[1]))
}

/// The team and weapon the retail playerstate capture was taken with, so a
/// test that drives the stock menus asks for the same two things the gate
/// does rather than carrying its own copy of them.
pub fn captured_join(map: &str, gametype: &str) -> (String, String) {
    let path = format!("tests/fixtures/playerstate/{map}-{gametype}.txt");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    (
        header_value(&text, "joined", &path).to_string(),
        header_value(&text, "weapon", &path).to_string(),
    )
}

// ------------------------------------------------------------------- the join

/// The fixture header's "3 s after the weapon menu was answered".
pub const SPAWN_SETTLE: Duration = Duration::from_secs(3);

/// Answers the stock team and weapon menus. `v g_scriptMainMenu <menu>` names
/// the menu, `t <index>` opens it and `mr <serverId> <index> <response>`
/// answers it (docs/research/cod11-hud-protocol.md, section 0.1).
pub struct Join {
    team: String,
    weapon: String,
    main_menu: String,
    answered: Vec<i32>,
    answered_team: bool,
    answered_weapon_at: Option<Instant>,
    log: Vec<String>,
}

impl Join {
    pub fn new(team: &str, weapon: &str) -> Self {
        Join {
            team: team.to_string(),
            weapon: weapon.to_string(),
            main_menu: String::new(),
            answered: Vec::new(),
            answered_team: false,
            answered_weapon_at: None,
            log: Vec::new(),
        }
    }

    pub fn on_server_command(
        &mut self,
        tokens: &[String],
        cl: &mut NetClient<ClientEnd>,
        now: Instant,
    ) {
        match tokens.first().map(String::as_str) {
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
                let menu = self.main_menu.clone();
                let reply = if menu.starts_with("team_") {
                    self.team.clone()
                } else if menu.starts_with("weapon_") {
                    self.weapon.clone()
                } else {
                    self.log.push(format!("menu {idx} ({menu:?}) has no reply"));
                    return;
                };
                cl.send_reliable(&format!("mr {} {idx} {reply}", cl.server_id()));
                self.answered.push(idx);
                self.log
                    .push(format!("answered menu {idx} ({menu}) with {reply}"));
                if menu.starts_with("weapon_") {
                    self.answered_weapon_at = Some(now);
                } else {
                    self.answered_team = true;
                }
            }
            _ => {}
        }
    }

    pub fn settled(&self, now: Instant) -> bool {
        self.answered_weapon_at
            .is_some_and(|t| now.duration_since(t) >= SPAWN_SETTLE)
    }

    /// What the join did, for a failure message.
    pub fn summary(&self) -> String {
        let log = match self.log.is_empty() {
            true => "the server opened no script menu at all".to_string(),
            false => self.log.join("; "),
        };
        format!("join: entered the world, {log}")
    }

    /// The path, not the state it ended in: a player that reached the
    /// playerstate without being asked which team and which weapon did not
    /// join through the stock menus, whatever its 103 fields say.
    pub fn findings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.answered_team {
            out.push("no team menu opened, so the client never answered one".to_string());
        }
        if self.answered_weapon_at.is_none() {
            out.push("no weapon menu opened, so the client never spawned".to_string());
        }
        out
    }
}

/// Connects a client, answers both stock menus with `team` and `weapon`, and
/// runs on until the spawn has settled. 20 s of simulated time at sv_fps 20,
/// well past the settle a completed join needs; a join that never happens
/// burns the lot. The loop's first usercmd is what enters the world and
/// releases `Callback_PlayerConnect`'s `waittill`; the menus follow from it.
pub fn join(
    sv: &mut Server,
    q: &Rc<RefCell<Queues>>,
    now: &mut Instant,
    team: &str,
    weapon: &str,
) -> (NetClient<ClientEnd>, Join) {
    let mut cl = connect(sv, q, now);
    let mut j = Join::new(team, weapon);
    for _ in 0..400 {
        *now += Duration::from_millis(50);
        cl.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        for e in step(sv, q, &mut cl, *now) {
            match e {
                NetEvent::ServerCommand(tokens) => j.on_server_command(&tokens, &mut cl, *now),
                NetEvent::Dropped(r) => panic!("dropped mid-join: {r}"),
                _ => {}
            }
        }
        if j.settled(*now) {
            break;
        }
    }
    (cl, j)
}
