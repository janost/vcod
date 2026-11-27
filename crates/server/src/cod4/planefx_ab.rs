//! `cod4_planefx_ab`: the jets' tag effects and a dead player's cursor
//! hint (`play-1.7-planefx.txt`) on ours. Retail ran `cod4_lnxded` 1.7
//! under `+set fs_game mods/vcodprobe +map mp_crash`, `war`, sv_pure 0,
//! with the probe gsc
//! `crates/cod4/tests/fixtures/gsc/plane_fx/maps/mp/mp_crash_fx.gsc` loose
//! in the mod's homepath, and one `cod4_probe` (`--team allies --class
//! assault_mp,0 --script idle --name owner --secs 25`). A second after the
//! probe spawns the gsc spawns a jet with the stock `playPlaneFx`'s four
//! `playFxOnTag`s in its spawn frame and, a frame later, one whose four
//! come half a second on; then the probe drops his gun at his feet and
//! suicides with its hint up (cod4-client-view.md 11.1, cod4-hud.md 20).
//! Needs `COD4_DIR`.

use std::collections::VecDeque;
use std::time::Instant;

use vcod_cod4::net::delta::EntityState;

use super::test_harness::{Probe, Side, retail, run_frames, side, test_cfg};
use super::*;
use crate::game::cod4::vehicle::ET_PLANE;

/// Past the probe's suicide (49000) and its respawn (51000).
const RUN_MS: i32 = 52_000;

static GSC: vcod_common::manifest::ManifestPath =
    vcod_common::manifest_const!("/../cod4/tests/fixtures/gsc/plane_fx");

fn word(e: &EntityState, name: &str) -> u32 {
    EntityState::table(e.etype())
        .iter()
        .find(|f| f.name == name)
        .map_or(0, |f| e.0[f.word()])
}

/// A jet as it entered: its `eventSequence`, its ring (event, parm) and
/// the time past its entry its ring first moved.
type Jet = (u32, Vec<(u32, u32)>, Option<i32>);

/// Each jet's first snapshot, in spawn order: its `eventSequence` and
/// ring (event, parm), and the time past it its ring first moved.
fn jets(s: &Side) -> Vec<Jet> {
    // (number, sequence, ring, entry time, first move).
    type Seen = (u32, u32, Vec<(u32, u32)>, i32, Option<i32>);
    let mut out: Vec<Seen> = Vec::new();
    for f in &s.snaps {
        for e in f.entities.iter().filter(|e| e.etype() == ET_PLANE) {
            let seq = word(e, "eventSequence");
            match out.iter_mut().find(|j| j.0 == e.number()) {
                Some(j) => {
                    if seq != j.1 && j.4.is_none() {
                        j.4 = Some((f.server_time - j.3).raw());
                    }
                }
                None => {
                    let ring = (0..4)
                        .map(|k| {
                            (
                                word(e, &format!("events[{k}]")),
                                word(e, &format!("eventParms[{k}]")),
                            )
                        })
                        .collect();
                    out.push((e.number(), seq, ring, f.server_time.raw(), None));
                }
            }
        }
    }
    out.into_iter().map(|j| (j.1, j.2, j.4)).collect()
}

/// The owner's cursor hint words across his death: (`pm_type`,
/// `cursorHint`, `cursorHintString`, `cursorHintEntIndex`) at each change.
fn hints(s: &Side) -> Vec<(u32, u32, u32, u32)> {
    let mut out: Vec<(u32, u32, u32, u32)> = Vec::new();
    for f in &s.snaps {
        let g = |n: &str| f.ps.get(n).unwrap_or(0);
        let row = (
            g("pm_type"),
            g("cursorHint"),
            g("cursorHintString"),
            g("cursorHintEntIndex"),
        );
        if out.last() != Some(&row) {
            out.push(row);
        }
    }
    out
}

