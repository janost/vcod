//! `suicide()` on a player whose script keeps `sessionstate` `playing`:
//! retail's `ClientEndFrame` keys its dead arm on the session, so the body
//! stays `pm_type` 0 at health 0 until the script writes `dead`
//! (`client-probes/probe_suicide` on retail mp_harbor, 2026-10-17;
//! docs/research/cod11-combat.md 5.1, "A suicide that stays playing").
//!
//! Needs `COD_DIR`; without the paks it returns early.

use crate::common;

use common::Queues;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::NULL_USERCMD;
use vcod_common::net::protocol::PROTOCOL_V1;

const SCRIPT: &str = r#"
main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;
	maps\mp\gametypes\_callbacksetup::SetupCallbacks();
}

Callback_StartGameType() {}

Callback_PlayerConnect()
{
	self waittill("begin");
	self.sessionteam = "allies";
	self.sessionstate = "playing";
	self.maxhealth = 100;
	self.health = 100;
	sp = getentarray("mp_teamdeathmatch_spawn", "classname")[0];
	self spawn(sp.origin, sp.angles);
	wait 1;
	self suicide();
	logPrint("PROBE suicided\n");
	wait 1;
	self.sessionstate = "dead";
	logPrint("PROBE dead\n");
}

Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, attacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
"#;

#[test]
fn a_suicide_that_stays_playing_keeps_pm_type_0() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let p = &PROTOCOL_V1;
    let mut now = Instant::now();
    let bsp_path = fs
        .resolve_map("mp_harbor")
        .expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut sv = vcod_server::Server::new(common::cfg("mp_harbor", "probe_suicide_one"), now);
    sv.overlay_script("maps/mp/gametypes/probe_suicide_one", SCRIPT);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = common::connect(&mut sv, &q, &mut now);
    // (phase, pm_type) per snapshot: 0 alive, 1 suicided, 2 dead session.
    let mut seen = Vec::new();
    for _ in 0..80 {
        now += Duration::from_millis(50);
        cl.send_frame(&NULL_USERCMD);
        common::step(&mut sv, &q, &mut cl, now);
        let phase = sv
            .script_log()
            .iter()
            .filter(|l| l.starts_with("PROBE "))
            .count();
        if let Some(s) = cl.snapshots().newest() {
            seen.push((phase, s.ps.field_i32(p, "pm_type")));
        }
    }
    // A snapshot goes out after the frame's end frame, so the phase a line
    // logs in is already the phase the snapshot carries.
    let in_phase = |n| {
        seen.iter()
            .filter(move |(ph, _)| *ph == n)
            .map(|(_, pm)| *pm)
    };
    assert!(in_phase(1).count() > 5, "{seen:?}");
    assert!(in_phase(1).all(|pm| pm == 0), "{seen:?}");
    assert!(in_phase(2).count() > 5, "{seen:?}");
    assert!(in_phase(2).all(|pm| pm == 6), "{seen:?}");
}

/// `client-probes/probe_suicide` with `probe_suicide_near 1` against
/// retail, two clients 64 units apart (2026-11-14, recipe in the probes'
/// README): after the `suicide()` slot 0's trace from his eye misses
/// slot 1, whose `player_die` relinked him as a corpse; 3 s on, still
/// `playing`, his end frame has linked him back as a body and the trace
/// stops on him, and still does right after the script writes `dead`
/// (cod11-combat.md 5.1, "A suicide that stays playing").
fn probe_suicide_near(
    fs: vcod_common::pk3::Pk3Fs,
    map: &str,
    proto: &'static vcod_common::net::protocol::Cod1Wire,
) -> Vec<String> {
    let mut now = Instant::now();
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_suicide"), now);
    sv.set_protocol(proto);
    sv.overlay_script(
        "maps/mp/gametypes/probe_suicide",
        include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_suicide.gsc"),
    );
    sv.set_cvar("probe_suicide_near", "1");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    common::run_alone(&mut sv, &mut now, 8000);
    let qa = Rc::new(RefCell::new(Queues::default()));
    let qb = Rc::new(RefCell::new(Queues::default()));
    let mut a = common::connect(&mut sv, &qa, &mut now);
    for _ in 0..40 {
        now += Duration::from_millis(50);
        a.send_frame(&NULL_USERCMD);
        common::step(&mut sv, &qa, &mut a, now);
    }
    let mut b =
        vcod_common::net::NetClient::start_with_qport(common::ClientEnd(qb.clone()), now, 0x2222);
    for _ in 0..500 {
        now += Duration::from_millis(50);
        a.send_frame(&NULL_USERCMD);
        b.send_frame(&NULL_USERCMD);
        common::step_pair(&mut sv, (&qa, &mut a), (&qb, &mut b), now);
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
    }
    sv.script_log()
        .iter()
        .filter(|l| l.starts_with("PROBE "))
        .cloned()
        .collect()
}

/// Retail 1.1d on mp_harbor.
#[test]
fn a_playing_suicide_is_traced_again_as_retail_11d_traces_it() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let retail = [
        "PROBE begin 0 at (-7352.00, -7976.00, 115.99)",
        "PROBE begin 1 at (-7288.00, -7976.00, 115.99)",
        "PROBE step alive health 100 state playing trace 1 origin (-7288.00, -7976.00, 0.12)",
        "PROBE killed 1 MOD_SUICIDE health 0",
        "PROBE step suicide_playing health 0 state playing trace none origin (-7288.00, -7976.00, 0.12)",
        "PROBE step suicide_playing2 health 0 state playing trace 1 origin (-7288.00, -7976.00, 0.12)",
        "PROBE step dead_session health 0 state dead trace 1 origin (-7288.00, -7976.00, 0.12)",
        "PROBE step respawned health 100 state playing trace none origin (-8160.00, -7000.00, 115.99)",
        "PROBE done",
    ];
    assert_eq!(probe_suicide_near(fs, "mp_harbor", &PROTOCOL_V1), retail);
}

/// Retail UO 1.51b on mp_uo_harbor. Ours prints the resting body's `z`
/// as 0.13 where retail prints 0.12 (both playerstates carry 0.125; open,
/// cod11-combat.md 5.1), so the lines are compared up to the origin.
#[test]
fn a_playing_suicide_is_traced_again_as_retail_uo_traces_it() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let retail = [
        "PROBE begin 0 at (-7352.00, -7976.00, 115.99)",
        "PROBE begin 1 at (-7288.00, -7976.00, 115.99)",
        "PROBE step alive health 100 state playing trace 1",
        "PROBE killed 1 MOD_SUICIDE health 0",
        "PROBE step suicide_playing health 0 state playing trace none",
        "PROBE step suicide_playing2 health 0 state playing trace 1",
        "PROBE step dead_session health 0 state dead trace 1",
        "PROBE step respawned health 100 state playing trace none",
        "PROBE done",
    ];
    let ours: Vec<String> = probe_suicide_near(
        fs,
        "mp_uo_harbor",
        &vcod_common::net::protocol::PROTOCOL_V22,
    )
    .into_iter()
    .map(|l| match l.split_once(" origin (") {
        Some((head, _)) if l.starts_with("PROBE step ") => head.to_string(),
        _ => l,
    })
    .collect();
    assert_eq!(ours, retail);
}
