//! An int spawn key whose value spells no number, against retail UO
//! 1.51b: mp_foy's base walls carry `"script_idnumber" "foo"`, which reads
//! 0 (`client-probes/probe_spawnint.gsc`, `COD_VERSION=uo
//! tools/run_probe.sh client-probes/probe_spawnint mp_foy`, 2026-10-15;
//! docs/research/bot-objectives.md 7). `bas.gsc` deletes a breached base's
//! walls only on `isdefined` of it.
//!
//! Needs `COD_DIR` at a UO install; without one it returns early.

use crate::common;

/// What retail's `games_mp.log` held.
const RETAIL: &[&str] = &[
    "PROBE count 15",
    "PROBE ent mp_gmi_bas_axis_spawn idnumber 0",
    "PROBE ent mp_gmi_bas_axis_spawn idnumber 0",
    "PROBE ent mp_gmi_bas_axis_spawn idnumber 0",
    "PROBE ent mp_gmi_bas_axis_spawn idnumber 0",
    "PROBE ent script_brushmodel idnumber undefined",
    "PROBE ent trigger_lookat idnumber undefined",
    "PROBE ent mp_gmi_bas_axis_spawn idnumber 0",
    "PROBE ent script_model idnumber undefined",
    "PROBE ent mp_gmi_bas_axis_spawn idnumber 0",
    "PROBE ent mp_gmi_bas_axis_spawn idnumber 0",
    "PROBE ent script_model idnumber undefined",
    "PROBE ent script_model idnumber 0",
    "PROBE ent script_model idnumber 0",
    "PROBE ent script_brushmodel idnumber undefined",
    "PROBE ent script_brushmodel idnumber 0",
];

#[test]
fn a_spawn_key_that_spells_no_number_reads_zero_as_retail_uo_does() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let fs = std::rc::Rc::new(fs);
    let src =
        std::fs::read_to_string("../gsc/tests/fixtures/semantics/client-probes/probe_spawnint.gsc")
            .expect("the probe");
    let (mut sv, mut now) = common::Boot {
        proto: Some(&vcod_common::net::protocol::PROTOCOL_V22),
        script: Some(&src),
        ..common::Boot::new("mp_foy", "probe_spawnint")
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, 1000);
    let ours: Vec<&str> = sv
        .script_log()
        .iter()
        .map(|l| l.trim())
        .filter(|l| l.starts_with("PROBE"))
        .collect();
    assert_eq!(ours, RETAIL);
}
