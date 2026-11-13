//! `StuckInClient`'s push against the retail overlap captures. Retail's side
//! is read straight off `--save-bump --capture-tag overlap*` (the walker's
//! playerstate and the target's entity per snapshot) and the target's own
//! playerstate lines its probe printed into the gsc log; ours is the same
//! placements on the real server. Both are held to the same properties:
//!
//! - the first snapshot after an overlap carries `pm_time` in (250, 300]
//!   and `pm_flags` 0x100 on both players;
//! - a pushed player moves at 190; with both still both are pushed, with one
//!   walking the still one takes 0;
//! - the higher slot writes last, so its direction is the separation plus a
//!   jitter in [1, 3) per axis, and the lower slot's is the opposite;
//! - the target's wire `solid` reads 0 on the snapshot after each push and
//!   its packed value otherwise;
//! - a spectator in slot 0 means no push at all on 1.1; 1.5's scan passes
//!   over him, and the 1.5 rig replays the push's `rand()` from retail's
//!   seed.
//!
//! The pure test also runs our `stuck_in_client` on every snapshot of the
//! bump capture, at retail's own positions, and it fires exactly where
//! retail's did. `STUCK_REPORT=1` prints the first frames of each overlap,
//! ours and retail's. The server half needs `COD_DIR`, at a 1.1 install
//! for the 1.1 captures and a 1.5 one for `-15`; the rest runs anywhere.

use crate::common;

use common::{ClientEnd, Join, Queues};
use glam::{Vec2, Vec3};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::movetrace::{Body, CONTENTS_BODY};
use vcod_common::net::msg::{NULL_USERCMD, UserCmd};
use vcod_common::net::protocol::PROTOCOL_V6;
use vcod_common::net::{NetClient, NetEvent};
use vcod_common::pk3::Pk3Fs;
use vcod_server::Server;
use vcod_server::game::stuck::{StuckRules, StuckView, stuck_in_client};

const PMF_TIME_KNOCKBACK: i32 = 0x100;
const PUSH_SPEED: f32 = 190.0;
const STAND_SOLID: i32 = 6684943;

/// One player's state at the end of a server frame, from whichever side saw
/// it.
#[derive(Clone, Copy, Debug)]
struct Seen {
    origin: Vec3,
    vel: Vec2,
    pm_flags: i32,
    pm_time: i32,
}

impl Seen {
    fn pushed_now(&self) -> bool {
        self.pm_flags & PMF_TIME_KNOCKBACK != 0 && self.pm_time == 300
    }
}

/// One end frame of a pair: the higher slot (the walker) and the lower one
/// (the target), and the target's `solid` as the walker was sent it.
#[derive(Clone, Copy, Debug)]
struct Frame {
    t: i32,
    walker: Seen,
    target: Option<Seen>,
    target_solid: i32,
}

// ------------------------------------------------------------------ retail

fn kv(rest: &str) -> BTreeMap<&str, &str> {
    rest.split_whitespace()
        .filter_map(|kv| kv.split_once('='))
        .collect()
}

fn vec3(s: &str) -> Vec3 {
    let v: Vec<f32> = s.split(',').map(|x| x.parse().unwrap()).collect();
    Vec3::new(v[0], v[1], v[2])
}

fn seen(kv: &BTreeMap<&str, &str>) -> Seen {
    let vel = vec3(kv["vel"]);
    Seen {
        origin: vec3(kv["origin"]),
        vel: vel.truncate(),
        pm_flags: kv["pm_flags"].parse().unwrap(),
        pm_time: kv["pm_time"].parse().unwrap(),
    }
}

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/playerstate/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The walker's snapshots, each with the target's own playerstate where its
/// probe printed one for the same server time.
fn retail_frames(tag: &str) -> Vec<Frame> {
    let script = fixture(&format!("mp_carentan-dm-bump-{tag}-script.txt"));
    let target: BTreeMap<i32, Seen> = script
        .lines()
        .filter_map(|l| l.strip_prefix("# BUMPT !snap "))
        .map(|rest| {
            let kv = kv(rest);
            (kv["t"].parse().unwrap(), seen(&kv))
        })
        .collect();
    fixture(&format!("mp_carentan-dm-bump-{tag}-walker.txt"))
        .lines()
        .filter_map(|l| l.strip_prefix("!snap "))
        .map(|rest| {
            let kv = kv(rest);
            let t = kv["t"].parse().unwrap();
            Frame {
                t,
                walker: seen(&kv),
                target: target.get(&t).copied(),
                target_solid: kv["target_solid"].parse().unwrap(),
            }
        })
        .collect()
}