/// The owner on ours: joins on retail's clock, allies and the assault
/// class at its twin's times, spawned on its twin's spot.
fn ours(install: &vcod_cod4::install::Install, r: &Side) -> (Side, Vec<String>) {
    let cs0 = String::from_utf8_lossy(r.gs.configstring(0)).into_owned();
    let mut sv = Cod4Server::new(test_cfg("mp_crash", "war"), 7);
    for (k, v) in vcod_common::net::connectionless::info_pairs(&cs0) {
        if !matches!(k, "protocol" | "shortversion" | "fs_game") {
            sv.set_cvar(k, v);
        }
    }
    sv.set_script_overrides(std::path::PathBuf::from(&*GSC));
    sv.load(Some(install)).unwrap();
    let spawn = r
        .snaps
        .iter()
        .find(|f| f.ps.get("pm_type") == Some(0) && f.ps.get("clientNum") == Some(0))
        .expect("the owner spawns");
    let yaw = spawn.ps.get_f32("viewangles[1]").unwrap_or(0.0);
    let origin = ["origin[0]", "origin[1]", "origin[2]"].map(|n| spawn.ps.get_f32(n).unwrap());
    let mut pins = vec![VecDeque::new(); crate::server::MAX_CLIENTS];
    pins[0].push_back(Some((origin, [0.0, yaw, 0.0])));
    sv.game.level.as_mut().unwrap().rt.host.spawn_pins =
        Some(std::rc::Rc::new(std::cell::RefCell::new(pins)));
    let first_snap = r.snaps[0].server_time;
    let mut probe = Probe::new(
        "owner",
        0,
        28961,
        first_snap.raw(),
        &[
            (1050, "mr {sid} 8 allies"),
            (2550, "mr {sid} 15 assault_mp,0"),
        ],
    );
    run_frames(
        &mut sv,
        std::slice::from_mut(&mut probe),
        RUN_MS,
        &mut Instant::now(),
        &mut |_, _, _| {},
    );
    let aborts = sv.game.level.as_ref().unwrap().rt.aborts();
    (side(&probe.lines), aborts)
}

/// Retail's jets enter with their spawn frame's four `EV_PLAY_FX_ON_TAG`s
/// already on the ring (which 1.7's `CG_ResetEntity` takes as played), the
/// late jet with none until its four come; ours send the same. The dead
/// owner's `cursorHint` reads 0 from his death's snapshot while its string
/// and entity stay until the respawn, on both.
#[test]
fn jets_tag_effects_and_the_dead_hint_match_retail() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let r = side(&retail("planefx"));
    let (o, aborts) = ours(&install, &r);
    assert!(aborts.is_empty(), "{aborts:?}");
    let fx = (0..256)
        .find(|&i| vcod_cod4::net::events::event_name(i) == Some("EV_PLAY_FX_ON_TAG"))
        .unwrap();
    let rj = jets(&r);
    assert_eq!(
        rj,
        vec![
            (4, (1..=4).map(|p| (fx, p)).collect(), None),
            (0, vec![(0, 0); 4], Some(500)),
        ],
        "retail's jets"
    );
    assert_eq!(jets(&o), rj, "ours against retail");

    // Retail: the gun's hint (its weapon plus 4, no string, the item),
    // then the death clears the hint alone, then the respawn the rest.
    let rh = hints(&r);
    let dead = |h: &[(u32, u32, u32, u32)]| {
        let k = h.iter().position(|x| x.0 == 7).expect("the owner dies");
        (h[k - 1], h[k])
    };
    let (before, at) = dead(&rh);
    assert!(before.1 > 4 && before.2 == u32::MAX, "{rh:?}");
    assert_eq!(at, (7, 0, before.2, before.3), "{rh:?}");
    let oh = hints(&o);
    let (obefore, oat) = dead(&oh);
    assert_eq!(obefore.1, before.1, "the gun's hint: {oh:?}");
    assert_eq!(oat, (7, 0, obefore.2, obefore.3), "ours: {oh:?}");
}
