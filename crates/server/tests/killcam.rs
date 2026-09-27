//! The killcam end to end on the stock `dm.gsc`: A kills B, and two seconds
//! later B's snapshots are A as the archive had it nine seconds before, until
//! the replay runs out and B is a dead client again where the replay left it,
//! or until B's use press skips it straight to the respawn
//! (docs/research/cod11-spectator-follow.md, section 12).
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::{ClientEnd, Queues};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{UserCmd, BUTTON_ADS, BUTTON_ATTACK, BUTTON_USE, NULL_USERCMD};
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_common::net::snapshot::Snapshot;
use vcod_common::net::NetClient;

const MAP: &str = "mp_carentan";
const PMF_FOLLOW: i32 = 0x10000;
const PM_DEAD: i32 = 6;

/// A and B on stock dm, B 40 units down A's +x facing back, and every
/// server time's A origin as A was sent it.
struct Rig {
    sv: vcod_server::Server,
    qa: Rc<RefCell<Queues>>,
    qb: Rc<RefCell<Queues>>,
    ca: NetClient<ClientEnd>,
    cb: NetClient<ClientEnd>,
    now: Instant,
    na: usize,
    nb: usize,
    spot: [f32; 3],
    trail: BTreeMap<i32, [f32; 3]>,
}

const FACING_A: UserCmd = UserCmd {
    angles: [0, 32768, 0],
    ..NULL_USERCMD
};

impl Rig {
    fn new() -> Option<Self> {
        let fs = vcod_common::testing::game_fs()?;
        let p = &PROTOCOL_V1;
        let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
        let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
        let mut now = Instant::now();
        let mut sv = vcod_server::Server::new(common::cfg(MAP, "dm"), now);
        sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
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
        let num = |c: &NetClient<ClientEnd>| {
            c.snapshots().newest().unwrap().ps.field_i32(p, "clientNum") as usize
        };
        let (na, nb) = (num(&ca), num(&cb));
        let spot = ca.snapshots().newest().unwrap().ps.origin(p);
        let mut rig = Rig {
            sv,
            qa,
            qb,
            ca,
            cb,
            now,
            na,
            nb,
            spot,
            trail: BTreeMap::new(),
        };
        rig.place();
        Some(rig)
    }

    fn place(&mut self) {
        let spot = self.spot;
        assert!(
            self.sv.test_clear_line(spot, 0.0, 40.0),
            "no clear 40 units along +x from the spawn"
        );
        self.sv.place_client(self.na, spot, 0.0);
        self.sv
            .place_client(self.nb, [spot[0] + 40.0, spot[1], spot[2]], 180.0);
    }

    /// One frame; B's snapshot.
    fn step(&mut self, a: &UserCmd, b: &UserCmd) -> Snapshot {
        self.now += Duration::from_millis(50);
        self.ca.send_frame(a);
        self.cb.send_frame(b);
        common::step_pair(
            &mut self.sv,
            (&self.qa, &mut self.ca),
            (&self.qb, &mut self.cb),
            self.now,
        );
        let s = self.ca.snapshots().newest().unwrap();
        self.trail.insert(s.server_time, s.ps.origin(&PROTOCOL_V1));
        self.cb.snapshots().newest().unwrap().clone()
    }

    /// Ten seconds of A walking about, so the archive has a trail and a
    /// replay of A cannot be mistaken for the live A, then two taps down the
    /// sight that kill B (the combat test's numbers). The kill's server time.
    fn walk_then_kill(&mut self) -> i32 {
        for i in 0..200 {
            let right = if (i / 50) % 2 == 0 { 127 } else { -127 };
            self.step(
                &UserCmd {
                    right,
                    ..NULL_USERCMD
                },
                &FACING_A,
            );
        }
        self.place();
        for _ in 0..20 {
            self.step(&NULL_USERCMD, &FACING_A);
        }
        let fire = UserCmd {
            buttons: BUTTON_ADS | BUTTON_ATTACK,
            ..NULL_USERCMD
        };
        let ads = UserCmd {
            buttons: BUTTON_ADS,
            ..NULL_USERCMD
        };
        for i in 0..80 {
            let a = if i % 40 == 0 { &fire } else { &ads };
            let sb = self.step(a, &FACING_A);
            if sb.ps.field_i32(&PROTOCOL_V1, "pm_type") == PM_DEAD {
                assert_eq!(self.sv.script_aborts(), Vec::<String>::new());
                return sb.server_time;
            }
        }
        panic!("B was not killed");
    }
}