/// The server times `probe_bump.gsc` setorigin'd the target onto the walker.
fn overlaps(tag: &str) -> Vec<i32> {
    fixture(&format!("mp_carentan-dm-bump-{tag}-script.txt"))
        .lines()
        .filter_map(|l| l.strip_prefix("PROBE overlap "))
        .map(|rest| rest.split_whitespace().next().unwrap().parse().unwrap())
        .collect()
}

// -------------------------------------------------------------- properties

/// The negated jitter's range per axis: 1.1d's `-1 - rand() / 2^30`, and
/// 1.5b's and UO's `2 rand() / 2^31 - 1` (cod11-player-clip.md 13.4).
const BAND_11: (f32, f32) = (1.0, 3.0);
const BAND_15: (f32, f32) = (-1.0, 1.0);

/// Whether some positive multiple of `dir` less `sep` lands in `band` on
/// both axes: `dir` is then `normalize(sep + j)` for a jitter `j` retail
/// can draw, negated. `tol` absorbs float noise.
fn in_jitter_band(dir: Vec2, sep: Vec2, band: (f32, f32)) -> bool {
    let tol = 0.01;
    let (mut lo, mut hi) = (1e-6f32, f32::MAX);
    for (d, s) in [(dir.x, sep.x), (dir.y, sep.y)] {
        let (a, b) = (s + band.0 - tol, s + band.1 + tol);
        if d.abs() < 1e-6 {
            if !(a..=b).contains(&0.0) {
                return false;
            }
            continue;
        }
        let (t0, t1) = if d > 0.0 {
            (a / d, b / d)
        } else {
            (b / d, a / d)
        };
        lo = lo.max(t0);
        hi = hi.min(t1);
    }
    lo <= hi
}

/// Whether the walker was standing or running when the target landed on it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Case {
    BothStill,
    WalkerWalking,
}

/// The five properties over the frames from an overlap on; returns the number
/// of push frames. `who` names the side in a failure.
fn check_overlap(who: &str, frames: &[Frame], case: Case, band: (f32, f32)) -> usize {
    if std::env::var_os("STUCK_REPORT").is_some() {
        for f in frames.iter().take(12) {
            println!("{who} {f:?}");
        }
    }
    let first = frames[0];
    assert!(
        (251..=300).contains(&first.walker.pm_time)
            && first.walker.pm_flags & PMF_TIME_KNOCKBACK != 0,
        "{who}: the walker's first snapshot after the overlap: {first:?}"
    );
    let t = first.target.expect("the target's own state at the overlap");
    assert!(
        (251..=300).contains(&t.pm_time) && t.pm_flags & PMF_TIME_KNOCKBACK != 0,
        "{who}: the target's first snapshot after the overlap: {t:?}"
    );
    let mut pushes = 0;
    for (i, f) in frames.iter().enumerate() {
        let solid_zero = i > 0 && frames[i - 1].walker.pushed_now();
        assert_eq!(
            f.target_solid == 0,
            solid_zero,
            "{who} t={}: target solid {} after a push frame: {}",
            f.t,
            f.target_solid,
            solid_zero
        );
        if !f.walker.pushed_now() {
            continue;
        }
        pushes += 1;
        let w = f.walker.vel;
        assert!(
            (w.length() - PUSH_SPEED).abs() < 0.01,
            "{who} t={}: walker pushed at {}",
            f.t,
            w.length()
        );
        let Some(t) = f.target else {
            panic!("{who} t={}: no target state beside a push", f.t)
        };
        let sep = (f.walker.origin - t.origin).truncate();
        assert!(
            in_jitter_band(w.normalize(), sep, band),
            "{who} t={}: walker {w:?} off the jitter band from {sep:?}",
            f.t
        );
        match case {
            Case::BothStill => assert!(
                (t.vel + w).length() < 0.01,
                "{who} t={}: target {:?} is not the walker's {w:?} reversed",
                f.t,
                t.vel
            ),
            Case::WalkerWalking => assert_eq!(
                t.vel,
                Vec2::ZERO,
                "{who} t={}: the still target was pushed",
                f.t
            ),
        }
    }
    pushes
}

