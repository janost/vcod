//! A script model's whole-model bounds reject, ours against retail's.
//! `client-probes/probe_modelbounds` turns `xmodel/vehicle_car` and
//! `xmodel/cow_dead2` through a few yaws and runs level `bulletTrace`s
//! across each. Retail's entity clip drops a model whose unrotated bounds
//! the segment misses before it clips a triangle, so the car's nose past
//! its 94.37 cube never stops a trace at yaw 0 or 90 but does at 45, and
//! the cow, whose bounds are all 0, never does (docs/research/cod11-combat.md
//! 2.7). The 1.1d test needs `COD_DIR` at a 1.1 install, the 1.5b and UO
//! ones at a UO install (its `main/` is 1.5's); each returns early without.
//! `MB_REPORT=1` prints both logs.

use crate::common;

use vcod_common::net::protocol::{Cod1Wire, PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22};

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_modelbounds.gsc");

fn ours(fs: vcod_common::pk3::Pk3Fs, proto: Option<&'static Cod1Wire>) -> Vec<String> {
    let (mut sv, mut now) = common::Boot {
        proto: Some(proto.unwrap_or(&PROTOCOL_V1)),
        script: Some(SCRIPT),
        ..common::Boot::new("mp_carentan", "probe_modelbounds")
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, 5000);
    sv.script_log().to_vec()
}

/// Each `btrace` line as its model, yaw, offset and height, whether it
/// stopped on the model, and its fraction.
fn traces(lines: &[String]) -> Vec<(String, bool, f32)> {
    let mut model = String::new();
    let mut out = Vec::new();
    for l in lines.iter().map(|l| l.trim()) {
        if let Some(r) = l.strip_prefix("PROBE model ") {
            model = r.split(' ').nth(1).unwrap().to_string();
        } else if let Some(r) = l.strip_prefix("PROBE btrace ") {
            let t: Vec<&str> = r.split(' ').collect();
            out.push((t[..4].join(" "), t[4] == model, t[5].parse().unwrap()));
        }
    }
    out
}

fn compare(fixture: &str, log: Vec<String>) {
    let retail: Vec<String> = fixture.lines().map(str::to_string).collect();
    if std::env::var_os("MB_REPORT").is_some() {
        for l in &log {
            eprintln!("ours   {l}");
        }
    }
    let (r, o) = (traces(&retail), traces(&log));
    assert_eq!(r.len(), 138);
    assert_eq!(o.len(), r.len(), "{log:?}");
    let bad: Vec<String> = r
        .iter()
        .zip(&o)
        .filter(|(r, o)| r.0 != o.0 || r.1 != o.1 || (r.2 - o.2).abs() > 1e-5)
        .map(|(r, o)| format!("retail {r:?}, ours {o:?}"))
        .collect();
    assert!(
        bad.is_empty(),
        "{} rows differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
fn the_model_bounds_reject_on_11() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    compare(
        include_str!("fixtures/scriptmodel/mp_carentan-scriptmodel-bounds.txt"),
        ours(fs, None),
    );
}

#[test]
fn the_model_bounds_reject_on_15() {
    let Some(fs) = vcod_common::testing::game_fs_15() else {
        return;
    };
    compare(
        include_str!("fixtures/scriptmodel/mp_carentan-scriptmodel-bounds-1.5.txt"),
        ours(fs, Some(&PROTOCOL_V6)),
    );
}

#[test]
fn the_model_bounds_reject_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    compare(
        include_str!("fixtures/scriptmodel/mp_carentan-scriptmodel-bounds-uo.txt"),
        ours(fs, Some(&PROTOCOL_V22)),
    );
}
