//! A player that `playSound` touched rides every client's snapshot past the
//! PVS cull, the S&D planter's case (`SVF_BROADCAST`, game.mp 0x5d957),
//! against the retail run of `client-probes/probe_playersound` on
//! mp_carentan (docs/research/cod11-sound-system.md, section 9): out of the
//! viewer's PVS, the sounder entered its snapshots on the sound's frame,
//! stayed through a move, left on its death's frame and came back on its
//! respawn's with no new sound, left as a spectator, came back as a player,
//! and was gone for good from the `map_restart(true)`.
//!
//! Needs `COD_DIR`; without the paks it returns early.

use crate::common;
use vcod_common::net::Msec;

use common::{ClientEnd, Queues};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use vcod_common::net::NetClient;
use vcod_common::net::msg::NULL_USERCMD;

const MAP: &str = "mp_carentan";
const PROBE_SRC: &str = "../gsc/tests/fixtures/semantics/client-probes/probe_playersound.gsc";

#[test]
fn a_sounded_player_rides_every_snapshot_until_the_restart() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let probe = std::fs::read_to_string(PROBE_SRC).expect("read the probe");
    let (mut sv, mut now) = common::Boot {
        script: Some(&probe),
        ..common::Boot::new(MAP, "probe_playersound")
    }
    .start(fs);

    let (qa, qb) = (
        Rc::new(RefCell::new(Queues::default())),
        Rc::new(RefCell::new(Queues::default())),
    );
    let mut viewer = NetClient::start_with_qport(ClientEnd(qa.clone()), now, 0x2001);
    let mut sounder = NetClient::start_with_qport(ClientEnd(qb.clone()), now, 0x2002);
    // (serverTime, entity 1 in the viewer's snapshot) at every change.
    let mut changes: Vec<(i32, bool)> = Vec::new();
    let mut seen = i32::MIN;
    // The script log starts over at the restart.
    let mut log: Vec<String> = Vec::new();
    for _ in 0..1400 {
        now += Duration::from_millis(50);
        viewer.send_frame(&NULL_USERCMD);
        sounder.send_frame(&NULL_USERCMD);
        common::step_pair(&mut sv, (&qa, &mut viewer), (&qb, &mut sounder), now);
        if let Some(s) = viewer.snapshots().newest()
            && s.server_time != Msec::from_raw(seen)
        {
            seen = s.server_time.raw();
            let rides = s.entities.contains_key(&1);
            if changes.last().is_none_or(|&(_, r)| r != rides) {
                changes.push((s.server_time.raw(), rides));
            }
        }
        for l in sv.script_log() {
            if !log.contains(l) {
                log.push(l.clone());
            }
        }
        if log.iter().any(|l| l.starts_with("PROBE step done")) {
            break;
        }
    }
    let step = |name: &str| -> i32 {
        let prefix = format!("PROBE step {name} ");
        log.iter()
            .find_map(|l| l.strip_prefix(&prefix))
            .unwrap_or_else(|| panic!("no step {name}: {log:?}"))
            .trim()
            .parse()
            .unwrap()
    };
    let restart = step("restart");
    let after = changes
        .iter()
        .find(|&&(t, _)| t > restart)
        .copied()
        .expect("a change after the restart");
    assert_eq!(
        changes,
        vec![
            (changes[0].0, false),
            (step("sound"), true),
            (step("die"), false),
            (step("respawn"), true),
            (step("spectator"), false),
            (step("playing"), true),
            (after.0, false),
        ],
        "{log:?}"
    );
}
