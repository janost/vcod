//! A tank's death on ours against retail's capture of it
//! (`crates/common/tests/fixtures/net/*-uo-wreck.bin`, an axis player under
//! `client-probes/probe_vehwreck` on mp_cassino; recipe in
//! docs/research/coduo-vehicles.md 15.2). Ours runs the same probe with a
//! client joined as axis and records the events its snapshots carry.
//!
//! `WRECK_REPORT=1` prints both sides' events. Needs `COD_DIR` at a UO
//! install; without one it returns early.

use crate::common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::NetEvent;
use vcod_common::net::event_ids::{EV_EARTHQUAKE, EV_SOUND_ALIAS};
use vcod_common::net::events::{EventTracker, GameEvent};
use vcod_common::net::msg::UserCmd;
use vcod_common::net::protocol::PROTOCOL_V22;

const MAP: &str = "mp_cassino";
const PROBE: &str = "probe_vehwreck";

/// Each event with the server time of the snapshot that brought it.
fn retail() -> Vec<(i32, GameEvent)> {
    let p = &PROTOCOL_V22;
    let mut tracker = EventTracker::new();
    let mut out = Vec::new();
    for f in vcod_common::testing::fixture_frames(
        p,
        "net/gamestate-uo-wreck.bin",
        "net/snapshots-uo-wreck.bin",
    ) {
        let s = &f.snapshot;
        out.extend(
            tracker
                .drain(s, p)
                .into_iter()
                .map(|e| (s.server_time.raw(), e)),
        );
    }
    out
}

