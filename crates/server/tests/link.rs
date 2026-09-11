//! What `sd.gsc`'s plant does to the planting client: `linkTo` pins it at
//! `pm_type` 1 with no ground entity and no velocity, and the abort's
//! `unlink` lets go (docs/research/cod11-gsc-object-model.md, 23.2).
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::{step_pair, Queues, FRAME_MS};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{UserCmd, BUTTON_USE, NULL_USERCMD};
use vcod_common::net::protocol::PROTOCOL_V1;

const MAP: &str = "mp_carentan";
/// `ENTITYNUM_NONE` as the wire carries it.
const ENTITYNUM_NONE: i32 = 1023;
/// Where the retail attacker planted from, the plant fixture's `# station`
/// line: inside `bombzone_A`'s brush and on its floor.
const STATION: [f32; 3] = [-225.0, 2452.0, -22.0];

fn server(now: Instant) -> Option<vcod_server::Server> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "sd"), now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    Some(sv)
}

/// The plant's own `other linkTo(self)`: use held inside `bombzone_A` puts
/// the attacker at `pm_type` 1, a forward cmd then moves it nowhere, and the
/// release takes the abort branch's `other unlink()`.
#[test]
fn a_planting_client_is_linked_and_the_abort_releases_it() {
    let p = &PROTOCOL_V1;
    let mut now = Instant::now();
    let Some(mut sv) = server(now) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let qa = Rc::new(RefCell::new(Queues::default()));
    let qb = Rc::new(RefCell::new(Queues::default()));
    let (mut ca, mut cb) = common::join_pair(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("allies", "m1carbine_mp"),
        ("axis", "kar98k_mp"),
    );
    // Past the match-start restart, which is where `_gameobjects` runs again
    // and the bombzones become plantable.
    for _ in 0..200 {
        now += Duration::from_millis(FRAME_MS as u64);
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&NULL_USERCMD);
        step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new());

    sv.place_client(0, STATION, 0.0);
    // The plant wants the weapon the playerstate holds on every cmd: a
    // `weapon` of 0 reads as a holster request (combat doc, 1.8).
    let weapon = ca
        .snapshots()
        .newest()
        .expect("a snapshot")
        .ps
        .field_i32(p, "weapon") as u8;
    let hold = UserCmd {
        buttons: BUTTON_USE,
        weapon,
        ..NULL_USERCMD
    };
    let walk = UserCmd {
        forward: 127,
        ..hold
    };

    // 60 frames of use: the touch pass fires `bombzone_A`, `bomb_think`
    // starts planting and links the planter.
    for _ in 0..60 {
        now += Duration::from_millis(FRAME_MS as u64);
        ca.send_frame(&hold);
        cb.send_frame(&NULL_USERCMD);
        step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
    }
    let s = ca.snapshots().newest().expect("a snapshot");
    assert_eq!(
        s.ps.field_i32(p, "pm_type"),
        1,
        "the planter is not linked at PM_NORMAL_LINKED"
    );
    assert_eq!(
        s.ps.field_i32(p, "groundEntityNum"),
        ENTITYNUM_NONE,
        "a linked client reads no ground entity on either retail capture"
    );
    for axis in ["velocity[0]", "velocity[1]", "velocity[2]"] {
        assert_eq!(s.ps.field_f32(p, axis), 0.0, "{axis} under the link");
    }
    let before = s.ps.origin(p);

    // 30 more with a walk input held. Retail's capture sent 92 forward cmds
    // under the link and the origin held to the tenth of a unit (23.2).
    for _ in 0..30 {
        now += Duration::from_millis(FRAME_MS as u64);
        ca.send_frame(&walk);
        cb.send_frame(&NULL_USERCMD);
        step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
    }
    let s = ca.snapshots().newest().expect("a snapshot");
    assert_eq!(s.ps.field_i32(p, "pm_type"), 1, "the walk broke the link");
    let after = s.ps.origin(p);
    let moved = ((after[0] - before[0]).powi(2)
        + (after[1] - before[1]).powi(2)
        + (after[2] - before[2]).powi(2))
    .sqrt();
    assert!(moved < 2.0, "a linked client walked {moved} units");

    // Released: the abort branch calls `other unlink()`.
    for _ in 0..40 {
        now += Duration::from_millis(FRAME_MS as u64);
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&NULL_USERCMD);
        step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
    }
    let s = ca.snapshots().newest().expect("a snapshot");
    assert_eq!(s.ps.field_i32(p, "pm_type"), 0, "the planter stayed linked");
}
