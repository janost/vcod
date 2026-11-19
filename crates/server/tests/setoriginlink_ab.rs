//! Which origin a `setOrigin` links a player at, against retail 1.1d
//! (`client-probes/probe_setoriginlink.gsc` on mp_carentan,
//! `tools/run_probe.sh`; capture in `fixtures/setoriginlink/`, recipe in
//! the client probes' README): `radiusDamage`'s second chance reads the
//! midpoint of the linked box, swept by radius in the frame of the
//! `setOrigin` and after his cmds (cod11-combat.md 14.3).
//!
//! Needs `COD_DIR` at a 1.1 install; otherwise it returns early.

use crate::common;

/// The `PROBE` lines of a log a replay can repeat: the `cmd` case's fall
/// and its sweep ride the client's own cmd clock, so only its verdict is
/// compared.
fn probe_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    lines
        .filter(|l| l.starts_with("PROBE"))
        .filter(|l| !l.starts_with("PROBE fell ") && !l.starts_with("PROBE edge cmd "))
        .collect()
}

/// Runs the probe on ours with one axis `--probe-uo-weapon direct` client
/// until it logs `PROBE done`; its `PROBE` lines.
fn ours(fs: vcod_common::pk3::Pk3Fs) -> Vec<String> {
    let (map, probe) = ("mp_carentan", "probe_setoriginlink");
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

/// Needs `COD_DIR` at a 1.1 install.
#[test]
fn set_origin_links_the_player_where_retail_does() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let ours = ours(fs);
    if std::env::var_os("SETORIGINLINK_REPORT").is_some() {
        for l in &ours {
            eprintln!("{l}");
        }
    }
    let retail = std::fs::read_to_string("tests/fixtures/setoriginlink/mp_carentan-1.1.txt")
        .expect("the capture");
    assert_eq!(
        probe_lines(ours.iter().map(String::as_str)),
        probe_lines(retail.lines())
    );
}