fn ours(secs: i64) -> Vec<(i32, GameEvent)> {
    let fs = vcod_common::testing::uo_fs().unwrap();
    let p = &PROTOCOL_V22;
    let src = format!("../gsc/tests/fixtures/semantics/client-probes/{PROBE}.gsc");
    let probe = std::fs::read_to_string(src).expect("read the probe");
    let (mut sv, mut now) = common::Boot {
        proto: Some(p),
        script: Some(&probe),
        ..common::Boot::new(MAP, PROBE)
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let mut cl = common::connect(&mut sv, &q, &mut now);
    let mut join = common::Join::new("axis", "kar98k_mp");
    let mut tracker = EventTracker::new();
    let mut out = Vec::new();
    for _ in 0..secs * 1000 / common::FRAME_MS {
        now += Duration::from_millis(common::FRAME_MS as u64);
        cl.send_frame(&UserCmd::default());
        for e in common::step(&mut sv, &q, &mut cl, now) {
            if let NetEvent::ServerCommand(tokens) = e {
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        if let Some(s) = cl.snapshots().newest() {
            out.extend(
                tracker
                    .drain(s, p)
                    .into_iter()
                    .map(|e| (s.server_time.raw(), e)),
            );
        }
    }
    if std::env::var_os("WRECK_REPORT").is_some() {
        for l in sv.script_log() {
            print!("ours log {l}");
        }
    }
    out
}

fn report(side: &str, run: &[(i32, GameEvent)]) {
    if std::env::var_os("WRECK_REPORT").is_none() {
        return;
    }
    for (t, e) in run {
        println!(
            "{side} t {t} ev {} on {} parm {} pos {:?}",
            e.event, e.entity_num, e.parm, e.pos
        );
    }
}

/// The kill's earthquake time and position, then each `EV_SOUND_ALIAS` a
/// temp entity carried: its time past the kill, parm and offset from the
/// quake.
fn wreck_sounds(run: &[(i32, GameEvent)]) -> (i32, Vec<(i32, i32, [f32; 3])>) {
    let (t0, quake) = run
        .iter()
        .find(|(_, e)| e.event == EV_EARTHQUAKE)
        .expect("the kill's earthquake");
    let sounds = run
        .iter()
        .filter(|(_, e)| e.event == EV_SOUND_ALIAS && e.entity_num != u32::MAX)
        .filter(|(t, _)| *t < t0 + 10_400)
        .map(|(t, e)| (t - t0, e.parm, [0, 1, 2].map(|i| e.pos[i] - quake.pos[i])))
        .collect();
    (*t0, sounds)
}

/// 100 ms after `spawnDuplicate`, the duplicate (unlinked, but a brush
/// model) runs its first step's collision probes against the burning tank
/// it stands in and plays `impactSnd` 2500 units above the hit
/// (coduo-vehicles.md 15.2): the one sound alias a temp entity carries.
#[test]
fn the_wrecks_duplicate_plays_its_impact_sound() {
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    let r = retail();
    let o = ours(48);
    report("retail", &r);
    report("ours", &o);
    let (_, rs) = wreck_sounds(&r);
    let (_, os) = wreck_sounds(&o);
    assert_eq!(rs.len(), 1, "{rs:?}");
    assert_eq!(os.len(), rs.len(), "retail {rs:?}, ours {os:?}");
    let (rt, rp, roff) = rs[0];
    let (ot, op, ooff) = os[0];
    assert_eq!((ot, op), (rt, rp), "retail {rs:?}, ours {os:?}");
    for i in 0..3 {
        assert!(
            (ooff[i] - roff[i]).abs() < 2.0,
            "retail {rs:?}, ours {os:?}"
        );
    }
}

/// The duplicate's first think strikes the wreck it stands in and, its
/// stuck clock 0 from the spawn, takes 0x827ab's stuck kill: 999999
/// `MOD_COLLISION` on itself, scaled by the hit's side, so it dies unseen
/// and waits out its respawn at -999 (coduo-vehicles.md 15.2). Retail's
/// lines under `client-probes/probe_vehdupe`, no client joined.
#[test]
fn the_wrecks_duplicate_takes_the_stuck_kill() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let probe = "probe_vehdupe";
    let fixture = include_str!("fixtures/vehicles/mp_cassino-tank-dupe-uo.txt");
    let retail: Vec<&str> = fixture.lines().filter(|l| l.starts_with("PROBE")).collect();
    let src = format!("../gsc/tests/fixtures/semantics/client-probes/{probe}.gsc");
    let gsc = std::fs::read_to_string(src).expect("read the probe");
    let (mut sv, mut now) = common::Boot {
        proto: Some(&PROTOCOL_V22),
        script: Some(&gsc),
        ..common::Boot::new(MAP, probe)
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, 14_000);
    let log = sv.script_log();
    let ours: Vec<&str> = log
        .iter()
        .map(|l| l.trim_end())
        .filter(|l| l.starts_with("PROBE"))
        .collect();
    // In order: `G_Damage` notifies `damage` and then `death`, and the
    // frame's pass runs the newest wake first, so `death` logs ahead
    // (coduo-vehicles.md 15.2).
    assert_eq!(ours, retail);
}

/// A stock vehicle's first think runs the collision probes once (it
/// settled at init), and a hit there takes the stuck kill. Retail's stock
/// vehicles start whole, so on ours none may strike anything then: every
/// UO map's tdm vehicles still hold `init_tank`'s health 3 s in.
#[test]
fn no_stock_vehicle_takes_the_stuck_kill_at_the_start() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let probe = "probe_vehstart";
    let gsc = "main()\n{\n\tthread w();\n\tmaps\\mp\\gametypes\\tdm::main();\n}\n\nw()\n{\n\twait 3;\n\tv = getentarray(\"script_vehicle\", \"classname\");\n\tfor (i = 0; i < v.size; i++)\n\t\tlogPrint(\"PROBE v \" + v[i] getEntityNumber() + \" \" + v[i].health + \"\\n\");\n}\n";
    let fs = Rc::new(fs);
    let mut dead = Vec::new();
    let mut seen = 0;
    for map in [
        "mp_arnhem",
        "mp_berlin",
        "mp_cassino",
        "mp_foy",
        "mp_italy",
        "mp_kharkov",
        "mp_kursk",
        "mp_ponyri",
        "mp_rhinevalley",
        "mp_sicily",
        "mp_uo_stanjel",
        "mp_peaks",
        "mp_streets",
    ] {
        let Some(path) = fs.resolve_map(map) else {
            continue;
        };
        let bsp = vcod_common::bsp::parse(&fs.read(&path).unwrap()).unwrap();
        let mut now = Instant::now();
        let mut sv = vcod_server::Server::new(common::cfg(map, probe), now);
        sv.set_protocol(&PROTOCOL_V22);
        sv.overlay_script(&format!("maps/mp/gametypes/{probe}"), gsc);
        sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&*fs)));
        sv.load_scripts(fs.clone()).expect("load the scripts");
        common::run_alone(&mut sv, &mut now, 3_200);
        for l in sv.script_log() {
            if let Some(r) = l.trim().strip_prefix("PROBE v ") {
                seen += 1;
                if r.ends_with(" -999") {
                    dead.push(format!("{map} {r}"));
                }
            }
        }
    }
    assert!(seen > 0, "no vehicle logged");
    assert!(dead.is_empty(), "{dead:?}");
}

/// Stock `_jeepdrive_gmi::process_deactivate`'s self-destruct timer on a
/// jeep destroyed and deleted while it waits: the free ends the thread,
/// whose `self` it is, so the timer's `dodamage` never runs on the dead
/// entity (bot-objectives.md 10.5, `probe_stale_self`). Retail's lines
/// under `client-probes/probe_jeepdeact`, no client joined.
#[test]
fn a_deleted_jeeps_self_destruct_timer_ends_with_it() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let map = "mp_foy";
    let probe = "probe_jeepdeact";
    let fixture = include_str!("fixtures/vehicles/mp_foy-jeep-deactivate-uo.txt");
    let retail: Vec<&str> = fixture.lines().filter(|l| l.starts_with("PROBE")).collect();
    let src = format!("../gsc/tests/fixtures/semantics/client-probes/{probe}.gsc");
    let gsc = std::fs::read_to_string(src).expect("read the probe");
    let (mut sv, mut now) = common::Boot {
        proto: Some(&PROTOCOL_V22),
        script: Some(&gsc),
        ..common::Boot::new(map, probe)
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, 25_000);
    let log = sv.script_log();
    let ours: Vec<&str> = log
        .iter()
        .map(|l| l.trim_end())
        .filter(|l| l.starts_with("PROBE"))
        .collect();
    assert_eq!(ours, retail);
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}
