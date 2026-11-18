//! A United Offensive tank's main gun on ours against retail's
//! `*-uo-tank.bin` capture (a `--probe-vehicle` driver on mp_cassino tdm,
//! `client-probes/probe_vehicle`; recipe in docs/research/coduo-diff.md
//! 19). Ours boards the same Panzer IV through the same probe gametype,
//! fires the main gun at the wall ahead, then the coaxial and the gunner's
//! MGs, and records the events its own snapshots carry
//! (docs/research/coduo-vehicles.md 14, `vehicle_fire_ab`).
//!
//! `VEHFIRE_REPORT=1` prints both sides' events and our probe's log lines,
//! `VEHFIRE_ENTS=1` every non-body entity per snapshot. Needs `COD_DIR` at a UO
//! install; without one it returns early.

use crate::common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::NetEvent;
use vcod_common::net::event_ids::{EV_FIRE_WEAPONB, EV_UO_BULLET_HIT, EV_UO_FIRE_WEAPONC};
use vcod_common::net::events::{EventTracker, GameEvent};
use vcod_common::net::msg::{BUTTON_ATTACK, BUTTON_MELEE, BUTTON_USE, UserCmd};
use vcod_common::net::protocol::{Cod1Wire, PROTOCOL_V22};

const MAP: &str = "mp_cassino";
const PROBE: &str = "probe_vehicle";

/// One event with the server time of the snapshot that brought it.
struct Seen {
    t: i32,
    ev: GameEvent,
}

fn report(side: &str, seen: &[Seen]) {
    if std::env::var_os("VEHFIRE_REPORT").is_none() {
        return;
    }
    for s in seen {
        let e = &s.ev;
        println!(
            "{side} t {} ev {:#x} on {} eType {} parm {} weapon {} scale {} surf {} eFlags {:#x} pos {:?}",
            s.t,
            e.event,
            e.entity_num,
            e.e_type,
            e.parm,
            e.weapon,
            e.scale,
            e.surf_type,
            e.eflags,
            e.pos.map(|v| v.round() as i32),
        );
    }
}

/// `VEHFIRE_ENTS=1`: every entity a snapshot carries but bodies, sound
/// blends and vehicles.
fn dump_ents(side: &str, s: &vcod_common::net::snapshot::Snapshot, p: &Cod1Wire) {
    if std::env::var_os("VEHFIRE_ENTS").is_none() {
        return;
    }
    for (n, e) in &s.entities {
        let t = e.field_i32(p, "eType");
        if !matches!(t, 1 | 3 | 8 | 9 | 12) {
            eprintln!(
                "{side} t {} ent {n} eType {t} pos {:?} tr {} {} seq {} ev {:?}",
                s.server_time,
                e.origin(p).map(|v| v.round() as i32),
                e.field_i32(p, "pos.trType"),
                e.field_i32(p, "pos.trTime"),
                e.field_i32(p, "eventSequence"),
                [0, 1, 2, 3].map(|k| e.field_i32(p, &format!("events[{k}]"))),
            );
        }
    }
}

/// The retail capture's events in 1.1's numbering.
fn retail(tag: &str) -> Vec<Seen> {
    let p = &PROTOCOL_V22;
    let frames = vcod_common::testing::fixture_frames(
        p,
        &format!("net/gamestate-uo-{tag}.bin"),
        &format!("net/snapshots-uo-{tag}.bin"),
    );
    let mut tracker = EventTracker::new();
    let mut out = Vec::new();
    for f in &frames {
        dump_ents("retail", &f.snapshot, p);
        for ev in tracker.drain(&f.snapshot, p) {
            out.push(Seen {
                t: f.snapshot.server_time,
                ev,
            });
        }
    }
    out
}

fn deg_to_short(d: f32) -> i32 {
    ((d * 65536.0 / 360.0).round() as i32) & 0xffff
}

/// [`ours`] once for every gate in this file.
fn ours_run() -> &'static [Seen] {
    static RUN: std::sync::OnceLock<Vec<Seen>> = std::sync::OnceLock::new();
    RUN.get_or_init(|| ours(10.0, 12))
}

