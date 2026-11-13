//! vcod's own NetClient against the server, in process, over two queues.
//! `now` is advanced by hand. Shared by the tests that drive a client; each
//! of them uses part of it, hence the crate-level allow.
#![allow(dead_code)]

pub mod snapcull;

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{NULL_USERCMD, UserCmd};
use vcod_common::net::{NetClient, NetEvent, NetState, Transport};
use vcod_server::Server;

pub const ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 31337);

/// A second client's address, for the tests that need two on one server.
pub const ADDR_B: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 31338);

/// A third client's address, for a test that needs a spectator or other
/// third role beside a joined pair.
pub const ADDR_C: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 31339);

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

/// One exchange each way, then one client pump, for a client at `addr`.
/// `step` is this at [`ADDR`]; a test with a client at another address (a
/// spectator alongside a joined pair) calls this directly.
pub fn step_at(
    sv: &mut Server,
    addr: SocketAddr,
    q: &Rc<RefCell<Queues>>,
    cl: &mut NetClient<ClientEnd>,
    now: Instant,
) -> Vec<NetEvent> {
    let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
    for p in pending {
        sv.handle_packet(addr, &p, now);
    }
    sv.tick(now);
    for (to, p) in sv.take_outgoing() {
        assert_eq!(to, addr);
        q.borrow_mut().to_client.push_back(p);
    }
    cl.pump_at(now)
}

/// One exchange each way, then one client pump.
pub fn step(
    sv: &mut Server,
    q: &Rc<RefCell<Queues>>,
    cl: &mut NetClient<ClientEnd>,
    now: Instant,
) -> Vec<NetEvent> {
    step_at(sv, ADDR, q, cl, now)
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
    let (ea, eb, _) = step_pair_seen(sv, a, b, now);
    (ea, eb)
}

/// What the server put on the wire in one frame, each packet with the
/// address it is for.
pub type SentPackets = Vec<(SocketAddr, Vec<u8>)>;

/// [`step_pair`] handing back every packet the server sent this frame as
/// well. A test about an out-of-band packet needs it: the client answers
/// none of them and raises no event, so the wire is the only witness.
pub fn step_pair_seen(
    sv: &mut Server,
    a: (&Rc<RefCell<Queues>>, &mut NetClient<ClientEnd>),
    b: (&Rc<RefCell<Queues>>, &mut NetClient<ClientEnd>),
    now: Instant,
) -> (Vec<NetEvent>, Vec<NetEvent>, SentPackets) {
    exchange_pair(sv, a, b, now, false)
}

/// [`step_pair`] with `b`'s packets handed to the server ahead of `a`'s.
pub fn step_pair_b_first(
    sv: &mut Server,
    a: (&Rc<RefCell<Queues>>, &mut NetClient<ClientEnd>),
    b: (&Rc<RefCell<Queues>>, &mut NetClient<ClientEnd>),
    now: Instant,
) -> (Vec<NetEvent>, Vec<NetEvent>) {
    let (ea, eb, _) = exchange_pair(sv, a, b, now, true);
    (ea, eb)
}

/// One frame of two clients joining together: while either is still
/// connecting `a`'s packets go first, so `a` takes the lower slot; once both
/// are in, `b`'s do. A frame runs the menu threads of its later packet first
/// (`client-probes/probe_threadorder`), so `a` answers and spawns ahead of
/// `b`, as a retail client that joined first does. Hands back the frame's
/// packets as [`step_pair_seen`] does.
pub fn step_pair_joining(
    sv: &mut Server,
    a: (&Rc<RefCell<Queues>>, &mut NetClient<ClientEnd>),
    b: (&Rc<RefCell<Queues>>, &mut NetClient<ClientEnd>),
    now: Instant,
) -> (Vec<NetEvent>, Vec<NetEvent>, SentPackets) {
    let entered = a.1.state() == NetState::Active && b.1.state() == NetState::Active;
    exchange_pair(sv, a, b, now, entered)
}

fn exchange_pair(
    sv: &mut Server,
    a: (&Rc<RefCell<Queues>>, &mut NetClient<ClientEnd>),
    b: (&Rc<RefCell<Queues>>, &mut NetClient<ClientEnd>),
    now: Instant,
    b_first: bool,
) -> (Vec<NetEvent>, Vec<NetEvent>, SentPackets) {
    let mut order = [(ADDR, a.0), (ADDR_B, b.0)];
    if b_first {
        order.reverse();
    }
    for (addr, q) in order {
        let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
        for p in pending {
            sv.handle_packet(addr, &p, now);
        }
    }
    sv.tick(now);
    let sent = sv.take_outgoing();
    for (to, p) in &sent {
        let q = if *to == ADDR { a.0 } else { b.0 };
        q.borrow_mut().to_client.push_back(p.clone());
    }
    (a.1.pump_at(now), b.1.pump_at(now), sent)
}

/// [`step_pair`] extended to a third client, each named by its own address:
/// a spectator or other third role whose packets must not spill into either
/// paired client's queue the way a two-way router would.
pub fn step_trio(
    sv: &mut Server,
    a: (SocketAddr, &Rc<RefCell<Queues>>, &mut NetClient<ClientEnd>),
    b: (SocketAddr, &Rc<RefCell<Queues>>, &mut NetClient<ClientEnd>),
    c: (SocketAddr, &Rc<RefCell<Queues>>, &mut NetClient<ClientEnd>),
    now: Instant,
) -> (Vec<NetEvent>, Vec<NetEvent>, Vec<NetEvent>) {
    step_trio_ordered(sv, a, b, c, now, [0, 1, 2])
}

/// [`step_trio`] handing the three clients' packets to the server in
/// `order` (indices into `a`, `b`, `c`). A frame runs the menu threads of
/// its later packet first (`client-probes/probe_threadorder`), so clients
/// that answer their menus in one frame spawn in the reverse of `order`.
pub fn step_trio_ordered(
    sv: &mut Server,
    a: (SocketAddr, &Rc<RefCell<Queues>>, &mut NetClient<ClientEnd>),
    b: (SocketAddr, &Rc<RefCell<Queues>>, &mut NetClient<ClientEnd>),
    c: (SocketAddr, &Rc<RefCell<Queues>>, &mut NetClient<ClientEnd>),
    now: Instant,
    order: [usize; 3],
) -> (Vec<NetEvent>, Vec<NetEvent>, Vec<NetEvent>) {
    let peers = [(a.0, a.1), (b.0, b.1), (c.0, c.1)];
    for (addr, q) in order.map(|i| peers[i]) {
        let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
        for p in pending {
            sv.handle_packet(addr, &p, now);
        }
    }
    sv.tick(now);
    for (to, p) in sv.take_outgoing() {
        let q = if to == a.0 {
            a.1
        } else if to == b.0 {
            b.1
        } else {
            c.1
        };
        q.borrow_mut().to_client.push_back(p);
    }
    (a.2.pump_at(now), b.2.pump_at(now), c.2.pump_at(now))
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
    let (ca, cb, _, _) = join_pair_logged(sv, qa, qb, now, a, b);
    (ca, cb)
}

/// [`join_pair`] handing back the two [`Join`]s as well, for a test that has
/// to keep answering menus after the join: a new level opens them again.
pub fn join_pair_logged(
    sv: &mut Server,
    qa: &Rc<RefCell<Queues>>,
    qb: &Rc<RefCell<Queues>>,
    now: &mut Instant,
    a: (&str, &str),
    b: (&str, &str),
) -> (NetClient<ClientEnd>, NetClient<ClientEnd>, Join, Join) {
    join_pair_stamped(sv, qa, qb, now, a, b, &[])
}

/// [`join_pair_staggered`] with both clients connecting at once.
pub fn join_pair_stamped(
    sv: &mut Server,
    qa: &Rc<RefCell<Queues>>,
    qb: &Rc<RefCell<Queues>>,
    now: &mut Instant,
    a: (&str, &str),
    b: (&str, &str),
    stamps_a: &[i32],
) -> (NetClient<ClientEnd>, NetClient<ClientEnd>, Join, Join) {
    join_pair_staggered(sv, qa, qb, now, a, b, stamps_a, 0)
}

