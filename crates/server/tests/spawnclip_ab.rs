//! A script-spawned brush entity's clip against the retail capture
//! `movers/mp_carentan-dm-spawnclip.txt` (its header carries the recipe):
//! `spawn` leaves `s.index` 0, so the entity's clip is a copy of the world's
//! where it stands (docs/research/cod11-movers.md 19). Ours runs the same
//! probe with no client and compares every `PROBE` line.
//!
//! Needs `COD_DIR`; without the paks the test returns early.

use crate::common;

use std::time::Duration;

const MAP: &str = "mp_carentan";
const FIXTURE: &str = "tests/fixtures/movers/mp_carentan-dm-spawnclip.txt";
const PROBE: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_spawnclip.gsc");

fn probe_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<String> {
    lines
        .filter_map(|l| l.find("PROBE ").map(|i| l[i + 6..].trim_end().to_string()))
        .collect()
}

#[test]
fn a_spawned_brush_entity_clips_as_the_world_where_it_stands() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let (mut sv, mut now) = common::Boot {
        script: Some(PROBE),
        ..common::Boot::new(MAP, "probe_spawnclip")
    }
    .start(fs);
    let mut log = Vec::new();
    for _ in 0..100 {
        now += Duration::from_millis(common::FRAME_MS as u64);
        sv.tick(now);
        sv.take_outgoing();
        log = sv.script_log().to_vec();
        if sv.is_killed() {
            break;
        }
    }
    assert!(sv.is_killed(), "the bobbing drops the level");
    let ours = probe_lines(log.iter().map(String::as_str));
    let text = std::fs::read_to_string(FIXTURE).unwrap();
    let retail: Vec<String> = probe_lines(text.lines())
        .into_iter()
        .filter(|l| !l.starts_with("PROBE_ERROR"))
        .collect();
    assert_eq!(ours, retail);
}