/// The frames from the overlap at `at` to three seconds after it.
fn window(frames: &[Frame], at: i32) -> Vec<Frame> {
    frames
        .iter()
        .filter(|f| (at..at + 3000).contains(&f.t))
        .copied()
        .collect()
}

/// Retail's push counts per overlap, still then walking.
fn retail_pushes() -> (usize, usize) {
    let frames = retail_frames("overlap");
    let at = overlaps("overlap");
    assert_eq!(at.len(), 2, "two overlaps in the capture");
    (
        check_overlap(
            "retail still",
            &window(&frames, at[0]),
            Case::BothStill,
            BAND_11,
        ),
        check_overlap(
            "retail walking",
            &window(&frames, at[1]),
            Case::WalkerWalking,
            BAND_11,
        ),
    )
}

#[test]
fn retail_pushes_both_players_apart_at_190() {
    let (still, walking) = retail_pushes();
    assert_eq!((still, walking), (2, 4));
}

#[test]
fn retail_does_not_push_beside_a_spectator_in_slot_0() {
    let frames = retail_frames("overlap-spectator");
    assert_eq!(overlaps("overlap-spectator").len(), 2);
    for f in &frames {
        assert!(
            f.walker.pm_time == 0
                && f.walker.pm_flags & PMF_TIME_KNOCKBACK == 0
                && f.target_solid == STAND_SOLID
                && f.target.is_none(),
            "t={}: {f:?}",
            f.t
        );
    }
}

/// 1.5b skips a slot without the own-view flag where 1.1d's scan ends, so
/// a spectator in slot 0 leaves the push on, and its jitter draws from
/// [-1, 1): the first push's direction from a zero separation, (-0.702,
/// 0.712) on the walker, is one 1.1d's cannot draw (cod11-player-clip.md
/// 13.4).
#[test]
fn retail_15_pushes_beside_a_spectator_in_slot_0() {
    let frames = retail_frames("overlap-spectator-15");
    let at = overlaps("overlap-spectator-15");
    assert_eq!(at.len(), 2, "two overlaps in the capture");
    let still = window(&frames, at[0]);
    let walking = window(&frames, at[1]);
    let pushes = (
        check_overlap("retail 1.5 still", &still, Case::BothStill, BAND_15),
        check_overlap("retail 1.5 walking", &walking, Case::WalkerWalking, BAND_15),
    );
    assert!(pushes.0 >= 1 && pushes.1 >= 1, "{pushes:?}");
    let first = still[0];
    let sep = (first.walker.origin - first.target.unwrap().origin).truncate();
    assert!(!in_jitter_band(first.walker.vel.normalize(), sep, BAND_11));
}

