//! What `sd.gsc`'s plant does to the planting client: `linkTo` pins it at
//! `pm_type` 1 with no ground entity and no velocity, and the abort's
//! `unlink` lets go (docs/research/cod11-gsc-object-model.md, 23.2), and
//! what `setOrigin` does to a player, which the S&D probe moves both clients
//! with.
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::{step_pair, ClientEnd, Join, Queues, FRAME_MS};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{UserCmd, BUTTON_USE, NULL_USERCMD};
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_common::net::{NetClient, NetEvent};

const MAP: &str = "mp_carentan";
/// `ENTITYNUM_NONE` as the wire carries it.
const ENTITYNUM_NONE: i32 = 1023;
/// Where the retail attacker planted from, the plant fixture's `# station`
/// line: inside `bombzone_A`'s brush and on its floor.
const STATION: [f32; 3] = [-225.0, 2452.0, -22.0];
/// `EF_TELEPORT_BIT`.
const EF_TELEPORT: i32 = 0x8;

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
    // A planter that stood still links with no velocity. The z is held to
    // under a unit, not to 0: retail's `PmoveSingle` snaps the velocity to
    // integers and ours does not, so a grounded client carries a sub-unit z.
    for axis in ["velocity[0]", "velocity[1]"] {
        assert_eq!(s.ps.field_f32(p, axis), 0.0, "{axis} under the link");
    }
    assert!(s.ps.field_f32(p, "velocity[2]").abs() < 0.5);
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

/// A client that links while moving keeps the velocity it linked with:
/// retail's abort reads `velocity` 184,27 on every linked snapshot and on
/// the release frame, while the origin holds (object-model doc, 23.2).
#[test]
fn a_client_linked_on_the_move_keeps_its_velocity() {
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
    for _ in 0..200 {
        now += Duration::from_millis(FRAME_MS as u64);
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&NULL_USERCMD);
        step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
    }
    sv.place_client(0, STATION, 0.0);
    let weapon = ca
        .snapshots()
        .newest()
        .expect("a snapshot")
        .ps
        .field_i32(p, "weapon") as u8;
    // A few frames of walk to build speed, then use on top of it: the live
    // probe's planter is still moving when its first use cmd goes out.
    let walk = UserCmd {
        forward: 127,
        weapon,
        ..NULL_USERCMD
    };
    let walk_use = UserCmd {
        buttons: BUTTON_USE,
        ..walk
    };
    let mut linked = Vec::new();
    for i in 0..40 {
        now += Duration::from_millis(FRAME_MS as u64);
        ca.send_frame(if i < 4 { &walk } else { &walk_use });
        cb.send_frame(&NULL_USERCMD);
        step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
        let s = ca.snapshots().newest().expect("a snapshot");
        if s.ps.field_i32(p, "pm_type") == 1 {
            let v = ["velocity[0]", "velocity[1]", "velocity[2]"].map(|a| s.ps.field_f32(p, a));
            linked.push((s.ps.origin(p), v));
        }
    }
    assert!(linked.len() > 10, "the planter never linked");
    let (origin, velocity) = linked[0];
    assert!(
        velocity[0].hypot(velocity[1]) > 10.0,
        "a client linked mid-walk reads velocity {velocity:?}"
    );
    for (o, v) in &linked {
        assert_eq!(*o, origin, "a linked client moved");
        assert_eq!(*v, velocity, "the velocity changed under the link");
    }
}

/// `probe_lookat.gsc` under `probe_teleport 1` `setOrigin`s each player once
/// onto a courtyard spawn, as soon as it is playing. The retail attacker's
/// frame reads the origin one unit above the argument, `(-512, 2688, -15)`,
/// with the teleport bit flipped (`eFlags` 24 -> 16 at `serverTime` 68750 in
/// the plant fixture).
#[test]
fn set_origin_moves_a_player_a_unit_up_and_flips_the_teleport_bit() {
    let p = &PROTOCOL_V1;
    let mut now = Instant::now();
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "probe_lookat"), now);
    sv.overlay_script(
        "maps/mp/gametypes/probe_lookat",
        include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_lookat.gsc"),
    );
    sv.set_cvar("probe_teleport", "1");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let qa = Rc::new(RefCell::new(Queues::default()));
    let qb = Rc::new(RefCell::new(Queues::default()));
    let mut ca = NetClient::start_with_qport(ClientEnd(qa.clone()), now, 0x2001);
    let mut cb = NetClient::start_with_qport(ClientEnd(qb.clone()), now, 0x2002);
    let mut ja = Join::new("allies", "m1carbine_mp");
    let mut jb = Join::new("axis", "kar98k_mp");

    // The join inline rather than through `join_pair`: the teleport lands
    // inside it, and only the frame it lands on shows the lift.
    let mut prev_eflags = None;
    let mut landed = None;
    for _ in 0..800 {
        now += Duration::from_millis(FRAME_MS as u64);
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&NULL_USERCMD);
        let (ea, eb) = step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
        for (events, join, cl) in [(ea, &mut ja, &mut ca), (eb, &mut jb, &mut cb)] {
            for e in events {
                if let NetEvent::ServerCommand(tokens) = e {
                    join.on_server_command(&tokens, cl, now);
                }
            }
        }
        let Some(s) = ca.snapshots().newest() else {
            continue;
        };
        let origin = s.ps.origin(p);
        let eflags = s.ps.field_i32(p, "eFlags");
        if origin[0] == -512.0 && origin[1] == 2688.0 {
            landed = Some((origin, prev_eflags, eflags));
            break;
        }
        prev_eflags = Some(eflags);
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
    let (origin, before, after) = landed.expect("the attacker never reached the courtyard spawn");
    assert_eq!(origin[2], -15.0, "setOrigin lifts the argument one unit");
    assert_eq!(
        before.map(|b| (b ^ after) & EF_TELEPORT),
        Some(EF_TELEPORT),
        "the teleport frame did not flip the teleport bit"
    );
}
