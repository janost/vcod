//! Two clients on one server, one walking into the other: live players block
//! each other, dead and spectating ones do not, and the entity `solid` each
//! is sent follows its stance (docs/research/cod11-player-clip.md).
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::{ClientEnd, Join, Queues};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::movetrace::Body;
use vcod_common::net::msg::{UserCmd, WBUTTON_CROUCH, WBUTTON_PRONE};
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_common::net::{NetClient, NetEvent};
use vcod_server::Server;

type Client = NetClient<ClientEnd>;

const MAP: &str = "mp_carentan";

struct Rig {
    sv: Server,
    ca: Client,
    cb: Client,
    qa: Rc<RefCell<Queues>>,
    qb: Rc<RefCell<Queues>>,
    now: Instant,
}

impl Rig {
    fn step(&mut self, a: &UserCmd, b: &UserCmd) {
        self.now += Duration::from_millis(50);
        self.ca.send_frame(a);
        self.cb.send_frame(b);
        common::step_pair(
            &mut self.sv,
            (&self.qa, &mut self.ca),
            (&self.qb, &mut self.cb),
            self.now,
        );
    }

    fn num(cl: &Client) -> usize {
        cl.snapshots()
            .newest()
            .unwrap()
            .ps
            .field_i32(&PROTOCOL_V1, "clientNum") as usize
    }

    fn origin(cl: &Client) -> [f32; 3] {
        cl.snapshots().newest().unwrap().ps.origin(&PROTOCOL_V1)
    }

    /// A's entity `solid` in B's newest snapshot, `None` when B is not sent A.
    fn solid_of_a_seen_by_b(&self) -> Option<i32> {
        let na = Self::num(&self.ca) as u32;
        let s = self.cb.snapshots().newest()?;
        Some(s.entities.get(&na)?.field_i32(&PROTOCOL_V1, "solid"))
    }

    /// A's `pos.trDelta` (its wire velocity) in B's newest snapshot, `None`
    /// when B is not sent A.
    fn vel_of_a_seen_by_b(&self) -> Option<[f32; 2]> {
        let na = Self::num(&self.ca) as u32;
        let s = self.cb.snapshots().newest()?;
        let e = s.entities.get(&na)?;
        Some([
            e.field_f32(&PROTOCOL_V1, "pos.trDelta[0]"),
            e.field_f32(&PROTOCOL_V1, "pos.trDelta[1]"),
        ])
    }
}