/// Our `StuckInClient` at every snapshot of the bump capture, with retail's
/// positions: the target at the header's spot in the stance its `solid`
/// last read, the walker where its playerstate says. It fires where retail
/// pushed and nowhere else, which is the inclusive box test on z and the
/// capsule radius test on the floor plane: a jump that comes down over a
/// crouched or prone head is stuck, one held at the side of a standing
/// player is not.
#[test]
fn our_stuck_test_fires_where_retail_pushed_in_the_bump_capture() {
    let text = fixture("mp_carentan-dm-bump-walker.txt");
    let spot = text
        .lines()
        .find_map(|l| l.strip_prefix("# bump "))
        .map(|h| vec3(kv(h)["spot"]))
        .unwrap();
    let snaps: Vec<_> = text
        .lines()
        .filter_map(|l| l.strip_prefix("!snap "))
        .map(kv)
        .collect();
    let mut solid = STAND_SOLID;
    let mut fired = Vec::new();
    let mut pushed = Vec::new();
    for (i, kv) in snaps.iter().enumerate() {
        let w = seen(kv);
        let ct: i32 = kv["ct"].parse().unwrap();
        let s: i32 = kv["target_solid"].parse().unwrap();
        let solid_next: i32 = snaps
            .get(i + 1)
            .map_or(-1, |n| n["target_solid"].parse().unwrap());
        if s != 0 {
            solid = s;
        }
        let target = Body::from_solid(0, spot, solid, CONTENTS_BODY);
        let view = |origin, maxs: Vec3, vel| StuckView {
            own_view: true,
            playing: true,
            health: 100,
            contents: CONTENTS_BODY,
            origin,
            mins: Vec3::new(-15.0, -15.0, 0.0),
            maxs,
            vel_xy: vel,
            speed: PUSH_SPEED,
            seated: false,
        };
        let views = [
            Some(view(spot, target.maxs, Vec2::ZERO)),
            Some(view(w.origin, Vec3::new(15.0, 15.0, 70.0), w.vel)),
        ];
        if stuck_in_client(0, &views, StuckRules::V1_1, || 1 << 30).is_some() {
            fired.push(ct);
        }
        if w.pushed_now() {
            pushed.push(ct);
            // The target stood still, so the walker alone moves, at 190, and
            // the target reads stuck on the next snapshot.
            let sep = (w.origin - spot).truncate();
            assert!(
                (w.vel.length() - PUSH_SPEED).abs() < 0.01
                    && in_jitter_band(w.vel.normalize(), sep, BAND_11)
                    && solid_next == 0,
                "ct={ct}: {w:?} from {sep:?}, next solid {solid_next}"
            );
        }
    }
    assert_eq!(pushed, [70633, 97483]);
    assert_eq!(fired, pushed);
}

// --------------------------------------------------------------------- ours

type Client = NetClient<ClientEnd>;

const MAP: &str = "mp_carentan";

/// The 1.5b install's paks, `None` elsewhere (a UO install's `main/` is
/// 1.5's, so this runs there too).
fn fs_15() -> Option<Pk3Fs> {
    let fs = vcod_common::testing::game_fs()?;
    (vcod_server::server::install_protocol(&fs).transport.version == PROTOCOL_V6.transport.version)
        .then_some(fs)
}

