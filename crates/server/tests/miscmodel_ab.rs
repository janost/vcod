//! Static models (`misc_model`) in a `bulletTrace`'s way, ours against
//! retail's. `client-probes/probe_miscmodel` traces segments that run into
//! one placement where a surface-by-surface clip, each surface culled by
//! its grown stored bounds short of the best fraction so far, meets another
//! point than a clip of every triangle at once (combat doc 2.7.2). 1.1d,
//! 1.5b and UO log the same rows; each test needs `COD_DIR` at its install
//! and returns early without. `MM_REPORT=1` prints both logs.

use crate::common;

use vcod_common::net::protocol::{Cod1Wire, PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22};

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_miscmodel.gsc");

/// Each `mtrace` line's segment, entity, fraction, then normal and position.
fn traces(text: impl IntoIterator<Item = String>) -> Vec<(String, String, f32, [f32; 6])> {
    let nums = |s: &str| -> Vec<f32> {
        s.split(|c: char| "(), ".contains(c))
            .filter(|t| !t.is_empty())
            .map(|t| t.parse().unwrap())
            .collect()
    };
    text.into_iter()
        .filter_map(|l| l.trim().strip_prefix("PROBE mtrace ").map(str::to_string))
        .map(|l| {
            let t: Vec<&str> = l.splitn(5, ' ').collect();
            let v = nums(t[4]);
            (
                t[1].to_string(),
                t[2].to_string(),
                t[3].parse().unwrap(),
                std::array::from_fn(|i| v[i]),
            )
        })
        .collect()
}

fn gate(fs: vcod_common::pk3::Pk3Fs, proto: &'static Cod1Wire, map: &str, fixture: &str) {
    let (mut sv, mut now) = common::Boot {
        proto: Some(proto),
        script: Some(SCRIPT),
        ..common::Boot::new(map, "probe_miscmodel")
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, 1500);
    let ours = sv.script_log().to_vec();
    if std::env::var_os("MM_REPORT").is_some() {
        ours.iter().for_each(|l| eprintln!("ours   {l}"));
        fixture.lines().for_each(|l| eprintln!("retail {l}"));
    }
    let (r, o) = (traces(fixture.lines().map(str::to_string)), traces(ours));
    assert!(r.len() >= 3, "{r:?}");
    assert_eq!(o.len(), r.len(), "{o:?}");
    for (r, o) in r.iter().zip(&o) {
        assert_eq!((&o.0, &o.1), (&r.0, &r.1), "retail {r:?}, ours {o:?}");
        assert!((o.2 - r.2).abs() < 2e-5, "retail {r:?}, ours {o:?}");
        assert!(
            o.3.iter().zip(&r.3).all(|(a, b)| (a - b).abs() < 0.015),
            "retail {r:?}, ours {o:?}"
        );
    }
}

const DEPOT: &str = include_str!("fixtures/miscmodel/mp_depot-miscmodel.txt");
const CHATEAU: &str = include_str!("fixtures/miscmodel/mp_chateau-miscmodel.txt");

/// mp_depot's boxcars, flatcars and cage lights and mp_chateau's truck on
/// 1.1d.
#[test]
fn a_static_model_clips_surface_by_surface() {
    for (map, fixture) in [("mp_depot", DEPOT), ("mp_chateau", CHATEAU)] {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            return;
        };
        gate(fs, &PROTOCOL_V1, map, fixture);
    }
}

/// The same on 1.5b.
#[test]
fn a_static_model_clips_surface_by_surface_on_15() {
    for (map, fixture) in [("mp_depot", DEPOT), ("mp_chateau", CHATEAU)] {
        let Some(fs) = vcod_common::testing::game_fs_15() else {
            return;
        };
        gate(fs, &PROTOCOL_V6, map, fixture);
    }
}

/// The same on UO.
#[test]
fn a_static_model_clips_surface_by_surface_on_uo() {
    for (map, fixture) in [("mp_depot", DEPOT), ("mp_chateau", CHATEAU)] {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        gate(fs, &PROTOCOL_V22, map, fixture);
    }
}