#[test]
fn a_kill_replays_the_killer_from_nine_seconds_back_then_returns() {
    let Some(mut rig) = Rig::new() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let p = &PROTOCOL_V1;
    let killed_at = rig.walk_then_kill();

    // From dm's two-second `delay` on, B is sent A as the archive had it
    // `delay + 7` seconds back, for exactly that long.
    let mut replay = Vec::new();
    let mut after = None;
    for _ in 0..300 {
        let sb = rig.step(&NULL_USERCMD, &NULL_USERCMD);
        if sb.ps.field_i32(p, "pm_flags") & PMF_FOLLOW != 0 {
            replay.push(sb);
        } else if !replay.is_empty() {
            after = Some(sb);
            break;
        }
    }
    let first = replay.first().expect("B never saw a killcam");
    assert_eq!(first.server_time - killed_at, 2000);
    for s in &replay {
        assert_eq!(s.ps.field_i32(p, "clientNum"), rig.na as i32);
        assert_eq!(s.ps.field_i32(p, "pm_flags") & 0x70000, 0x30000);
        assert_eq!(s.ps.field_i32(p, "deltaTime"), 9000);
        let then = rig.trail[&(s.server_time - 9000)];
        assert_eq!(s.ps.origin(p), then, "at {}", s.server_time);
        assert!(!s.entities.contains_key(&(rig.na as u32)));
    }
    // B was alive nine seconds before, so the replay carries B's own entity.
    assert!(first.entities.contains_key(&(rig.nb as u32)));
    // The bars, title, skip text and timer are B's own unarchived elements,
    // from the frame after the first.
    assert!(first.ps.arrays.hud_current.is_empty());
    assert_eq!(replay[1].ps.arrays.hud_current.len(), 5);

    // `sessionstate = "dead"` with the copy still in the playerstate: B is a
    // dead client again, where the replay left it, until the use press.
    let after = after.expect("the killcam never ended");
    assert_eq!(after.server_time - first.server_time, 9000);
    assert_eq!(after.ps.field_i32(p, "clientNum"), rig.nb as i32);
    assert_eq!(after.ps.field_i32(p, "pm_type"), PM_DEAD);
    assert_eq!(after.ps.origin(p), replay.last().unwrap().ps.origin(p));
    // The spawn's clear goes out with that frame, both HUD arrays empty, and
    // the next frame has the round clock back.
    assert!(after.ps.arrays.hud_archived.is_empty() && after.ps.arrays.hud_current.is_empty());
    let next = rig.step(&NULL_USERCMD, &NULL_USERCMD);
    assert_eq!(next.ps.arrays.hud_archived.len(), 1);
    // The spawn's memset took the weapons with it, and nothing gives them
    // back while dead (the retail capture's `clip=- ammo=-` throughout).
    for _ in 0..5 {
        let s = rig.step(&NULL_USERCMD, &NULL_USERCMD);
        assert_eq!(s.ps.field_i32(p, "weapon"), 0);
        assert!(s.ps.arrays.ammoclip.iter().all(|c| *c == 0));
    }
    let use_ = UserCmd {
        buttons: BUTTON_USE,
        ..NULL_USERCMD
    };
    for _ in 0..10 {
        rig.step(&NULL_USERCMD, &use_);
    }
    let sb = rig.cb.snapshots().newest().unwrap();
    assert_eq!(sb.ps.field_i32(p, "pm_type"), 0, "B is playing again");
    assert_eq!(sb.ps.field_i32(p, "clientNum"), rig.nb as i32);
    assert_eq!(rig.sv.script_aborts(), Vec::<String>::new());
}

/// Retail's skip: one use press ends the replay and `waitRespawnButton`,
/// past its `wait 0`, reads the same press, so the frame after the last
/// replayed one is B alive at a spawn, with no dead frame between.
#[test]
fn a_use_press_skips_the_killcam_straight_to_the_respawn() {
    let Some(mut rig) = Rig::new() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let p = &PROTOCOL_V1;
    rig.walk_then_kill();
    let mut replayed = 0;
    while replayed < 20 {
        let sb = rig.step(&NULL_USERCMD, &NULL_USERCMD);
        if sb.ps.field_i32(p, "pm_flags") & PMF_FOLLOW != 0 {
            replayed += 1;
        }
    }
    let use_ = UserCmd {
        buttons: BUTTON_USE,
        ..NULL_USERCMD
    };
    let pressed = rig.step(&NULL_USERCMD, &use_);
    let next = rig.step(&NULL_USERCMD, &use_);
    let (a, b) = (&pressed, &next);
    let back = if a.ps.field_i32(p, "pm_flags") & PMF_FOLLOW == 0 {
        a
    } else {
        b
    };
    assert_eq!(back.ps.field_i32(p, "clientNum"), rig.nb as i32);
    assert_eq!(back.ps.field_i32(p, "pm_type"), 0, "a dead frame between");
    assert_eq!(back.ps.health(), 100);
    assert_eq!(rig.sv.script_aborts(), Vec::<String>::new());
}