/// A dm server on `fs`'s protocol, its pushes drawn from glibc's `rand()`
/// under `seed` when given (`Server::pin_stuck_rand`).
fn server(fs: Pk3Fs, seed: Option<(u32, usize)>) -> (Server, Instant) {
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let now = Instant::now();
    let mut sv = Server::new(common::cfg(MAP, "dm"), now);
    sv.set_protocol(vcod_server::server::install_protocol(&fs));
    if let Some((seed, skip)) = seed {
        sv.pin_stuck_rand(seed, skip);
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    (sv, now)
}

/// A client on the rig, by its address.
struct Peer {
    addr: std::net::SocketAddr,
    q: Rc<RefCell<Queues>>,
    cl: Client,
}

impl Peer {
    fn new(addr: std::net::SocketAddr, qport: u16, now: Instant) -> Peer {
        let q = Rc::new(RefCell::new(Queues::default()));
        let cl = NetClient::start_with_qport(ClientEnd(q.clone()), now, qport);
        Peer { addr, q, cl }
    }

    fn ps(&self) -> &vcod_common::net::msg::PlayerState {
        &self.cl.snapshots().newest().unwrap().ps
    }

    fn num(&self) -> usize {
        self.ps().field_i32(self.cl.protocol(), "clientNum") as usize
    }

    fn seen(&self) -> Seen {
        let ps = self.ps();
        Seen {
            origin: ps.origin(self.cl.protocol()).into(),
            vel: Vec2::new(
                ps.field_f32(self.cl.protocol(), "velocity[0]"),
                ps.field_f32(self.cl.protocol(), "velocity[1]"),
            ),
            pm_flags: ps.field_i32(self.cl.protocol(), "pm_flags"),
            pm_time: ps.field_i32(self.cl.protocol(), "pm_time"),
        }
    }

    /// `other`'s entity `solid` in this client's newest snapshot.
    fn solid_of(&self, other: usize) -> i32 {
        let s = self.cl.snapshots().newest().unwrap();
        s.entities
            .get(&(other as u32))
            .expect("the pair is sent each other")
            .field_i32(self.cl.protocol(), "solid")
    }
}

/// The server and every peer, stepped together, each peer sending the cmd
/// the caller hands it (`None` sends nothing that frame).
struct Rig {
    sv: Server,
    now: Instant,
    peers: Vec<Peer>,
}

impl Rig {
    fn step(&mut self, cmds: &[Option<UserCmd>]) -> Vec<Vec<NetEvent>> {
        self.step_ordered(cmds, false)
    }

    /// [`Rig::step`] handing the packets to the server last peer first when
    /// `reversed`.
    fn step_ordered(&mut self, cmds: &[Option<UserCmd>], reversed: bool) -> Vec<Vec<NetEvent>> {
        self.now += Duration::from_millis(50);
        for (p, c) in self.peers.iter_mut().zip(cmds) {
            if let Some(c) = c {
                p.cl.send_frame(c);
            }
        }
        let mut order: Vec<&Peer> = self.peers.iter().collect();
        if reversed {
            order.reverse();
        }
        for p in order {
            let pending: Vec<Vec<u8>> = p.q.borrow_mut().to_server.drain(..).collect();
            for pkt in pending {
                self.sv.handle_packet(p.addr, &pkt, self.now);
            }
        }
        self.sv.tick(self.now);
        for (to, pkt) in self.sv.take_outgoing() {
            let p = self.peers.iter().find(|p| p.addr == to).unwrap();
            p.q.borrow_mut().to_client.push_back(pkt);
        }
        let now = self.now;
        self.peers.iter_mut().map(|p| p.cl.pump_at(now)).collect()
    }

    fn holding(&self, i: usize) -> UserCmd {
        common::holding(&self.peers[i].cl)
    }
}

/// `spectator` first when asked, so it holds slot 0, then the target and the
/// walker joined as allied carbines in that order.
fn joined(fs: Pk3Fs, seed: Option<(u32, usize)>, spectator: bool) -> Rig {
    let (sv, now) = server(fs, seed);
    let mut rig = Rig {
        sv,
        now,
        peers: Vec::new(),
    };
    if spectator {
        let q = Rc::new(RefCell::new(Queues::default()));
        let cl = common::connect_at(&mut rig.sv, common::ADDR_C, &q, &mut rig.now, 0x2000);
        rig.peers.push(Peer {
            addr: common::ADDR_C,
            q,
            cl,
        });
    }
    rig.peers.push(Peer::new(common::ADDR, 0x2001, rig.now));
    rig.peers.push(Peer::new(common::ADDR_B, 0x2002, rig.now));
    let first = rig.peers.len() - 2;
    let mut joins = [
        Join::new("allies", "m1carbine_mp"),
        Join::new("allies", "m1carbine_mp"),
    ];
    for _ in 0..600 {
        let cmds = vec![Some(NULL_USERCMD); rig.peers.len()];
        // Once the pair is in, the walker's packets go ahead of the
        // target's, so the target answers and spawns first
        // (`common::join_pair_stamped`). The spectator sends no answer.
        let entered = rig.peers[first..]
            .iter()
            .all(|p| p.cl.state() == vcod_common::net::NetState::Active);
        let events = rig.step_ordered(&cmds, entered);
        let now = rig.now;
        for (k, join) in joins.iter_mut().enumerate() {
            for e in &events[first + k] {
                if let NetEvent::ServerCommand(tokens) = e {
                    join.on_server_command(tokens, &mut rig.peers[first + k].cl, now);
                }
            }
        }
        if joins.iter().all(|j| j.settled(rig.now)) {
            break;
        }
    }
    assert!(
        joins.iter().all(|j| j.settled(rig.now)),
        "the pair never joined"
    );
    if spectator {
        assert_eq!(rig.peers[0].num(), 0, "the spectator holds slot 0");
    }
    let (t, w) = (rig.peers[first].num(), rig.peers[first + 1].num());
    assert!(t < w, "the target {t} is the lower slot, the walker {w}");
    rig
}

/// Where retail's walker was placed, on the flat brush floor the capture
/// used, facing +x down 560 units of it; the target waits 70 units off that
/// line (`probe_bump.gsc`, and the fixture header's `spot`).
const WALKER_SPOT: [f32; 3] = [932.0, -376.0, -151.875];
const TARGET_SPOT: [f32; 3] = [1132.0, -306.0, -151.875];

fn facing(rig: &Rig, i: usize, yaw: f32) -> UserCmd {
    UserCmd {
        angles: [0, (yaw * 65536.0 / 360.0) as i32 & 0xffff, 0],
        ..rig.holding(i)
    }
}

/// Puts the pair on retail's spots and lets both settle, runs the walker
/// forward for `run` frames when asked, then `setorigin`s the target onto
/// the walker a unit up, the way `probe_bump.gsc` does: queued for the next
/// frame's script pass, after that frame's moves and before its end frame.
/// Records the 40 frames from that one on.
fn overlap_on_ours(
    fs: Pk3Fs,
    seed: Option<(u32, usize)>,
    spectator: bool,
    run: usize,
) -> Vec<Frame> {
    let mut rig = joined(fs, seed, spectator);
    let (ti, wi) = (rig.peers.len() - 2, rig.peers.len() - 1);
    let (tn, wn) = (rig.peers[ti].num(), rig.peers[wi].num());
    let yaw = 0.0;
    rig.sv.place_client(wn, WALKER_SPOT, yaw);
    rig.sv.place_client(tn, TARGET_SPOT, yaw);
    let n = rig.peers.len();
    let hold = |rig: &Rig| -> Vec<Option<UserCmd>> {
        (0..n)
            .map(|i| {
                Some(if i < ti {
                    NULL_USERCMD
                } else {
                    facing(rig, i, yaw)
                })
            })
            .collect()
    };
    for _ in 0..20 {
        let cmds = hold(&rig);
        rig.step(&cmds);
    }
    for _ in 0..run {
        let mut cmds = hold(&rig);
        cmds[wi] = Some(UserCmd {
            forward: 127,
            ..facing(&rig, wi, yaw)
        });
        rig.step(&cmds);
    }
    let mut frames = Vec::new();
    for k in 0..40 {
        if k == 0 {
            // Where the walker stands once this frame's cmd has run: a run
            // at full speed on flat ground covers its velocity times the
            // frame.
            let w = rig.peers[wi].seen();
            let at = w.origin + (w.vel * 0.05).extend(1.0);
            rig.sv.test_script_set_origin(tn, at.into());
        }
        let mut cmds = hold(&rig);
        if run > 0 && k < 30 {
            cmds[wi] = Some(UserCmd {
                forward: 127,
                ..facing(&rig, wi, yaw)
            });
        }
        rig.step(&cmds);
        let (tp, wp) = (&rig.peers[ti], &rig.peers[wi]);
        frames.push(Frame {
            t: k * 50,
            walker: wp.seen(),
            target: Some(tp.seen()),
            target_solid: wp.solid_of(tn),
        });
    }
    frames
}

#[test]
fn ours_pushes_the_still_pair_the_way_retail_does() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or not 1.1's: skipping");
        return;
    };
    let frames = overlap_on_ours(fs, None, false, 0);
    let ours = check_overlap("ours still", &frames, Case::BothStill, BAND_11);
    let (retail, _) = retail_pushes();
    assert_eq!(ours, retail, "push frames, ours against retail");
}

