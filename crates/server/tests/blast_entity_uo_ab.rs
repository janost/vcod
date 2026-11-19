//! A `radiusDamage` blast on a United Offensive script model that
//! `setTakeDamage` opened, against retail UO 1.51b: the `"damage"` notify's
//! six arguments (`client-probes/probe_blastbase.gsc` on mp_cassino,
//! `COD_VERSION=uo tools/run_probe.sh`, 2026-10-14; docs/research/coduo-diff.md
//! 22). The charge is 9 on both, not the 97 the distance gives: the model
//! sits on the floor, and 97 times `CanDamage`'s 0.1 is the likely reading
//! (not traced).
//!
//! Needs `COD_DIR` at a UO install; without one it returns early.

use crate::common;

/// What retail's `games_mp.log` held.
const RETAIL: &[&str] = &[
    "PROBE dmg 9 MOD_EXPLOSIVE (0.00, 0.00, 1.00) (100.00, 200.00, 10.00)",
    "PROBE end",
];

#[test]
fn a_blast_notifies_a_script_model_as_retail_uo_does() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let fs = std::rc::Rc::new(fs);
    let src = std::fs::read_to_string(
        "../gsc/tests/fixtures/semantics/client-probes/probe_blastbase.gsc",
    )
    .expect("the probe");
    let (mut sv, mut now) = common::Boot {
        proto: Some(&vcod_common::net::protocol::PROTOCOL_V22),
        script: Some(&src),
        ..common::Boot::new("mp_cassino", "probe_blastbase")
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, 3000);
    let ours: Vec<&str> = sv
        .script_log()
        .iter()
        .map(|l| l.trim())
        .filter(|l| l.starts_with("PROBE"))
        .collect();
    assert_eq!(ours, RETAIL);
}
