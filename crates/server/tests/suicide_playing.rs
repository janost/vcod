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