fn dist_xy(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// `holding` looking along `yaw`. The client sends absolute view angles and
/// subtracts `delta_angles` itself.
fn facing(cl: &Client, yaw: f32) -> UserCmd {
    UserCmd {
        angles: [0, (yaw * 65536.0 / 360.0) as i32 & 0xffff, 0],
        ..common::holding(cl)
    }
}

fn server() -> Option<(Server, Instant)> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let now = Instant::now();
    let mut sv = Server::new(common::cfg(MAP, "dm"), now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    Some((sv, now))
}

/// Both clients joined as allied carbines.
fn joined() -> Option<Rig> {
    let (mut sv, mut now) = server()?;
    let qa = Rc::new(RefCell::new(Queues::default()));
    let qb = Rc::new(RefCell::new(Queues::default()));
    let (ca, cb) = common::join_pair(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("allies", "m1carbine_mp"),
        ("allies", "m1carbine_mp"),
    );
    Some(Rig {
        sv,
        ca,
        cb,
        qa,
        qb,
        now,
    })
}

/// B joined, A connected and never answering the team menu, so it stays the
/// spectator the connect made it.
fn joined_beside_a_spectator() -> Option<Rig> {
    let (mut sv, mut now) = server()?;
    let qa = Rc::new(RefCell::new(Queues::default()));
    let qb = Rc::new(RefCell::new(Queues::default()));
    let mut ca = NetClient::start_with_qport(ClientEnd(qa.clone()), now, 0x2001);
    let mut cb = NetClient::start_with_qport(ClientEnd(qb.clone()), now, 0x2002);
    let mut jb = Join::new("allies", "m1carbine_mp");
    for _ in 0..600 {
        now += Duration::from_millis(50);
        ca.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        cb.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        let (_, eb) = common::step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
        for e in eb {
            if let NetEvent::ServerCommand(tokens) = e {
                jb.on_server_command(&tokens, &mut cb, now);
            }
        }
        if jb.settled(now) {
            break;
        }
    }
    assert!(jb.settled(now), "{}", jb.summary());
    Some(Rig {
        sv,
        ca,
        cb,
        qa,
        qb,
        now,
    })
}

/// A yaw from `spot` along which the map is open and flat for 200 units (60
/// behind), and the ground 150 units out. Spawns are random, so the
/// direction is searched.
fn open_line(sv: &Server, spot: [f32; 3]) -> (f32, [f32; 3]) {
    for i in 0..16 {
        let yaw = i as f32 * 22.5;
        // Behind A too, so a walk that passes through has room to show it.
        if !sv.test_clear_line(spot, yaw, 200.0) || !sv.test_clear_line(spot, yaw + 180.0, 60.0) {
            continue;
        }
        let (s, c) = yaw.to_radians().sin_cos();
        let at = |d: f32| [spot[0] + c * d, spot[1] + s * d, spot[2] + 16.0];
        let flat = (1..=8).all(|k| {
            sv.test_ground_under(at(k as f32 * 25.0))
                .is_some_and(|g| (g[2] - spot[2]).abs() < 0.5)
        });
        if let (true, Some(far)) = (flat, sv.test_ground_under(at(150.0))) {
            return (yaw, far);
        }
    }
    panic!("no open flat line from {spot:?}");
}

/// Where B stands along the line from A's spot, in units from A's feet.
fn along(spot: [f32; 3], yaw: f32, p: [f32; 3]) -> f32 {
    let (s, c) = yaw.to_radians().sin_cos();
    (p[0] - spot[0]) * c + (p[1] - spot[1]) * s
}

/// B is put at `far` facing A's spot and runs at it; A holds still. Returns
/// where B ended along the line, in units from A's feet.
fn run_b_at_a(r: &mut Rig, a_spot: [f32; 3], yaw: f32, far: [f32; 3], frames: usize) -> f32 {
    let nb = Rig::num(&r.cb);
    r.sv.place_client(nb, far, yaw + 180.0);
    for _ in 0..frames {
        let hold = facing(&r.ca, yaw);
        let run = UserCmd {
            forward: 127,
            ..facing(&r.cb, yaw + 180.0)
        };
        r.step(&hold, &run);
    }
    along(a_spot, yaw, Rig::origin(&r.cb))
}

#[test]
fn a_walk_into_a_standing_player_stops_at_thirty_units() {
    let Some(mut r) = joined() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let spot = Rig::origin(&r.ca);
    let (yaw, far) = open_line(&r.sv, spot);
    r.sv.place_client(Rig::num(&r.ca), spot, yaw);
    let d = run_b_at_a(&mut r, spot, yaw, far, 30);
    assert!(
        (30.0..=31.0).contains(&d),
        "B stopped {d:.2} units from A's feet"
    );
}

#[test]
fn dead_and_spectator_do_not_block() {
    let Some(mut r) = joined() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let spot = Rig::origin(&r.ca);
    let (yaw, far) = open_line(&r.sv, spot);
    r.sv.place_client(Rig::num(&r.ca), spot, yaw);
    r.ca.send_reliable("kill");
    for _ in 0..10 {
        let (a, b) = (common::holding(&r.ca), common::holding(&r.cb));
        r.step(&a, &b);
    }
    let pm_type = |cl: &Client| {
        cl.snapshots()
            .newest()
            .unwrap()
            .ps
            .field_i32(&PROTOCOL_V1, "pm_type")
    };
    assert_eq!(pm_type(&r.ca), 6, "A is dead");
    let d = run_b_at_a(&mut r, spot, yaw, far, 30);
    assert!(d < -20.0, "B stopped {d:.2} units from dead A's feet");

    let Some(mut r) = joined_beside_a_spectator() else {
        return;
    };
    // A spectator is wherever the connect parked it; the line runs from B's
    // spawn, and A is moved onto it without leaving spectator mode.
    let spot = Rig::origin(&r.cb);
    let (yaw, far) = open_line(&r.sv, spot);
    let na = Rig::num(&r.ca);
    r.sv.test_set_client_origin(na, spot);
    let d = run_b_at_a(&mut r, spot, yaw, far, 30);
    assert_eq!(pm_type(&r.ca), 4, "A is a spectator");
    assert!(d < -20.0, "B stopped {d:.2} units from spectator A");
}

#[test]
fn crouched_and_prone_solid_on_the_wire() {
    let Some(mut r) = joined() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let spot = Rig::origin(&r.ca);
    let (yaw, far) = open_line(&r.sv, spot);
    // A faces away from B: a prone body lies behind the view, and the line
    // towards B is the ground known to be open.
    r.sv.place_client(Rig::num(&r.ca), spot, yaw + 180.0);
    r.sv.place_client(Rig::num(&r.cb), far, yaw + 180.0);
    let settle = |r: &mut Rig, wbuttons: u8| {
        for _ in 0..40 {
            let a = UserCmd {
                wbuttons,
                ..facing(&r.ca, yaw + 180.0)
            };
            let b = facing(&r.cb, yaw + 180.0);
            r.step(&a, &b);
        }
        r.solid_of_a_seen_by_b().expect("B is sent A")
    };
    let mins = glam::Vec3::new(-15.0, -15.0, 0.0);
    let box_to = |z: f32| Body::pack_solid(mins, glam::Vec3::new(15.0, 15.0, z));
    assert_eq!(settle(&mut r, 0), 6684943, "standing");
    assert_eq!(box_to(50.0), (82 << 16) | (1 << 8) | 15);
    assert_eq!(settle(&mut r, WBUTTON_CROUCH), box_to(50.0), "crouched");
    assert_eq!(box_to(30.0), (62 << 16) | (1 << 8) | 15);
    assert_eq!(settle(&mut r, WBUTTON_PRONE), box_to(30.0), "prone");
}

#[test]
fn stuck_player_solid_reads_zero_after_its_next_cmd() {
    let Some(mut r) = joined() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let spot = Rig::origin(&r.ca);
    r.sv.place_client(Rig::num(&r.ca), spot, 0.0);
    r.sv.place_client(Rig::num(&r.cb), spot, 180.0);
    // Tick 1's end frame marks both corpses (they start fully overlapped),
    // but nothing relinks until each plays its next cmd, so tick 1's
    // snapshot still carries the standing box (6684943, the same pack as
    // line 283's `settle`) and tick 2's, relinked from the corpse mark,
    // reads 0.
    let mut seen = Vec::new();
    for _ in 0..2 {
        let (a, b) = (common::holding(&r.ca), common::holding(&r.cb));
        r.step(&a, &b);
        seen.push(r.solid_of_a_seen_by_b());
    }
    assert_eq!(
        seen[0],
        Some(6684943),
        "A's solid should still be packed on the snapshot straight after the stuck frame: {seen:?}"
    );
    assert_eq!(
        seen[1],
        Some(0),
        "A's solid should read 0 once A's next cmd has relinked the corpse mark: {seen:?}"
    );
}

#[test]
fn overlapping_pair_pushes_apart() {
    let Some(mut r) = joined() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let spot = Rig::origin(&r.ca);
    r.sv.place_client(Rig::num(&r.ca), spot, 0.0);
    r.sv.place_client(Rig::num(&r.cb), spot, 180.0);
    let mut push_speed = None;
    for _ in 0..20 {
        let (a, b) = (common::holding(&r.ca), common::holding(&r.cb));
        r.step(&a, &b);
        if push_speed.is_none() {
            if let Some([vx, vy]) = r.vel_of_a_seen_by_b() {
                let speed = (vx * vx + vy * vy).sqrt();
                if speed > 100.0 {
                    push_speed = Some(speed);
                }
            }
        }
    }
    let speed = push_speed.expect("B never saw A's push velocity on the wire");
    assert!((speed - 190.0).abs() < 2.0, "push speed {speed}");
    let d = dist_xy(Rig::origin(&r.ca), Rig::origin(&r.cb));
    assert!(d >= 30.0, "the pair is still {d:.2} apart after 1 s");
}

#[test]
fn a_spectator_anywhere_disables_the_push() {
    let Some((mut sv, mut now)) = server() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let qs = Rc::new(RefCell::new(Queues::default()));
    // Connects alone first, so it is the only client the slot table has ever
    // seen and holds slot 0 once the pair joins beside it.
    let mut cs = common::connect_at(&mut sv, common::ADDR_C, &qs, &mut now, 0x2000);

    let qa = Rc::new(RefCell::new(Queues::default()));
    let qb = Rc::new(RefCell::new(Queues::default()));
    let mut ca = NetClient::start_with_qport(ClientEnd(qa.clone()), now, 0x2001);
    let mut cb = NetClient::start_with_qport(ClientEnd(qb.clone()), now, 0x2002);
    let (mut ja, mut jb) = (
        Join::new("allies", "m1carbine_mp"),
        Join::new("allies", "m1carbine_mp"),
    );
    for _ in 0..600 {
        now += Duration::from_millis(50);
        cs.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        ca.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        cb.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        let (_, ea, eb) = common::step_trio(
            &mut sv,
            (common::ADDR_C, &qs, &mut cs),
            (common::ADDR, &qa, &mut ca),
            (common::ADDR_B, &qb, &mut cb),
            now,
        );
        for e in ea {
            if let NetEvent::ServerCommand(tokens) = e {
                ja.on_server_command(&tokens, &mut ca, now);
            }
        }
        for e in eb {
            if let NetEvent::ServerCommand(tokens) = e {
                jb.on_server_command(&tokens, &mut cb, now);
            }
        }
        if ja.settled(now) && jb.settled(now) {
            break;
        }
    }
    assert!(ja.settled(now) && jb.settled(now), "the pair never joined");

    let num = |cl: &Client| {
        cl.snapshots()
            .newest()
            .unwrap()
            .ps
            .field_i32(&PROTOCOL_V1, "clientNum") as usize
    };
    assert_eq!(
        num(&cs),
        0,
        "the spectator connected first and must hold slot 0"
    );

    let (na, nb) = (num(&ca), num(&cb));
    let a_spot = cl_origin(&ca);
    sv.place_client(na, a_spot, 0.0);
    sv.place_client(nb, a_spot, 180.0);
    for _ in 0..20 {
        now += Duration::from_millis(50);
        cs.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        let a = common::holding(&ca);
        let b = common::holding(&cb);
        ca.send_frame(&a);
        cb.send_frame(&b);
        common::step_trio(
            &mut sv,
            (common::ADDR_C, &qs, &mut cs),
            (common::ADDR, &qa, &mut ca),
            (common::ADDR_B, &qb, &mut cb),
            now,
        );
    }
    let d = dist_xy(cl_origin(&ca), cl_origin(&cb));
    assert!(
        d < 5.0,
        "the pair separated by {d:.2} despite the spectator's slot-0 veto"
    );
}

fn cl_origin(cl: &Client) -> [f32; 3] {
    cl.snapshots().newest().unwrap().ps.origin(&PROTOCOL_V1)
}