/// [`join_pair_logged`] with client `a`'s cmds, once it is in the game,
/// stamped `stamps_a` (a retail client's, on a server whose clock runs on
/// retail's level time): each frame sends those past its last and before
/// the frame's time. Empty, every client sends one cmd a frame on its own
/// clock.
///
/// Each frame goes through [`step_pair_joining`], so `a` spawns ahead of `b`.
///
/// `b` connects `b_after_ms` after `a` (a frame at a time), as a retail
/// client started later does; until then `a` joins and plays alone. A late
/// `b` is handed back a second after its spawn, not [`SPAWN_SETTLE`], so a
/// capture that starts soon after it can still be met.
#[allow(clippy::too_many_arguments)]
pub fn join_pair_staggered(
    sv: &mut Server,
    qa: &Rc<RefCell<Queues>>,
    qb: &Rc<RefCell<Queues>>,
    now: &mut Instant,
    a: (&str, &str),
    b: (&str, &str),
    stamps_a: &[i32],
    b_after_ms: i64,
) -> (NetClient<ClientEnd>, NetClient<ClientEnd>, Join, Join) {
    // Distinct qports: the server keys a peer by ip and qport, so two clients
    // sharing one read as a single client reconnecting and the second is
    // refused. A real client is one per process and gets this for free.
    let mut ca = NetClient::start_with_qport(ClientEnd(qa.clone()), *now, 0x2001);
    let (mut ja, mut jb) = (Join::new(a.0, a.1), Join::new(b.0, b.1));
    let mut last_a = i32::MIN;
    let mut waited = 0;
    while waited < b_after_ms {
        *now += Duration::from_millis(50);
        waited += 50;
        match ca.snapshots().newest().filter(|_| !stamps_a.is_empty()) {
            Some(snap) if ca.state() == NetState::Active => {
                let end = snap.server_time + FRAME_MS as i32;
                let cmds = stamped(&ca, &holding(&ca), stamps_a, last_a, end);
                if let Some(c) = cmds.last() {
                    last_a = c.server_time;
                }
                ca.send_cmds(&cmds);
            }
            _ => {
                ca.send_frame(&holding(&ca));
            }
        }
        for e in step(sv, qa, &mut ca, *now) {
            match e {
                NetEvent::ServerCommand(tokens) => ja.on_server_command(&tokens, &mut ca, *now),
                NetEvent::Dropped(r) => panic!("dropped mid-join: {r}"),
                _ => {}
            }
        }
    }
    let mut cb = NetClient::start_with_qport(ClientEnd(qb.clone()), *now, 0x2002);
    for _ in 0..600 {
        *now += Duration::from_millis(50);
        match ca.snapshots().newest().filter(|_| !stamps_a.is_empty()) {
            Some(snap) if ca.state() == NetState::Active => {
                let end = snap.server_time + FRAME_MS as i32;
                let cmds = stamped(&ca, &holding(&ca), stamps_a, last_a, end);
                if let Some(c) = cmds.last() {
                    last_a = c.server_time;
                }
                ca.send_cmds(&cmds);
            }
            _ => {
                ca.send_frame(&holding(&ca));
            }
        }
        cb.send_frame(&holding(&cb));
        let (ea, eb, _) = step_pair_joining(sv, (qa, &mut ca), (qb, &mut cb), *now);
        for (events, join, cl) in [(ea, &mut ja, &mut ca), (eb, &mut jb, &mut cb)] {
            for e in events {
                match e {
                    NetEvent::ServerCommand(tokens) => join.on_server_command(&tokens, cl, *now),
                    NetEvent::Dropped(r) => panic!("dropped mid-join: {r}"),
                    _ => {}
                }
            }
        }
        let b_wait = if b_after_ms > 0 {
            Duration::from_secs(1)
        } else {
            SPAWN_SETTLE
        };
        if ja.settled(*now) && jb.settled_after(*now, b_wait) {
            break;
        }
    }
    (ca, cb, ja, jb)
}

/// `cmd` once for every stamp in `stamps` past `after` and before `end`, in
/// order, its view rebased on the newest snapshot's `delta_angles` as
/// `send_frame` does.
pub fn stamped(
    cl: &NetClient<ClientEnd>,
    cmd: &UserCmd,
    stamps: &[i32],
    after: i32,
    end: i32,
) -> Vec<UserCmd> {
    let p = cl.protocol();
    let da = cl.snapshots().newest().map_or([0; 3], |s| {
        [0, 1, 2].map(|i| s.ps.field_i32(p, &format!("delta_angles[{i}]")))
    });
    let mut last = after;
    let mut out = Vec::new();
    for &st in stamps {
        if st > last && st < end {
            let mut c = *cmd;
            c.server_time = st;
            c.angles = [0, 1, 2].map(|i| (cmd.angles[i] - da[i]) & 0xffff);
            out.push(c);
            last = st;
        }
    }
    out
}

/// Three clients through the stock menus, as [`join_pair`] does two, at
/// [`ADDR`], [`ADDR_B`] and [`ADDR_C`]. Until all three are in, each frame's
/// packets reach the server in that order; from then on in the reverse, so
/// the three answer and spawn in slot order, as [`join_pair_stamped`] does.
pub fn join_trio(
    sv: &mut Server,
    q: [&Rc<RefCell<Queues>>; 3],
    now: &mut Instant,
    picks: [(&str, &str); 3],
) -> [NetClient<ClientEnd>; 3] {
    let mut cl: [NetClient<ClientEnd>; 3] = std::array::from_fn(|i| {
        NetClient::start_with_qport(ClientEnd(q[i].clone()), *now, 0x2001 + i as u16)
    });
    let mut joins: [Join; 3] = std::array::from_fn(|i| Join::new(picks[i].0, picks[i].1));
    for _ in 0..600 {
        *now += Duration::from_millis(50);
        for c in cl.iter_mut() {
            c.send_frame(&holding(c));
        }
        let entered = cl.iter().all(|c| c.state() == NetState::Active);
        let [a, b, c] = &mut cl;
        let events = step_trio_ordered(
            sv,
            (ADDR, q[0], a),
            (ADDR_B, q[1], b),
            (ADDR_C, q[2], c),
            *now,
            if entered { [2, 1, 0] } else { [0, 1, 2] },
        );
        for (i, events) in <[Vec<NetEvent>; 3]>::from(events).into_iter().enumerate() {
            for e in events {
                match e {
                    NetEvent::ServerCommand(tokens) => {
                        joins[i].on_server_command(&tokens, &mut cl[i], *now)
                    }
                    NetEvent::Dropped(r) => panic!("dropped mid-join: {r}"),
                    _ => {}
                }
            }
        }
        if joins.iter().all(|j| j.settled(*now)) {
            break;
        }
    }
    cl
}