/// Ours: the probe boards a Panzer IV as its driver, looks `pitch` degrees
/// down, taps the main gun a second in, holds the coaxial MG (0x20) from
/// 2.5 s to 3 s, moves to the gunner's seat at 4 s and fires its MG from
/// 6 s to 6.5 s, then holds for the rest of `secs`; every event its
/// snapshots carried.
fn ours(pitch: f32, secs: i64) -> Vec<Seen> {
    let fs = vcod_common::testing::uo_fs().unwrap();
    let p: &'static Cod1Wire = &PROTOCOL_V22;
    let bsp_path = fs.resolve_map(MAP).expect("the map in the UO paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, PROBE), now);
    sv.set_protocol(p);
    sv.overlay_script(
        &format!("maps/mp/gametypes/{PROBE}"),
        include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_vehicle.gsc"),
    );
    sv.set_cvar("probe_vehicletype", "panzeriv_mp");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let mut cl = common::connect(&mut sv, &q, &mut now);
    let mut join = common::Join::new("axis", "kar98k_mp");
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let mut tracker = EventTracker::new();
    let mut out = Vec::new();
    let mut placed_at: Option<i64> = None;
    let mut seated_at: Option<i64> = None;
    let mut seat_yaw: Option<f32> = None;
    let mut last_sent = 0;
    for i in 0..(secs * 1000 / common::FRAME_MS) {
        now += frame;
        let since = |at: Option<i64>| at.map(|a| (i - a) * common::FRAME_MS);
        // Retail's `PROBE activated` is 2050 past its `PROBE place`
        // (coduo-vehicles.md 13, "The driver's idle").
        let tap =
            since(placed_at).is_some_and(|ms| (2050..2200).contains(&ms)) && seated_at.is_none();
        let seated = since(seated_at);
        let held = |from, to| seated.is_some_and(|ms| (from..to).contains(&ms));
        let buttons = if tap { BUTTON_USE } else { 0 }
            | if held(1000, 1150) || held(6000, 6500) {
                BUTTON_ATTACK
            } else {
                0
            }
            | if held(2500, 3000) { BUTTON_MELEE } else { 0 };
        if seated == Some(4000) {
            cl.send_reliable("nextvehslot");
        }
        if let Some(s) = cl.snapshots().newest() {
            let da = ["delta_angles[0]", "delta_angles[1]", "delta_angles[2]"]
                .map(|n| s.ps.field_i32(p, n));
            let v = s.ps.viewangles(p);
            let want = match seat_yaw {
                Some(y) => [pitch, y, 0.0],
                None => v,
            };
            let cmd = UserCmd {
                server_time: s.server_time.max(last_sent + 1),
                angles: std::array::from_fn(|k| (deg_to_short(want[k]) - da[k]) & 0xffff),
                buttons,
                weapon: s.ps.field_i32(p, "weapon") as u8,
                ..UserCmd::default()
            };
            last_sent = cmd.server_time;
            cl.send_cmds(&[cmd]);
        } else if let Some(c) = cl.send_frame(&UserCmd::default()) {
            last_sent = c.server_time;
        }
        for e in common::step(&mut sv, &q, &mut cl, now) {
            if let NetEvent::ServerCommand(tokens) = e {
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        if placed_at.is_none() && sv.script_log().iter().any(|l| l.contains("PROBE place")) {
            placed_at = Some(i);
        }
        if let Some(s) = cl.snapshots().newest() {
            if seated_at.is_none()
                && s.ps.field_i32(p, "eFlags") & vcod_common::net::flags::EF_IN_VEHICLE_V22 != 0
            {
                seated_at = Some(i);
                seat_yaw = Some(s.ps.viewangles(p)[1]);
            }
            dump_ents("ours", s, p);
            for ev in tracker.drain(s, p) {
                out.push(Seen {
                    t: s.server_time,
                    ev,
                });
            }
        }
    }
    if std::env::var_os("VEHFIRE_REPORT").is_some() {
        for l in sv.script_log().iter().filter(|l| l.contains("PROBE")) {
            print!("ours log {l}");
        }
    }
    assert!(seated_at.is_some(), "never got into the tank");
    out
}

const EV_EXPLODE: i32 = vcod_common::net::event_ids::EV_UO_PROJECTILE_EXPLODE;

/// What the client's explode dispatch reads (`fx::registry`, the UO
/// explode arm): the row comes from `es.scale` and the vehicle bit, the
/// column from `surfType`, the second effect from the weapon.
fn dispatch(e: &GameEvent) -> (i32, i32, i32, i32, i32) {
    (
        e.event,
        e.e_type,
        e.scale,
        e.weapon,
        e.eflags & vcod_common::net::flags::EF_IN_VEHICLE_V22,
    )
}

/// The Panzer IV's main gun: retail's two shells explode as type 6 on
/// `eType` 0 with the gun's weapon and no vehicle bit, the second one
/// first seen already exploded, which the client's ring still plays
/// (`CG_ResetEntity`); ours, fired at a wall a frame's flight away, sends
/// the same explode, also first seen exploded. Along retail's first
/// shell's line our missile trace stops on the face retail's struck and
/// reads its surface and normal, so both resolve to the same
/// `tank_explode` effect (docs/research/coduo-vehicles.md 13).
#[test]
fn the_tank_shell_explodes_as_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let p = &PROTOCOL_V22;
    let retail = retail("tank");
    let ours = ours_run();
    report("retail", &retail);
    report("ours", ours);
    let gun = vcod_server::defs::UO
        .weapon_index("panzeriv_turret_mp")
        .unwrap() as i32;
    let want = (EV_EXPLODE, 0, 6, gun, 0);
    let shells: Vec<&Seen> = retail.iter().filter(|s| s.ev.event == EV_EXPLODE).collect();
    assert_eq!(shells.len(), 2, "retail's two shells");
    for s in &shells {
        assert_eq!(dispatch(&s.ev), want, "retail at {}", s.t);
    }
    let mine: Vec<&Seen> = ours.iter().filter(|s| s.ev.event == EV_EXPLODE).collect();
    assert_eq!(mine.len(), 1, "our shell");
    assert_eq!(dispatch(&mine[0].ev), want);
    assert_eq!(mine[0].ev.eflags, shells[0].ev.eflags, "the shell's 0x8000");

    // Retail's first shell in flight, then where it went off.
    let frames = vcod_common::testing::fixture_frames(
        p,
        "net/gamestate-uo-tank.bin",
        "net/snapshots-uo-tank.bin",
    );
    let first = &shells[0].ev;
    let base = frames
        .iter()
        .find_map(|f| {
            let e = f.snapshot.entities.get(&first.entity_num)?;
            (e.field_i32(p, "eType") == 4).then(|| glam::Vec3::from(e.origin(p)))
        })
        .expect("the shell in flight");
    let at = glam::Vec3::from(first.pos);
    let bsp_path = fs.resolve_map(MAP).unwrap();
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let world = vcod_server::world::World::from_bsp(&bsp, Some(&fs));
    let tr = world
        .collision
        .missile_trace(base, base + (at - base) * 1.2);
    assert!(
        tr.fraction < 1.0 && (tr.endpos - at).length() < 2.0,
        "{tr:?} {at}"
    );
    assert_eq!(
        (
            vcod_common::collision::sound_material(tr.surface_flags),
            vcod_common::net::events::dir_to_byte(tr.normal.into())
        ),
        (first.surf_type, first.parm),
        "the struck face"
    );
}

/// Retail's capture and ours on the vehicle MGs: every coaxial (164) and
/// gunner (165) round that struck no player carries its weapon with bit 7,
/// no bullet-hit temp entity, so the client traces it itself
/// (`fx::client_bullet`).
#[test]
fn the_tank_mgs_leave_their_rounds_to_the_client_as_on_uo() {
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    let retail = retail("tank");
    let ours = ours_run();
    let parms = |run: &[Seen], event| {
        let mut v: Vec<i32> = run
            .iter()
            .filter(|s| s.ev.event == event)
            .map(|s| s.ev.parm)
            .collect();
        v.dedup();
        v
    };
    // The Panzer IV's coaxial mg34_tank_mp (27) and gunner's mg42_turret_mp
    // (32), each with bit 7.
    assert_eq!(parms(&retail, EV_FIRE_WEAPONB), [0x9b]);
    assert_eq!(parms(&retail, EV_UO_FIRE_WEAPONC), [0xa0]);
    for (side, run) in [("retail", retail.as_slice()), ("ours", ours)] {
        for event in [EV_FIRE_WEAPONB, EV_UO_FIRE_WEAPONC] {
            assert_eq!(
                parms(run, event),
                parms(&retail, event),
                "{side} {event:#x}"
            );
        }
        assert!(
            !run.iter().any(|s| s.ev.event == EV_UO_BULLET_HIT),
            "{side}: a bullet-hit temp entity"
        );
    }
}