#[test]
fn ours_pushes_only_the_walker_when_it_walks_in() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or not 1.1's: skipping");
        return;
    };
    let frames = overlap_on_ours(fs, None, false, 15);
    let ours = check_overlap("ours walking", &frames, Case::WalkerWalking, BAND_11);
    let (_, retail) = retail_pushes();
    println!("walking overlap: ours {ours} push frames, retail {retail}");
    assert_eq!(ours, retail, "push frames, ours against retail");
}

#[test]
fn ours_does_not_push_beside_a_spectator_in_slot_0() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or not 1.1's: skipping");
        return;
    };
    let frames = overlap_on_ours(fs, None, true, 0);
    let first = frames[0];
    let overlap = (first.walker.origin - first.target.unwrap().origin).truncate();
    assert!(overlap.length() < 1.0, "the setorigin missed: {first:?}");
    for f in &frames {
        let t = f.target.unwrap();
        assert!(
            f.walker.pm_time == 0
                && t.pm_time == 0
                && (f.walker.pm_flags | t.pm_flags) & PMF_TIME_KNOCKBACK == 0
                && f.target_solid == STAND_SOLID,
            "t={}: {f:?}",
            f.t
        );
    }
}

/// Retail 1.5b's `rand()` seed under `-overlap-spectator-15` and the draws
/// before its first push, the joins' (player-clip doc 13.4).
const SEED_15: (u32, usize) = (1252, 8);