/// Drives an already-built client through the connect handshake at `addr`
/// until its gamestate lands. Shared by [`connect`] and [`connect_at`], which
/// differ only in how the client is built (a real client is one per process
/// and gets a fine qport for free off `NetClient::start`; a harness running
/// several in one process needs a qport of its own per client instead).
fn run_connect(
    sv: &mut Server,
    addr: SocketAddr,
    q: &Rc<RefCell<Queues>>,
    now: &mut Instant,
    cl: &mut NetClient<ClientEnd>,
) {
    for _ in 0..40 {
        *now += Duration::from_millis(250);
        let events = step_at(sv, addr, q, cl, *now);
        if events.contains(&NetEvent::GamestateReady) {
            return;
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

pub fn connect(
    sv: &mut Server,
    q: &Rc<RefCell<Queues>>,
    now: &mut Instant,
) -> NetClient<ClientEnd> {
    let mut cl = NetClient::start(ClientEnd(q.clone()), *now);
    run_connect(sv, ADDR, q, now, &mut cl);
    cl
}

/// [`connect`] at an address and qport of the caller's choosing, for a
/// client sharing a process (and so a qport space) with others already
/// connected.
pub fn connect_at(
    sv: &mut Server,
    addr: SocketAddr,
    q: &Rc<RefCell<Queues>>,
    now: &mut Instant,
    qport: u16,
) -> NetClient<ClientEnd> {
    let mut cl = NetClient::start_with_qport(ClientEnd(q.clone()), *now, qport);
    run_connect(sv, addr, q, now, &mut cl);
    cl
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
        bots: 0,
        bots_shoot: false,
    }
}

/// What the moving-shot captures' servers changed in a stock weapon file
/// (`moveshot_ab.rs`, `vehicle_ab.rs`'s rider): no spread and no gun kick,
/// so a round leaves along the view from the muzzle. Keys a file lacks are
/// left out.
pub const STEADY_WEAPON: &[(&str, &str)] = &[
    ("adsSpread", "0"),
    ("adsSpreadProne", "0"),
    ("adsSpreadDucked", "0"),
    ("hipSpreadStandMin", "0"),
    ("hipSpreadDuckedMin", "0"),
    ("hipSpreadProneMin", "0"),
    ("hipSpreadMax", "0"),
    ("hipGunKickPitchMin", "0"),
    ("hipGunKickPitchMax", "0"),
    ("hipGunKickYawMin", "0"),
    ("hipGunKickYawMax", "0"),
    ("adsGunKickPitchMin", "0"),
    ("adsGunKickPitchMax", "0"),
    ("adsGunKickYawMin", "0"),
    ("adsGunKickYawMax", "0"),
];

/// Overlays `weapons/mp/<weapon>` with the stock file's
/// [`STEADY_WEAPON`] keys changed (`WEAPONFILE\key\value...`).
pub fn steady_weapon(fs: &mut vcod_common::pk3::Pk3Fs, weapon: &str) {
    let path = format!("weapons/mp/{weapon}");
    let stock = fs.read(&path).expect("the stock weapon file");
    let text = String::from_utf8_lossy(&stock);
    let mut parts: Vec<String> = text.split('\\').map(str::to_string).collect();
    let mut i = 1;
    while i + 1 < parts.len() {
        if let Some((_, v)) = STEADY_WEAPON.iter().find(|(k, _)| *k == parts[i]) {
            parts[i + 1] = v.to_string();
        }
        i += 2;
    }
    fs.overlay(&path, parts.join("\\").into_bytes());
}

/// How long a probe gate runs the server before its client connects. A
/// client probe's watcher sleeps a second before it polls for a playing
/// client, and retail's client joined seconds after the server started (each
/// fixture's header), so its body's idle clip, which runs from the spawn,
/// started on the frame the probe's schedule did. A client joined at once
/// spawns before the first poll and poses its body that much further into
/// the clip (cod11-movers.md 15).
pub const PROBE_JOIN_MS: i64 = 1500;

/// `sv` alone for `ms`, a frame at a time: the server's clock before a
/// client connects.
pub fn run_alone(sv: &mut Server, now: &mut Instant, ms: i64) {
    for _ in 0..ms / FRAME_MS {
        *now += Duration::from_millis(FRAME_MS as u64);
        sv.tick(*now);
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
    /// The playerstate the step ended at, as the field lines after its traces
    /// carry it. Only the fields a gate names are compared; a transient is
    /// not in here at all, since a settled sample cannot hold one.
    pub settled: BTreeMap<String, i32>,
    /// Where the replay puts the body before the step runs, origin and yaw;
    /// `None` leaves it where the step before ended. A gate sets it to
    /// retail's settled spot after a step that walked, since where a walk
    /// ends is not reproducible.
    pub place: Option<([f32; 3], f32)>,
    /// Every cmd retail's step sent once its clock ran (`!cmds`), `None` on
    /// a capture taken before the line existed.
    pub cmds: Option<CmdLog>,
}

/// A step's `!cmds` line: the newest snapshot's `serverTime` when the
/// step's clock started, each cmd's `serverTime`, and the `buttons` and
/// `wbuttons` of each cmd whose bits differ from the step's base.
pub struct CmdLog {
    pub start: i32,
    pub stamps: Vec<i32>,
    pub bits: BTreeMap<usize, (u8, u8)>,
    /// The weapon byte from each cmd on which it changes, the first
    /// against the step's `weapon`; `None` on a capture whose line carries
    /// no `w=`, which replays the step's switch on its own clock instead.
    pub weapons: Option<BTreeMap<usize, u8>>,
    /// The absolute pitch and yaw shorts from each cmd on which they
    /// change, the first always; `None` on a capture whose line carries no
    /// `a=`, whose cmds hold the step's base view. A walk's log always
    /// carries it: the probe's stall turns are only in the log.
    pub views: Option<BTreeMap<usize, [i32; 2]>>,
    /// `forward`, `right` and `up` from each cmd on which they change, the
    /// first always; `None` on a capture whose line carries no `m=`, whose
    /// cmds hold the step's base.
    pub moves: Option<BTreeMap<usize, [i8; 3]>>,
}

impl CmdLog {
    /// The weapon byte retail's cmd `i` carried, if the log has the column.
    fn weapon(&self, i: usize, step: &Step) -> Option<u8> {
        let w = self.weapons.as_ref()?;
        Some(w.range(..=i).next_back().map_or(step.weapon, |(_, b)| *b))
    }

    /// The absolute pitch and yaw retail's cmd `i` carried, if the log has
    /// the column.
    fn view(&self, i: usize) -> Option<[i32; 2]> {
        Some(*self.views.as_ref()?.range(..=i).next_back()?.1)
    }

    /// Sets cmd `i`'s view and movement: the logged ones where the log has
    /// them, else the step's base.
    fn aim(&self, i: usize, cmd: &mut UserCmd) {
        if let Some([pitch, yaw]) = self.view(i) {
            cmd.angles[0] = pitch;
            cmd.angles[1] = yaw;
        }
        if let Some((_, &[f, r, u])) = self.moves.as_ref().and_then(|m| m.range(..=i).next_back()) {
            (cmd.forward, cmd.right, cmd.up) = (f, r, u);
        }
    }
}

impl Step {
    /// The origin and yaw retail's playerstate settled at by the step's end,
    /// if its field lines carry them.
    pub fn settled_place(&self) -> Option<([f32; 3], f32)> {
        let f = |k: &str| self.settled.get(k).map(|v| f32::from_bits(*v as u32));
        Some((
            [f("origin[0]")?, f("origin[1]")?, f("origin[2]")?],
            f("viewangles[1]")?,
        ))
    }
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
    /// `ps.viewangles`, degrees, wire convention; `None` on a capture taken
    /// before the trace carried the three columns.
    pub viewangles: Option<[f32; 3]>,
    /// `pm_flags` (0x20 is the gated ADS bit) and `groundEntityNum`, the two
    /// the sight ramp is gated on; `None` on a capture taken before the trace
    /// carried them. Reported beside a sight miss, never compared on their
    /// own.
    pub pm_flags: Option<i32>,
    pub ground_entity: Option<i32>,
    /// The snapshot's `serverTime` and `ps.commandTime`; `None` on a capture
    /// taken before the trace carried them.
    pub server_time: Option<i32>,
    pub command_time: Option<i32>,
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
                settled: BTreeMap::new(),
                place: None,
                cmds: None,
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
        } else if let Some(rest) = line.strip_prefix("!cmds ") {
            let m = kv(rest);
            let mut st: i32 = m["st"].parse().unwrap();
            let mut stamps = vec![st];
            for d in m["d"].split(',').filter(|d| !d.is_empty()) {
                st += d.parse::<i32>().unwrap();
                stamps.push(st);
            }
            let bits = m
                .get("in")
                .map(|v| v.as_str())
                .unwrap_or("")
                .split(',')
                .filter(|c| !c.is_empty())
                .map(|c| {
                    let (i, b) = c.split_once(':').unwrap();
                    let (b, w) = b.split_once('/').unwrap();
                    (i.parse().unwrap(), (b.parse().unwrap(), w.parse().unwrap()))
                })
                .collect();
            let weapons = m.get("w").map(|v| {
                v.split(',')
                    .filter(|c| !c.is_empty())
                    .map(|c| {
                        let (i, w) = c.split_once(':').unwrap();
                        (i.parse().unwrap(), w.parse().unwrap())
                    })
                    .collect()
            });
            let views = m.get("a").map(|v| {
                v.split(',')
                    .filter(|c| !c.is_empty())
                    .map(|c| {
                        let (i, a) = c.split_once(':').unwrap();
                        let (p, y) = a.split_once('/').unwrap();
                        (i.parse().unwrap(), [p.parse().unwrap(), y.parse().unwrap()])
                    })
                    .collect()
            });
            let moves = m.get("m").map(|v| {
                v.split(',')
                    .filter(|c| !c.is_empty())
                    .map(|c| {
                        let (i, a) = c.split_once(':').unwrap();
                        let mut it = a.split('/').map(|x| x.parse::<i8>().unwrap());
                        let mv = [0; 3].map(|_| it.next().unwrap());
                        (i.parse().unwrap(), mv)
                    })
                    .collect()
            });
            step.cmds = Some(CmdLog {
                start: m["start"].parse().unwrap(),
                stamps,
                bits,
                weapons,
                views,
                moves,
            });
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
                viewangles: m.contains_key("viewangles[0]").then(|| {
                    [
                        f("viewangles[0]").unwrap(),
                        f("viewangles[1]").unwrap(),
                        f("viewangles[2]").unwrap(),
                    ]
                }),
                pm_flags: m.get("pm_flags").map(|v| v.parse().unwrap()),
                ground_entity: m.get("groundEntityNum").map(|v| v.parse().unwrap()),
                server_time: m.get("serverTime").map(|v| v.parse().unwrap()),
                command_time: m.get("commandTime").map(|v| v.parse().unwrap()),
            });
        } else if !line.starts_with('!') {
            // The settled field lines: `<name> <i32>`, the playerstate the
            // step ended at.
            if let Some((k, v)) = line.split_once(' ')
                && let Ok(v) = v.trim().parse::<i32>()
            {
                step.settled.insert(k.to_string(), v);
            }
        }
        // `!observed` is not compared here.
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
    replay_as(
        &vcod_common::net::protocol::PROTOCOL_V1,
        map,
        gametype,
        steps,
        join,
        fs,
        place,
    )
}

