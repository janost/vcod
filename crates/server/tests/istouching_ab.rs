//! `isTouching` against a standing, crouched, prone and dead player,
//! against retail 1.1d, 1.5b and UO 1.51b (`client-probes/
//! probe_istouching.gsc` on mp_carentan, `tools/run_probe.sh`,
//! 2026-10-21; captures in `fixtures/istouching/`, recipe in the client
//! probes' README). The partner is the map's trigger_hurt brush, swept
//! past the player's box face by face (cod11-gsc-object-model.md 22.3).
//!
//! Each test needs `COD_DIR` at its install; otherwise it returns early.

use crate::common;

/// The `PROBE` lines of a log.
fn probe_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    lines.filter(|l| l.starts_with("PROBE")).collect()
}

/// Runs the probe on ours with one axis `--probe-uo-weapon direct` client
/// until it logs `PROBE done`; its `PROBE` lines.
fn ours(fs: vcod_common::pk3::Pk3Fs) -> Vec<String> {
    let (map, probe) = ("mp_carentan", "probe_istouching");
    let src = common::client_probe(probe);
    let (mut sv, mut now) = common::Boot {
        script: Some(&src),
        ..common::Boot::new(map, probe)
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, 3000);
    let hold = common::ProbeHold {
        aim: true,
        ..Default::default()
    };
    let join = common::Join::new("axis", "kar98k_mp");
    common::run_probe(
        &mut sv,
        &mut now,
        join,
        hold,
        3000,
        &mut common::DirectInput::default(),
    )
}

fn check(fs: Option<vcod_common::pk3::Pk3Fs>, fixture: &str) {
    let Some(fs) = fs else {
        return;
    };
    let ours = ours(fs);
    if std::env::var_os("ISTOUCHING_REPORT").is_some() {
        for l in &ours {
            eprintln!("{l}");
        }
    }
    let retail = std::fs::read_to_string(format!("tests/fixtures/istouching/{fixture}"))
        .expect("the capture");
    assert_eq!(
        probe_lines(ours.iter().map(String::as_str)),
        probe_lines(retail.lines())
    );
}

/// Needs `COD_DIR` at a 1.1 install.
#[test]
fn is_touching_follows_the_stance_box_like_retail_1_1() {
    check(vcod_common::testing::game_fs_11(), "mp_carentan-1.1.txt");
}

/// Needs `COD_DIR` at a 1.5 install.
#[test]
fn is_touching_follows_the_stance_box_like_retail_1_5() {
    check(vcod_common::testing::game_fs_15(), "mp_carentan-1.5.txt");
}

/// Needs `COD_DIR` at a UO install.
#[test]
fn is_touching_follows_the_stance_box_like_retail_uo() {
    check(vcod_common::testing::uo_fs(), "mp_carentan-uo.txt");
}