/// Each push frame's walker velocity.
fn push_vels(frames: &[Frame]) -> Vec<Vec2> {
    frames
        .iter()
        .filter(|f| f.walker.pushed_now())
        .map(|f| f.walker.vel)
        .collect()
}

/// 1.5b's scan passes over the spectator in slot 0, so the still pair is
/// pushed apart beside him, on retail's draws: the first push, from the
/// script's placement, to the bit, and as many push frames as retail's.
/// The second push's positions ride each client's cmd clock (retail's
/// walker ran 60 Hz cmds, its target's are not logged), so its direction
/// is held to the band only.
#[test]
fn ours_15_pushes_the_still_pair_beside_a_spectator_in_slot_0() {
    let Some(fs) = fs_15() else {
        eprintln!("COD_DIR unset or not 1.5's: skipping");
        return;
    };
    let frames = overlap_on_ours(fs, Some(SEED_15), true, 0);
    check_overlap("ours 1.5 still", &frames, Case::BothStill, BAND_15);
    let retail = retail_frames("overlap-spectator-15");
    let at = overlaps("overlap-spectator-15");
    let (ours, retail) = (push_vels(&frames), push_vels(&window(&retail, at[0])));
    assert_eq!(ours.len(), retail.len(), "push frames: {ours:?} {retail:?}");
    assert_eq!(ours[0], retail[0], "the first push's velocity");
}

/// The walking overlap beside the spectator on 1.5b: only the walker is
/// pushed, as many frames as retail's. The walk starts on our own spot, not
/// where retail's first push left the pair, so the draws past the still
/// overlap's eight give other directions.
#[test]
fn ours_15_pushes_only_the_walker_beside_a_spectator_in_slot_0() {
    let Some(fs) = fs_15() else {
        eprintln!("COD_DIR unset or not 1.5's: skipping");
        return;
    };
    let frames = overlap_on_ours(fs, Some((SEED_15.0, SEED_15.1 + 8)), true, 15);
    let ours = check_overlap("ours 1.5 walking", &frames, Case::WalkerWalking, BAND_15);
    let retail = retail_frames("overlap-spectator-15");
    let at = overlaps("overlap-spectator-15");
    let walking = window(&retail, at[1]);
    assert_eq!(
        ours,
        push_vels(&walking).len(),
        "push frames, ours against retail"
    );
}

// ---------------------------------------------------------------- the wall

const WALL: &str = "mp_carentan-dm-bump-overlap-wall-walker.txt";

/// One walker snapshot of the wall capture: what the rebase starts from.
struct WallSnap {
    ct: i32,
    origin: Vec3,
    vel: Vec3,
    on_ground: bool,
    pm_flags: i32,
    pm_time: i32,
}

fn wall_capture() -> (Vec<UserCmd>, Vec<WallSnap>) {
    let text = fixture(WALL);
    let mut cmds = Vec::new();
    let mut snaps = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("!cmd ") {
            let kv = kv(rest);
            let n = |k: &str| kv[k].parse::<i32>().unwrap();
            let a: Vec<i32> = kv["angles"]
                .split(',')
                .map(|x| x.parse().unwrap())
                .collect();
            cmds.push(UserCmd {
                server_time: n("st"),
                buttons: n("buttons") as u8,
                wbuttons: n("wbuttons") as u8,
                weapon: n("weapon") as u8,
                angles: [a[0], a[1], a[2]],
                forward: n("forward") as i8,
                right: n("right") as i8,
                up: n("up") as i8,
                ..NULL_USERCMD
            });
        } else if let Some(rest) = line.strip_prefix("!snap ") {
            let kv = kv(rest);
            snaps.push(WallSnap {
                ct: kv["ct"].parse().unwrap(),
                origin: vec3(kv["origin"]),
                vel: vec3(kv["vel"]),
                on_ground: kv["ground"] != "1023",
                pm_flags: kv["pm_flags"].parse().unwrap(),
                pm_time: kv["pm_time"].parse().unwrap(),
            });
        }
    }
    (cmds, snaps)
}