/// [`replay`] on a server speaking `proto`.
pub fn replay_as(
    proto: &'static vcod_common::net::protocol::Protocol,
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
    sv.set_protocol(proto);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs).expect("load the scripts");
    let q = std::rc::Rc::new(RefCell::new(Queues::default()));
    let (mut cl, _join) = self::join(&mut sv, &q, &mut now, join.0, join.1);
    // The client sends absolute view angles: `NetClient::send_frame`
    // subtracts the playerstate's `delta_angles` off them, so the spawn yaw
    // `place_client` leaves there is cancelled unless the capture's own yaw
    // is offset by it. A capture's `!input yaw` is relative to the facing its
    // header records.
    //
    // A header yaw is a wire view, an exact short printed as degrees, and
    // `* 65536 / 360` lands a hair either side of that short: truncated, a
    // negative one (-138.0048, short 40413) became 40414. Round it, and hand
    // `place_client`, which truncates as `ANGLE2SHORT` does, the short's
    // midpoint.
    let short_of = |yaw: f32| (yaw * 65536.0 / 360.0).round() as i32 & 0xffff;
    let mut yaw_short = 0;
    let put = |sv: &mut vcod_server::Server,
               cl: &mut vcod_common::net::NetClient<ClientEnd>,
               now: &mut Instant,
               (origin, yaw): ([f32; 3], f32)| {
        let short = short_of(yaw);
        sv.place_client(0, origin, (short as f32 + 0.5) * 360.0 / 65536.0);
        // `place_client`'s delta puts the view at `yaw` under a raw cmd
        // angle of 0, so the settle sends that raw 0 from the first frame,
        // past the snapshot that still carries the old delta: rebased off
        // that one, the first cmd and the next turn the view twice, and
        // UO's `hipSpreadTurnAdd` opens the cone where retail's spawn,
        // which turns no cmd, left it shut (cod11-combat.md 2.1).
        for _ in 0..4 {
            *now += Duration::from_millis(FRAME_MS as u64);
            cl.pump_at(*now);
            let settle = UserCmd {
                server_time: cl.server_clock_ms(),
                ..holding(cl)
            };
            cl.send_cmds(&[settle]);
            self::step(sv, &q, cl, *now);
        }
    };
    if let Some(place) = place {
        put(&mut sv, &mut cl, &mut now, place);
        yaw_short = short_of(place.1);
    }
    let p = proto;
    let mut out = Vec::new();
    // Cmds a logged step sent past the next step's start, with their
    // retail stamps; they run in the next step's first frames.
    let mut carry: Vec<(i32, UserCmd)> = Vec::new();
    // The logged step before's clock offset, which the next one keeps.
    let mut shift_carried: Option<i32> = None;
    for (si, step) in steps.iter().enumerate() {
        if let Some(place) = step.place {
            put(&mut sv, &mut cl, &mut now, place);
            yaw_short = short_of(place.1);
        }
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
        // A logged step's wait is in its log.
        if step.wait_ready && step.cmds.is_none() {
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
        // A step that logged its cmds replays them at retail's stamps on our
        // clock, so each tap lands where in the server frame retail's did:
        // retail's frame after `start` is ours after the step's newest
        // snapshot. Each frame runs the cmds retail's did, up to the
        // `commandTime` its snapshot read (a cmd can reach retail's server
        // after the frame its stamp puts it in), else those stamped before
        // the frame. The step ends on the last snapshot retail traced in it,
        // and what it logged past that runs in the next step, on the same
        // clock unless that one waits for the weapon first.
        if let Some(log) = &step.cmds {
            let open = cl.snapshots().newest().map_or(0, |s| s.server_time);
            let shift = shift_carried.unwrap_or(open - log.start);
            let first = log.stamps[0];
            let hands_on = steps.get(si + 1).is_some_and(|s| s.cmds.is_some());
            let end = step.trace.iter().filter_map(|t| t.server_time).max();
            // A step no logged step ran into opens on cmds the probe sent
            // before its clock started, which its log does not hold: its
            // traces' `commandTime`s name them. Run them on the held input,
            // or the first logged cmd's msec spans the gap and a press on it
            // lands in another chop of the move (`melee_tap`'s melee delay
            // read 34 ms short).
            if carry.is_empty() {
                let ours_ct = cl
                    .snapshots()
                    .newest()
                    .map_or(i32::MIN, |s| s.ps.field_i32(p, "commandTime"));
                let early: std::collections::BTreeSet<i32> = step
                    .trace
                    .iter()
                    .filter_map(|t| t.command_time)
                    .filter(|&ct| ct < first && ct > ours_ct - shift)
                    .collect();
                for ct in early {
                    let mut cmd = base;
                    // A walk stands still until its clock runs
                    // (`CombatProbe::cmd`): the probe sends the null cmd at
                    // the held view.
                    if step.walks {
                        (cmd.buttons, cmd.wbuttons) = (0, 0);
                        (cmd.forward, cmd.right, cmd.up) = (0, 0, 0);
                    }
                    cmd.weapon = weapon_byte(step, 0, held(&cl));
                    carry.push((ct, cmd));
                }
            }
            let mut next = 0;
            let mut frame = open;
            let more = |frame: i32, next: usize| {
                end.is_some_and(|e| frame - shift < e) || (!hands_on && next < log.stamps.len())
            };
            while more(frame, next) {
                frame += FRAME_MS as i32;
                let ran = step
                    .trace
                    .iter()
                    .find(|t| t.server_time == Some(frame - shift))
                    .and_then(|t| t.command_time)
                    .unwrap_or(frame - shift - 1);
                let ours_now = held(&cl);
                let da = cl.snapshots().newest().map_or([0; 3], |s| {
                    [0, 1, 2].map(|i| s.ps.field_i32(p, &format!("delta_angles[{i}]")))
                });
                let mut batch: Vec<(i32, UserCmd)> = Vec::new();
                while carry.first().is_some_and(|c| c.0 <= ran) {
                    batch.push(carry.remove(0));
                }
                while next < log.stamps.len() && log.stamps[next] <= ran {
                    let st = log.stamps[next];
                    let mut cmd = base;
                    cmd.weapon = log
                        .weapon(next, step)
                        .unwrap_or_else(|| weapon_byte(step, i64::from(st - first), ours_now));
                    (cmd.buttons, cmd.wbuttons) = log
                        .bits
                        .get(&next)
                        .copied()
                        .unwrap_or((base.buttons, base.wbuttons));
                    log.aim(next, &mut cmd);
                    batch.push((st, cmd));
                    next += 1;
                }
                let wire: Vec<UserCmd> = batch
                    .into_iter()
                    .map(|(st, mut cmd)| {
                        cmd.server_time = st + shift;
                        cmd.angles = [0, 1, 2].map(|i| (cmd.angles[i] - da[i]) & 0xffff);
                        cmd
                    })
                    .collect();
                now += Duration::from_millis(FRAME_MS as u64);
                cl.pump_at(now);
                cl.send_cmds(&wire);
                self::step(&mut sv, &q, &mut cl, now);
                let blasts = sv.pending_explosions().len();
                if let Some(s) = cl.snapshots().newest() {
                    samples.push(sample_of(s, i64::from(frame - open), blasts));
                }
            }
            // What retail's step sent past the next one's start, on the
            // next step's clock, which runs on this shift.
            let ours_now = held(&cl);
            for k in next..log.stamps.len() {
                let st = log.stamps[k];
                let mut cmd = base;
                cmd.weapon = log
                    .weapon(k, step)
                    .unwrap_or_else(|| weapon_byte(step, i64::from(st - first), ours_now));
                (cmd.buttons, cmd.wbuttons) = log
                    .bits
                    .get(&k)
                    .copied()
                    .unwrap_or((base.buttons, base.wbuttons));
                log.aim(k, &mut cmd);
                carry.push((st, cmd));
            }
            shift_carried = hands_on.then_some(shift);
            // Each sample pairs with retail's of the same frame and takes its
            // `ms`; one retail traced in another step is dropped.
            let retail: BTreeMap<i32, i64> = step
                .trace
                .iter()
                .filter_map(|t| Some((t.server_time?, t.ms)))
                .collect();
            if !retail.is_empty() {
                samples.retain_mut(|s| {
                    retail
                        .get(&(s.server_time - shift))
                        .map(|&ms| s.ms = ms)
                        .is_some()
                });
            }
            out.push(samples);
            continue;
        }
        carry.clear();
        shift_carried = None;
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
/// replay that has to throw from the same spot and look the same way.
/// `None` for a capture taken before either header existed.
///
/// The `# spawn` line is the general one every combat capture now carries;
/// the grenade header is the older form and stays readable, since the
/// committed grenade fixtures predate the other.
pub fn captured_place(text: &str) -> Option<([f32; 3], f32)> {
    let h = kv_header(text, "# spawn ").or_else(|| grenade_header(text))?;
    Some((header_vec3(&h, "origin"), header_vec3(&h, "viewangles")[1]))
}

/// A `# <tag> k=v k=v` header line as a map.
fn kv_header(text: &str, tag: &str) -> Option<BTreeMap<String, String>> {
    let line = text.lines().find(|l| l.starts_with(tag))?;
    Some(
        line[tag.len()..]
            .split_whitespace()
            .filter_map(|t| t.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
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

    fn send_answer(
        &mut self,
        cl: &mut NetClient<ClientEnd>,
        now: Instant,
        idx: i32,
        reply: &str,
        weapon_menu: bool,
    ) {
        cl.send_reliable(&format!("mr {} {idx} {reply}", cl.server_id()));
        if weapon_menu {
            self.answered_weapon_at = Some(now);
        } else {
            self.answered_team = true;
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
                // 1.5's stock gametypes open `serverinfo_<gametype>` first,
                // named by its configstring, and the team menu once it
                // answers `close` (cod15-protocol-diff.md 7).
                let first = cl
                    .protocol()
                    .cs
                    .bounds(vcod_common::net::protocol::CsRange::Menu)
                    .0;
                if usize::try_from(idx)
                    .is_ok_and(|i| cl.configstring(first + i).starts_with("serverinfo_"))
                {
                    cl.send_reliable(&format!("mr {} {idx} close", cl.server_id()));
                    self.answered.push(idx);
                    return;
                }
                // The menu `t` opens is the one its configstring names: UO
                // sends the weapon menu's `t` ahead of the
                // `g_scriptMainMenu` that names it.
                let named = usize::try_from(idx)
                    .map(|i| cl.configstring(first + i).to_string())
                    .unwrap_or_default();
                let menu = if named.is_empty() {
                    self.main_menu.clone()
                } else {
                    named
                };
                let reply = if menu.starts_with("team_") {
                    self.team.clone()
                } else if menu.starts_with("weapon_") {
                    self.weapon.clone()
                } else {
                    self.log.push(format!("menu {idx} ({menu:?}) has no reply"));
                    return;
                };
                let weapon_menu = menu.starts_with("weapon_");
                self.send_answer(cl, now, idx, &reply, weapon_menu);
                self.answered.push(idx);
                self.log
                    .push(format!("answered menu {idx} ({menu}) with {reply}"));
            }
            _ => {}
        }
    }

    /// A new level offers the stock menus again under the indices the last
    /// one already answered, so a client that crosses a gamestate has to
    /// forget what it answered or it sits on the team menu for the whole map.
    pub fn reset_menus(&mut self) {
        self.main_menu.clear();
        self.answered.clear();
        self.answered_team = false;
        self.answered_weapon_at = None;
    }

    pub fn settled(&self, now: Instant) -> bool {
        self.settled_after(now, SPAWN_SETTLE)
    }

    /// Whether the weapon menu was answered `wait` ago or more.
    pub fn settled_after(&self, now: Instant, wait: Duration) -> bool {
        self.answered_weapon_at
            .is_some_and(|t| now.duration_since(t) >= wait)
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
/// A client that connects and leaves again, the way each earlier probe run
/// left the capture's server: its `ClientDisconnect` frees entity 0, and
/// `G_FreeEntity`'s sweep reads 1023 into every `groundEntityNum` still at
/// `G_Spawn`'s 0 (protocol doc, "The static fields").
pub fn connect_and_leave(sv: &mut Server, q: &Rc<RefCell<Queues>>, now: &mut Instant) {
    let mut cl = connect(sv, q, now);
    cl.disconnect();
    for _ in 0..5 {
        *now += Duration::from_millis(50);
        step(sv, q, &mut cl, *now);
    }
}

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
        cl.send_frame(&holding(&cl));
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

// ------------------------------------------------------- the netchan capture

/// One line of a `--save-mapchange` / `--save-roundrestart` fixture, and the
/// same thing recorded off our own [`NetClient`]: what a client saw on the
/// wire either side of a map change or a round restart.
#[derive(Debug, Clone, PartialEq)]
pub struct NetchanEvent {
    /// Retail's is the probe's own clock, ours the frame grid; both count
    /// from the first line of their run.
    pub ms: i64,
    pub kind: NetchanKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NetchanKind {
    Gamestate {
        server_id: i32,
        mapname: String,
    },
    /// The serverCommand verbatim, `d` and `n` included.
    Cmd(String),
    /// Command word and body, newlines escaped as the fixture writes them.
    Oob(String),
    /// The four playerstate fields both gates read. The fixture's trace line
    /// carries eleven; the rest move every frame a client walks and would
    /// make a settled stretch unreadable.
    Trace {
        pm_type: i32,
        eflags: i32,
        health: i32,
        weapon: i32,
        /// The non-zero clips as `index:rounds`, comma-joined, `-` for none:
        /// the capture's own rendering.
        ammoclip: String,
    },
}

impl NetchanEvent {
    pub fn cmd(&self) -> Option<&str> {
        match &self.kind {
            NetchanKind::Cmd(t) => Some(t),
            _ => None,
        }
    }
    pub fn trace(&self) -> Option<&NetchanKind> {
        matches!(self.kind, NetchanKind::Trace { .. }).then_some(&self.kind)
    }
    pub fn pm_type(&self) -> Option<i32> {
        match self.kind {
            NetchanKind::Trace { pm_type, .. } => Some(pm_type),
            _ => None,
        }
    }
    pub fn eflags(&self) -> Option<i32> {
        match self.kind {
            NetchanKind::Trace { eflags, .. } => Some(eflags),
            _ => None,
        }
    }
}

/// Parses a `tests/fixtures/netchan/*.txt` capture. Header comments, the
/// `role` line and `!observed` are skipped; every other line is one event.
/// The file is already sorted by `(ms, gamestate < cmd < oob < trace)`, which
/// is the order [`record_netchan`] appends in.
pub fn parse_netchan_fixture(text: &str) -> Vec<NetchanEvent> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        // `key=value` up to the first space, except `text=`, which runs to
        // end of line and may hold spaces of its own.
        fn kv(rest: &str) -> BTreeMap<&str, &str> {
            rest.split_whitespace()
                .filter_map(|t| t.split_once('='))
                .collect()
        }
        fn tail(rest: &str, key: &str) -> String {
            rest.split_once(key)
                .map_or(String::new(), |(_, v)| v.into())
        }
        fn ms(rest: &str) -> i64 {
            kv(rest)["ms"].parse().expect("an ms on every line")
        }
        if let Some(rest) = line.strip_prefix("!gamestate ") {
            let m = kv(rest);
            out.push(NetchanEvent {
                ms: ms(rest),
                kind: NetchanKind::Gamestate {
                    server_id: m["serverId"].parse().unwrap(),
                    mapname: m["mapname"].to_string(),
                },
            });
        } else if let Some(rest) = line.strip_prefix("!cmd ") {
            out.push(NetchanEvent {
                ms: ms(rest),
                kind: NetchanKind::Cmd(tail(rest, "text=")),
            });
        } else if let Some(rest) = line.strip_prefix("!oob ") {
            out.push(NetchanEvent {
                ms: ms(rest),
                kind: NetchanKind::Oob(tail(rest, "text=")),
            });
        } else if let Some(rest) = line.strip_prefix("!trace ") {
            let m = kv(rest);
            let i = |k: &str| m[k].parse::<i32>().unwrap();
            out.push(NetchanEvent {
                ms: ms(rest),
                kind: NetchanKind::Trace {
                    pm_type: i("pm_type"),
                    eflags: i("eFlags"),
                    health: i("health"),
                    weapon: i("weapon"),
                    ammoclip: m["ammoclip"].to_string(),
                },
            });
        }
    }
    out
}

/// Appends this frame's wire to `out`, in the order the fixture writer sorts
/// one `ms` into: the gamestate, then every serverCommand verbatim, then the
/// out-of-band packets, then one trace.
///
/// The trace is unconditional; a capture keeps only the snapshots whose
/// watched fields moved, so both sides go through [`collapse_traces`] before
/// they are compared.
pub fn record_netchan(
    cl: &mut NetClient<ClientEnd>,
    events: &[NetEvent],
    now_ms: i64,
    out: &mut Vec<NetchanEvent>,
) {
    let mut push = |kind| out.push(NetchanEvent { ms: now_ms, kind });
    for e in events {
        if *e == NetEvent::GamestateReady {
            push(NetchanKind::Gamestate {
                server_id: cl.server_id(),
                mapname: vcod_common::net::info_value_for_key(cl.configstring(0), "mapname")
                    .unwrap_or_default()
                    .to_string(),
            });
        }
    }
    // Verbatim, off the reliable ring: `d` and `n` never reach `NetEvent`,
    // and `d` is what carries the restart's new `sv_serverid`.
    for text in cl.take_server_commands() {
        push(NetchanKind::Cmd(text));
    }
    for e in events {
        if let NetEvent::OutOfBand(text) = e {
            push(NetchanKind::Oob(text.replace('\n', "\\n")));
        }
    }
    if let Some(s) = cl.snapshots().newest() {
        let p = &vcod_common::net::protocol::PROTOCOL_V1;
        let clips: Vec<String> = (0..64)
            .filter(|&i| s.ps.clip(i) != 0)
            .map(|i| format!("{i}:{}", s.ps.clip(i)))
            .collect();
        push(NetchanKind::Trace {
            pm_type: s.ps.field_i32(p, "pm_type"),
            eflags: s.ps.field_i32(p, "eFlags"),
            health: s.ps.health(),
            weapon: s.ps.field_i32(p, "weapon"),
            ammoclip: if clips.is_empty() {
                "-".to_string()
            } else {
                clips.join(",")
            },
        });
    }
}

/// Drops every trace whose four fields repeat the previous one, which is what
/// a capture's own `keep_sample` does bar its once-a-second heartbeat. Both
/// sides go through it, so the heartbeat lines and our per-frame traces
/// collapse to the same transitions.
pub fn collapse_traces(events: &[NetchanEvent]) -> Vec<NetchanEvent> {
    let mut out: Vec<NetchanEvent> = Vec::new();
    let mut last: Option<NetchanKind> = None;
    for e in events {
        if let NetchanKind::Trace { .. } = e.kind {
            if last.as_ref() == Some(&e.kind) {
                continue;
            }
            last = Some(e.kind.clone());
        }
        out.push(e.clone());
    }
    out
}

/// A usercmd holding the weapon the playerstate says the client holds. A
/// `cmd.weapon` of 0 is not neutral: retail reads a byte differing from
/// `ps.weapon` as a request to holster (`docs/research/cod11-combat.md`,
/// section 1.8), so a replay that sent `NULL_USERCMD` would be asking for a
/// putaway on every frame.
pub fn holding(cl: &NetClient<ClientEnd>) -> UserCmd {
    let p = cl.protocol();
    UserCmd {
        weapon: cl
            .snapshots()
            .newest()
            .map_or(0, |s| s.ps.field_i32(p, "weapon") as u8),
        ..NULL_USERCMD
    }
}

/// `cmd` with a weapon byte of 0 filled with the weapon the newest snapshot
/// says `cl` holds, as a client echoing `ps.weapon` sends it ([`holding`]).
/// For the hand-written cmds that predate pmove reading a 0 as retail does;
/// a test that means a 0 sends its cmd directly.
pub fn echo_weapon(cl: &NetClient<ClientEnd>, cmd: &UserCmd) -> UserCmd {
    if cmd.weapon != 0 {
        return *cmd;
    }
    UserCmd {
        weapon: holding(cl).weapon,
        ..*cmd
    }
}

/// `sv_serverid` off a `d 1 \...\sv_serverid\17\...` configstring update.
pub fn serverid_of(cmd: &str) -> Option<i32> {
    let info = cmd.strip_prefix("d 1 ")?;
    vcod_common::net::info_value_for_key(info, "sv_serverid")?
        .parse()
        .ok()
}

// ------------------------------------------------------- the S&D captures

/// One `!trace` line of a `--probe-plant` / `--probe-defuse` fixture: the
/// movement fields, the sweep's offset, the two HUD flags the probe computed
/// and block 4 as the wire carried it.
#[derive(Debug, Clone)]
pub struct SdTrace {
    pub ms: i64,
    pub server_time: i32,
    pub buttons: u8,
    pub forward: i8,
    pub pm_type: i32,
    pub ground: i32,
    pub origin: [f32; 3],
    pub velocity: [f32; 3],
    pub viewangles: [f32; 3],
    pub yaw_off: f32,
    pub pitch_off: f32,
    /// `pitchApplied`; `None` on a capture taken before the column existed.
    pub pitch_applied: Option<f32>,
    pub icon: bool,
    pub bar: bool,
    /// The HUD column verbatim, for a failure message.
    pub hud: String,
    /// Slot -> (state, icon, entNum, teamNum, origin), non-empty slots only.
    pub objectives: BTreeMap<usize, (i32, i32, i32, i32, [f32; 3])>,
}

pub struct SdPhase {
    pub name: String,
    /// Every usercmd sent during the phase with its `st`, wire words.
    pub cmds: Vec<(i32, UserCmd)>,
    pub traces: Vec<SdTrace>,
}

pub struct SdFixture {
    /// `# station origin=… viewangles=…`: where the replayable part starts.
    pub station: ([f32; 3], [f32; 3]),
    pub phases: Vec<SdPhase>,
}

impl SdFixture {
    pub fn phase(&self, name: &str) -> &SdPhase {
        self.phases
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("no [phase {name}] in the fixture"))
    }
}

fn parse_vec3(s: &str) -> [f32; 3] {
    let mut out = [0.0; 3];
    for (i, v) in s.split(',').enumerate().take(3) {
        out[i] = v.parse().unwrap_or_else(|_| panic!("a vector, got {s:?}"));
    }
    out
}

pub fn parse_sd_fixture(text: &str) -> SdFixture {
    let station = kv_header(text, "# station ").expect("a `# station` header line");
    let mut fx = SdFixture {
        station: (
            header_vec3(&station, "origin"),
            header_vec3(&station, "viewangles"),
        ),
        phases: Vec::new(),
    };
    for line in text.lines() {
        let line = line.trim_end();
        if let Some(name) = line
            .strip_prefix("[phase ")
            .and_then(|l| l.strip_suffix(']'))
        {
            fx.phases.push(SdPhase {
                name: name.to_string(),
                cmds: Vec::new(),
                traces: Vec::new(),
            });
            continue;
        }
        let Some(phase) = fx.phases.last_mut() else {
            continue;
        };
        fn kv(rest: &str) -> BTreeMap<&str, &str> {
            rest.split_whitespace()
                .filter_map(|t| t.split_once('='))
                .collect()
        }
        if let Some(rest) = line.strip_prefix("!cmd ") {
            let m = kv(rest);
            let i = |k: &str| m[k].parse::<i64>().unwrap();
            let angles = parse_vec3(m["angles"]);
            phase.cmds.push((
                i("st") as i32,
                UserCmd {
                    buttons: i("buttons") as u8,
                    wbuttons: i("wbuttons") as u8,
                    weapon: i("weapon") as u8,
                    up: i("up") as i8,
                    forward: i("forward") as i8,
                    right: i("right") as i8,
                    angles: [angles[0] as i32, angles[1] as i32, angles[2] as i32],
                    ..NULL_USERCMD
                },
            ));
        } else if let Some(rest) = line.strip_prefix("!trace ") {
            // `hud=` runs to `objectives=`, spaces inside.
            let (head, objectives) = rest.split_once(" objectives=").unwrap_or((rest, ""));
            let (head, hud) = head.split_once(" hud=").unwrap_or((head, ""));
            let m = kv(head);
            let i = |k: &str| m[k].parse::<i64>().unwrap();
            let f = |k: &str| m[k].parse::<f32>().unwrap();
            // An empty block reads `-`.
            let objectives = objectives
                .split_whitespace()
                .filter(|slot| *slot != "-")
                .map(|slot| {
                    let mut it = slot.splitn(6, ':');
                    let idx: usize = it.next().unwrap().parse().unwrap();
                    let mut n = || it.next().unwrap().parse::<i32>().unwrap();
                    let (state, icon, ent, team) = (n(), n(), n(), n());
                    (
                        idx,
                        (state, icon, ent, team, parse_vec3(it.next().unwrap())),
                    )
                })
                .collect();
            phase.traces.push(SdTrace {
                ms: i("ms"),
                server_time: i("serverTime") as i32,
                buttons: i("buttons") as u8,
                forward: i("forward") as i8,
                pm_type: i("pm_type") as i32,
                ground: i("groundEntityNum") as i32,
                origin: parse_vec3(m["origin"]),
                velocity: parse_vec3(m["velocity"]),
                viewangles: parse_vec3(m["viewangles"]),
                yaw_off: f("yawOffset"),
                pitch_off: f("pitchOffset"),
                pitch_applied: m.get("pitchApplied").map(|v| v.parse().unwrap()),
                icon: i("icon") != 0,
                bar: i("bar") != 0,
                hud: hud.to_string(),
                objectives,
            });
        }
    }
    fx
}

/// What `vcod --probe-uo-weapon hold` (and `--probe-vehicle`'s on-foot
/// steps) send on a UO server (`vehicle_probe.rs`): a cmd every
/// `--probe-cmd-ms` 16 of the client's own clock, the view it first saw
/// once spawned held and rebased on the newest snapshot's
/// `delta_angles`. A gsc `setPlayerAngles` writes the view and
/// `delta_angles` against the last cmd, so here a frame's first cmd aims
/// along the view the gsc left (the aim block reads the view ahead of
/// `Pmove`) and the later ones along the held view. Over a real link the
/// rebase lags the gsc's writes and the rounds land between the two views
/// by the client's timing (docs/research/coduo-vehicles.md 13); `aim`
/// (`--probe-uo-weapon aim`) sends every cmd at the newest snapshot's view
/// instead, which a gsc steers round for round.
#[derive(Default)]
pub struct ProbeHold {
    /// The held view, degrees; `None` until [`ProbeHold::cmds`] is first
    /// asked once spawned and not following.
    pub base: Option<[f32; 3]>,
    /// The last cmd's `serverTime`.
    pub last: i32,
    /// `--probe-vehicle`'s steps before the use tap: each cmd takes the
    /// newest snapshot's view instead of a held one.
    pub follow: bool,
    /// The forward key the cmds hold.
    pub forward: i8,
    /// `--probe-uo-weapon aim`: every cmd at the newest snapshot's view.
    pub aim: bool,
    /// Cmds 16, 16 and 18 ms apart, the last on a frame's time and sent
    /// after that frame has run, as the net probe's ran in the captures
    /// that log them (`CMDS` lines), in place of an even [`PROBE_CMD_MS`]:
    /// a drop's airborne prone body is the one its last airborne cmd
    /// starts at (cod15-movement.md 4).
    pub framed: bool,
    /// Cmds on a 60 Hz grid of the server's clock, 16, 17 and 17 ms apart
    /// from each frame's time, as the net probe's loop sends them in the
    /// `--probe-uo-weapon` captures that log them (`!cmd` lines), in place
    /// of an even [`PROBE_CMD_MS`]: a held trigger fires on the cmds, and
    /// the thompson's 85 ms fires every sixth one, 100 ms apart here and 96
    /// at 16 ms, which is what decides whether the 1.5 and UO gun kick
    /// settles between rounds (cod15-movement.md 6).
    pub sixty: bool,
    /// The `serverTime`s a capture's client sent (its `CMDS` lines, sorted):
    /// under [`ProbeHold::framed`] a frame's cmds are the captured ones in
    /// the 50 ms from its time, where there are any. A `setOrigin` drop
    /// lands where its cmds' split stops it (cod11-combat.md 3.6).
    pub times: Vec<i32>,
    /// Each captured snapshot's time and `commandTime` (the `CMDS` lines'
    /// `ct=`, sorted), the last cmd retail ran before that frame: under
    /// [`ProbeHold::framed`] the cmds sent on a snapshot are the captured
    /// ones past its `commandTime` up to the next snapshot's, which places
    /// each cmd on retail's side of a frame, in place of the 50 ms from
    /// the frame's time (cod11-combat.md 15.1, "The cmd that fires").
    pub ran: Vec<(i32, i32)>,
    /// The newest snapshot's time, view and `delta_angles` at the last
    /// [`ProbeHold::cmds`], and the snapshot's before it: under
    /// [`ProbeHold::ran`] a cmd stamped before the newest went out on the
    /// older one.
    pub seen: [Option<SeenSnap>; 2],
}

/// A snapshot's time, view and `delta_angles`, as [`ProbeHold::seen`] keeps it.
pub type SeenSnap = (i32, [f32; 3], [i32; 3]);

/// A cmd's buttons, `wbuttons` and `up`.
pub type CmdInput = (u8, u8, i8);

/// Each cmd of a client's `CMDS` lines, those prefixed `who` only, in the
/// order it went out: its `serverTime` and the input an `in=<k>:<buttons>:
/// <wbuttons>:<up>` set on it, which the cmds after it went out with too.
fn cmds_sent(text: &str, who: &str) -> Vec<(i32, Option<CmdInput>)> {
    let mut out = Vec::new();
    for l in text.lines() {
        let Some(rest) = l
            .strip_prefix(who)
            .and_then(|r| r.trim_start().strip_prefix("CMDS st="))
        else {
            continue;
        };
        let mut f = rest.split_whitespace();
        let mut times = vec![
            f.next()
                .and_then(|t| t.parse::<i32>().ok())
                .expect("a CMDS start"),
        ];
        let mut ins = BTreeMap::new();
        for tok in f {
            if let Some(d) = tok.strip_prefix("d=") {
                for d in d.split(',').filter(|d| !d.is_empty()) {
                    times.push(times.last().unwrap() + d.parse::<i32>().expect("a CMDS step"));
                }
            } else if let Some(i) = tok.strip_prefix("in=") {
                for e in i.split(',') {
                    let v: Vec<i32> = e.split(':').map(|x| x.parse().expect("an input")).collect();
                    ins.insert(v[0] as usize, (v[1] as u8, v[2] as u8, v[3] as i8));
                }
            }
        }
        out.extend(
            times
                .iter()
                .enumerate()
                .map(|(k, &t)| (t, ins.get(&k).copied())),
        );
    }
    out
}

/// The cmds of [`cmds_sent`] the server runs: a cmd no newer than one
/// sent before it is skipped (`SV_UserMove`), and the net probe sends
/// those, a stamp off a snapshot's arrival a ms under its clock's last
/// one, or the same time twice.
fn cmds_run(text: &str, who: &str) -> Vec<(i32, CmdInput)> {
    let (mut out, mut input, mut newest) = (Vec::new(), (0, 0, 0), i32::MIN);
    for (t, set) in cmds_sent(text, who) {
        input = set.unwrap_or(input);
        if t > newest {
            newest = t;
            out.push((t, input));
        }
    }
    out
}

/// The input changes of a client's `CMDS` lines, keyed by the `serverTime`
/// of the first cmd the server runs with each ([`cmds_run`]).
pub fn cmds_inputs(text: &str, who: &str) -> Vec<(i32, CmdInput)> {
    let mut out = Vec::new();
    let mut last = None;
    let changed = cmds_sent(text, who).iter().any(|c| c.1.is_some());
    for (t, input) in cmds_run(text, who) {
        if changed && last != Some(input) {
            out.push((t, input));
            last = Some(input);
        }
    }
    out
}

/// The cmd times of a client's `CMDS` lines (the net probe's
/// `--probe-fall` and `--probe-uo-weapon` stdout) the server runs
/// ([`cmds_run`]), each line a first `serverTime` and the steps after it.
pub fn cmds_times(text: &str, who: &str) -> Vec<i32> {
    cmds_run(text, who).into_iter().map(|c| c.0).collect()
}

/// Each cmd the server runs of a client's `CMDS` lines ([`cmds_run`]) and
/// the probe's wall clock at its send in microseconds (the `w=` token, the
/// first absolute and the steps after it), which orders two probes'
/// packets on one host as the server read them. Empty for a capture that
/// logs no `w=`.
pub fn cmds_walls(text: &str, who: &str) -> Vec<(i32, u64)> {
    let mut walls = Vec::new();
    for l in text.lines() {
        let Some(rest) = l
            .strip_prefix(who)
            .and_then(|r| r.trim_start().strip_prefix("CMDS st="))
        else {
            continue;
        };
        let Some(w) = rest.split_whitespace().find_map(|t| t.strip_prefix("w=")) else {
            return Vec::new();
        };
        let mut at = 0u64;
        for (k, d) in w.split(',').enumerate() {
            let d: u64 = d.parse().expect("a w= step");
            at = if k == 0 { d } else { at + d };
            walls.push(at);
        }
    }
    let sent = cmds_sent(text, who);
    assert_eq!(sent.len(), walls.len(), "{who}: a w= per cmd");
    let mut newest = i32::MIN;
    let mut out = Vec::new();
    for ((t, _), w) in sent.into_iter().zip(walls) {
        if t > newest {
            newest = t;
            out.push((t, w));
        }
    }
    out
}

/// Each cmd of a client's `CMDS` lines in the order it went out, its
/// `serverTime` and the netchan sequence of its message (the `q=` token,
/// the first absolute and the steps after it). Empty for a capture that
/// logs no `q=`.
fn cmds_seqs(text: &str, who: &str) -> Vec<(i32, u32)> {
    let mut seqs = Vec::new();
    for l in text.lines() {
        let Some(rest) = l
            .strip_prefix(who)
            .and_then(|r| r.trim_start().strip_prefix("CMDS st="))
        else {
            continue;
        };
        let Some(q) = rest.split_whitespace().find_map(|t| t.strip_prefix("q=")) else {
            return Vec::new();
        };
        let mut at = 0u32;
        for (k, d) in q.split(',').enumerate() {
            let d: u32 = d.parse().expect("a q= step");
            at = if k == 0 { d } else { at.wrapping_add(d) };
            seqs.push(at);
        }
    }
    let sent = cmds_sent(text, who);
    assert_eq!(sent.len(), seqs.len(), "{who}: a q= per cmd");
    sent.into_iter().map(|c| c.0).zip(seqs).collect()
}

/// Every cmd the server ran of the clients `who` (in their slot order),
/// as `(index into who, serverTime)`, in the order retail read their
/// packets: the capture's `RECV` lines (`tools/recv_order.sh`, retail's
/// `showpackets 1` print, docs/dev/retail.md "Receive order") matched to
/// each client's `q=` sequences. A sequence matches the client whose next
/// unread cmds hold it, once that client's slot has connected (`c<n>`); a
/// cmd skipped over was never read, one the netchan dropped (`o<s>`) never
/// ran, and one no newer than the client's newest run is skipped
/// (`SV_UserMove`). Empty for a capture without `RECV` or `q=` lines.
pub fn cmds_read(text: &str, who: &[&str]) -> Vec<(usize, i32)> {
    /// How many unread cmds a match may skip, the packets lost before it.
    const LOST: usize = 8;
    let sent: Vec<Vec<(i32, u32)>> = who.iter().map(|w| cmds_seqs(text, w)).collect();
    let tokens: Vec<&str> = text
        .lines()
        .filter_map(|l| l.strip_prefix("RECV "))
        .flat_map(|l| l.split(','))
        .collect();
    if tokens.is_empty() || sent.iter().any(Vec::is_empty) {
        return Vec::new();
    }
    let n = who.len();
    let (mut next, mut live, mut newest) = (vec![0; n], vec![false; n], vec![i32::MIN; n]);
    let mut out = Vec::new();
    // The entry the last print pushed, which an `o<s>` after it takes back.
    let mut last: Option<(usize, i32)> = None;
    for tok in tokens {
        if let Some(slot) = tok.strip_prefix('c') {
            let slot: usize = slot.parse().expect("a connect slot");
            if slot < n {
                live[slot] = true;
            }
            continue;
        }
        if tok.starts_with('o') {
            if let Some((k, prev)) = last.take() {
                assert_eq!(out.pop(), Some((k, newest[k])), "a dropped packet's cmd");
                newest[k] = prev;
            }
            continue;
        }
        if tok.starts_with('d') {
            continue;
        }
        let s: u32 = tok.parse().expect("a RECV sequence");
        let hits: Vec<(usize, usize)> = (0..n)
            .filter(|&k| live[k])
            .filter_map(|k| {
                let end = (next[k] + LOST).min(sent[k].len());
                (next[k]..end).find(|&j| sent[k][j].1 == s).map(|j| (k, j))
            })
            .collect();
        assert!(hits.len() < 2, "RECV {s} matches two clients: {hits:?}");
        last = None;
        if let Some(&(k, j)) = hits.first() {
            next[k] = j + 1;
            let t = sent[k][j].0;
            if t > newest[k] {
                last = Some((k, newest[k]));
                newest[k] = t;
                out.push((k, t));
            }
        }
    }
    out
}

/// The `ct=<serverTime>:<commandTime>` pairs of a client's `CMDS` lines,
/// those prefixed `who` only, sorted by time.
pub fn cmds_ran(text: &str, who: &str) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> = text
        .lines()
        .filter_map(|l| l.strip_prefix(who))
        .filter(|r| r.trim_start().starts_with("CMDS st="))
        .flat_map(|r| r.split_whitespace())
        .filter_map(|tok| tok.strip_prefix("ct="))
        .flat_map(|list| list.split(','))
        .map(|e| {
            let (t, ct) = e.split_once(':').expect("a ct= pair");
            (
                t.parse().expect("a snapshot time"),
                ct.parse().expect("a commandTime"),
            )
        })
        .collect();
    out.sort_unstable();
    out
}

/// `--probe-cmd-ms`'s default.
pub const PROBE_CMD_MS: i32 = 16;

impl ProbeHold {
    /// The cmds of one 50 ms frame, up to a frame past the newest
    /// snapshot's time, with `buttons` held.
    pub fn cmds(&mut self, cl: &NetClient<ClientEnd>, buttons: u8) -> Vec<UserCmd> {
        let p = cl.protocol();
        let Some(s) = cl.snapshots().newest() else {
            return Vec::new();
        };
        let v = match self.aim {
            true => s.ps.viewangles(p),
            false => *self.base.get_or_insert_with(|| s.ps.viewangles(p)),
        };
        let da =
            ["delta_angles[0]", "delta_angles[1]", "delta_angles[2]"].map(|n| s.ps.field_i32(p, n));
        let mut t = self.last.max(s.server_time - FRAME_MS as i32);
        let mut times = Vec::new();
        if self.framed {
            let f = s.server_time;
            // The `commandTime` of the newest snapshot logged at or before
            // `t`, inside the logged span: a stall that kept snapshots off
            // the client also kept its cmds off the server, so a frame in
            // the gap ran none of them.
            let ct = |t: i32| {
                let last = self.ran.last()?.0;
                let i = self.ran.partition_point(|e| e.0 <= t);
                (i > 0 && t <= last).then(|| self.ran[i - 1].1)
            };
            // A snapshot pair retail logged sends what retail ran between
            // the two, none included.
            let ran = ct(f).zip(ct(f + FRAME_MS as i32));
            let captured = match ran {
                Some((lo, hi)) => {
                    let lo = self.times.partition_point(|&c| c <= lo);
                    let hi = self.times.partition_point(|&c| c <= hi);
                    &self.times[lo..hi.max(lo)]
                }
                None => {
                    let lo = self.times.partition_point(|&c| c < f);
                    let hi = self.times.partition_point(|&c| c < f + FRAME_MS as i32);
                    &self.times[lo..hi]
                }
            };
            if captured.is_empty() && ran.is_none() {
                times.extend([f, f + 16, f + 32].into_iter().filter(|&c| c > t));
            } else {
                // Cmds a stall held back go out late, older than a frame.
                if ran.is_some() {
                    t = self.last;
                }
                times.extend(captured.iter().copied().filter(|&c| c > t));
            }
        } else {
            let next = |t: i32| match self.sixty {
                true => {
                    let tick = |k: i32| (50 * k).div_euclid(3);
                    let mut k = (3 * t).div_euclid(50);
                    while tick(k) <= t {
                        k += 1;
                    }
                    tick(k)
                }
                false => t + PROBE_CMD_MS,
            };
            while next(t) <= s.server_time + FRAME_MS as i32 {
                t = next(t);
                times.push(t);
            }
        }
        // A cmd stamped before the newest snapshot's time went out on the
        // snapshot before it, its view and `delta_angles`.
        if self.seen[0].is_none_or(|(st, _, _)| st != s.server_time) {
            self.seen = [Some((s.server_time, v, da)), self.seen[0]];
        }
        let mut out = Vec::new();
        for &t in &times {
            let (v, da) = match self.seen[1] {
                Some((_, ov, oda)) if !self.ran.is_empty() && t < s.server_time => (ov, oda),
                _ => (v, da),
            };
            out.push(UserCmd {
                server_time: t,
                angles: std::array::from_fn(|i| {
                    ((v[i] * 65536.0 / 360.0).round() as i32 - da[i]) & 0xffff
                }),
                buttons,
                weapon: s.ps.field_i32(p, "weapon") as u8,
                forward: self.forward,
                ..UserCmd::default()
            });
        }
        self.last = times.last().copied().unwrap_or(t);
        out
    }
}

/// A map's `mp_*spawn*` entities as `(origin, angles)`.
pub fn spawn_points(fs: &vcod_common::pk3::Pk3Fs, map: &str) -> Vec<([f32; 3], [f32; 3])> {
    let path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&path).unwrap()).unwrap();
    vcod_common::bsp::entity_blocks(&bsp.entities)
        .iter()
        .filter(|e| {
            e.get("classname")
                .is_some_and(|c| c.starts_with("mp_") && c.contains("spawn"))
        })
        .filter_map(|e| {
            let origin = vcod_common::bsp::parse_vec3(e.get("origin")?)?;
            let angles = match (e.get("angles"), e.get("angle")) {
                (Some(a), _) => vcod_common::bsp::parse_vec3(a)?,
                (None, Some(y)) => [0.0, y.parse().ok()?, 0.0],
                (None, None) => [0.0; 3],
            };
            Some((origin, angles))
        })
        .collect()
}
