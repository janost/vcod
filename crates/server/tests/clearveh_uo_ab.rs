//! `clearVehiclePosition` on a dropped weapon in a United Offensive tank's
//! box against retail UO 1.51b (`client-probes/probe_clearveh.gsc` on
//! mp_cassino, `COD_VERSION=uo tools/run_probe.sh`, 2026-11-14;
//! docs/research/coduo-vehicles.md 16): the item's `death` notify with itself
//! as the argument after the calling thread's step, its `endon` fired, the
//! item still defined 50 ms on, and the one outside the box left alone.
//! Where each `dropItem` lands is a `rand()` draw the capture did not log
//! (cod11-items.md 14.2), so the item lines are compared without it; the
//! retail toss put the near one 129 units ahead of the tank's origin, past
//! the file's 70-unit box and inside `VEH_GetMinsMaxs`'s padded one.
//!
//! Needs `COD_DIR` at a UO install; without one it returns early.

use crate::common;

/// What retail's `games_mp.log` held.
const RETAIL: &[&str] = &[
    "PROBE vehicle 186 panzeriv_mp (4676.00, -1979.00, -7.88)",
    "PROBE item near 157 (4805.10, -1979.00, -6.04)",
    "PROBE item far 158 (5327.09, -1979.00, -6.19)",
    "PROBE call 7550",
    "PROBE after 7550 1 1",
    "PROBE death near 7550 157 157",
    "PROBE ended near 7550",
    "PROBE after 7600 1 1",
    "PROBE spawn 7600 159",
    "PROBE spawn 7650 160",
    "PROBE spawn 7700 156",
    "PROBE spawn 7750 162",
    "PROBE done 7800",
];

#[test]
fn a_vehicle_clears_a_dropped_weapon_as_retail_uo_does() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let fs = std::rc::Rc::new(fs);
    let src =
        std::fs::read_to_string("../gsc/tests/fixtures/semantics/client-probes/probe_clearveh.gsc")
            .expect("the probe");
    let (mut sv, mut now) = common::Boot {
        proto: Some(&vcod_common::net::protocol::PROTOCOL_V22),
        script: Some(&src),
        ..common::Boot::new("mp_cassino", "probe_clearveh")
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, 10_000);
    // `PROBE item <tag> <number>`, the drop's landing cut.
    let unplaced = |l: &str| -> String {
        if l.starts_with("PROBE item ") {
            l.split(" (").next().unwrap_or(l).to_string()
        } else {
            l.to_string()
        }
    };
    let ours: Vec<String> = sv
        .script_log()
        .iter()
        .map(|l| l.trim())
        .filter(|l| l.starts_with("PROBE"))
        .map(unplaced)
        .collect();
    let retail: Vec<String> = RETAIL.iter().map(|l| unplaced(l)).collect();
    assert_eq!(ours, retail);
}