/// `StuckInClient`'s push into a wall (cod11-player-clip.md 10.4): the
/// walker, pushed at 190 into the +y wall beside it, slides along it under
/// 0x100, and pushed again into the corner it walked into, stops dead.
/// Each snapshot pair the push did not write between is replayed through
/// our pmove from retail's state, cmd for cmd, and the origin, velocity and
/// timer compared with the next snapshot. The push direction is
/// `rand()`'s, so this half replays pmove only; the push itself is the
/// gates above. Needs `COD_DIR` at a 1.1 install.
#[test]
fn a_pushed_player_slides_along_a_wall_as_retail_does() {
    use vcod_common::collision::CollisionWorld;
    use vcod_common::movetrace::MoveWorld;
    use vcod_common::pmove::cmd::{self, EventRing};
    use vcod_common::pmove::{PMF_TIME_KNOCKBACK, PlayerState};
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let cs7 = include_str!("fixtures/configstrings/mp_carentan-dm.txt")
        .lines()
        .find_map(|l| l.strip_prefix("7 "))
        .expect("configstring 7");
    let weapons = vcod_common::weapon_table::from_configstring(&fs, cs7);
    let bsp_path = fs.resolve_map("mp_carentan").unwrap();
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let world = CollisionWorld::build(
        &bsp,
        &vcod_common::props::collision_tris(&fs, &bsp.entities),
    );
    world.unlink_script_brushes(&bsp.entities, "dm");
    let mw = MoveWorld::bare(&world);
    let (cmds, snaps) = wall_capture();
    let (mut compared, mut pushed, mut held) = (0, 0, 0);
    let mut bad = Vec::new();
    for pair in snaps.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let elapsed = b.ct - a.ct;
        // A push between the two writes `pm_time` 300 over pmove's count.
        let untouched = elapsed > 0
            && a.pm_time >= b.pm_time
            && (b.pm_time == 0 || a.pm_time - b.pm_time == elapsed);
        if !untouched {
            continue;
        }
        let mut ps = PlayerState::spawn(a.origin, 0.0);
        ps.velocity = a.vel;
        ps.on_ground = a.on_ground;
        ps.knockback_ms = a.pm_time as f32;
        ps.knockback_flags = a.pm_flags & PMF_TIME_KNOCKBACK;
        ps.weapon = 12;
        ps.weapons_held = 1 << 12;
        let mut last = a.ct;
        for c in cmds
            .iter()
            .filter(|c| c.server_time > a.ct && c.server_time <= b.ct)
        {
            // The walker faces +x: its delta_angles undo the cmd's yaw.
            let mut da = [0, (-c.angles[1]).rem_euclid(65536), 0];
            let mut ring = EventRing::default();
            cmd::chop(last, c, |step, dt| {
                cmd::player_step(&mut ps, &mut da, &mut ring, step, dt, &mw, &weapons);
                ps.jump_latched
            });
            last = c.server_time;
        }
        if last != b.ct {
            continue;
        }
        compared += 1;
        if a.pm_flags & PMF_TIME_KNOCKBACK != 0 {
            pushed += 1;
            if a.vel.y > 0.0 && b.origin.y > 456.8 {
                held += 1;
            }
        }
        let off = (ps.origin - b.origin).abs().max_element();
        let voff = (ps.velocity - b.vel).abs().max_element();
        if off > 0.01 || voff > 1.0 || ps.knockback_ms as i32 != b.pm_time {
            bad.push(format!(
                "ct {}: ours {} {} {} retail {} {} {}",
                b.ct, ps.origin, ps.velocity, ps.knockback_ms, b.origin, b.vel, b.pm_time
            ));
        }
    }
    println!("{compared} snapshots compared, {pushed} under the push, {held} along the wall");
    assert!(pushed >= 12, "only {pushed} pushed snapshots");
    assert!(held >= 4, "only {held} along the wall");
    assert!(bad.is_empty(), "{bad:#?}");
}
