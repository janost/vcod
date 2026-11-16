//! `func_door`, `func_door_rotating` and `func_rotating` against retail:
//! mp_carentan with the stock blocks of six submodels replaced by
//! `fixtures/doors/mp_carentan-doors.ent`, under
//! `client-probes/probe_doors` with one `--probe-uo-weapon direct` client,
//! on 1.1d, 1.5b and UO (`fixtures/doors/mp_carentan-dm-doors*.txt`,
//! recipes in their headers). Every frame's door origins and angles, the
//! client's origin, the use key's `"trigger"`, the crush damage and the
//! rotating slave's turn must be retail's (cod11-movers.md 26).
//!
//! Needs `COD_DIR` at the capture's install; without it the test returns
//! early.

use crate::common;
use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22, Protocol};
use vcod_common::pk3::Pk3Fs;

const MAP: &str = "mp_carentan";
const BLOCKS: &str = include_str!("fixtures/doors/mp_carentan-doors.ent");

/// `fs` with the doors lump in place of mp_carentan's.
fn with_doors(fs: Option<Pk3Fs>) -> Option<Pk3Fs> {
    let mut fs = fs?;
    let path = fs.resolve_map(MAP)?;
    let bytes = fs.read(&path)?;
    fs.overlay(
        &path,
        vcod_common::testing::bsp_replacing_models(&bytes, BLOCKS),
    );
    Some(fs)
}

fn fs_11() -> Option<Pk3Fs> {
    with_doors(vcod_common::testing::game_fs_11())
}

fn fs_15() -> Option<Pk3Fs> {
    let fs = vcod_common::testing::game_fs()?;
    let v6 = PROTOCOL_V6.transport.version;
    let fs = (vcod_server::server::install_protocol(&fs).transport.version == v6).then_some(fs);
    with_doors(fs)
}

fn fs_uo() -> Option<Pk3Fs> {
    with_doors(vcod_common::testing::uo_fs())
}

/// The lines up to `PROBE done`: the frame logger runs on after it, on
/// retail until the server quits, on ours until the replay stops.
fn to_done(mut lines: Vec<String>) -> Vec<String> {
    if let Some(i) = lines.iter().position(|l| l == "PROBE done") {
        lines.truncate(i + 1);
    }
    lines
}

fn check(fs: fn() -> Option<Pk3Fs>, proto: &'static Protocol, fixture: &str) {
    if fs().is_none() {
        eprintln!("COD_DIR unset or not the capture's install: skipping");
        return;
    }
    let text = std::fs::read_to_string(fixture).unwrap();
    let retail = to_done(
        text.lines()
            .filter(|l| l.starts_with("PROBE"))
            .map(str::to_string)
            .collect(),
    );
    let ours = to_done(common::direct_replay(
        fs,
        proto,
        MAP,
        "probe_doors",
        "kar98k_mp",
        &text,
    ));
    if std::env::var("DOORS_REPORT").is_ok_and(|v| v == "1") {
        for (i, (a, b)) in retail.iter().zip(&ours).enumerate() {
            if a != b {
                eprintln!("{i}\n retail {a}\n ours   {b}");
            }
        }
    }
    let first = retail.iter().zip(&ours).position(|(a, b)| a != b);
    assert!(
        first.is_none() && retail.len() == ours.len(),
        "{fixture}: first difference at line {first:?} of {} / {}: retail {:?} ours {:?}",
        retail.len(),
        ours.len(),
        first.map(|i| &retail[i]),
        first.map(|i| &ours[i]),
    );
}

#[test]
fn doors_run_as_on_retail_11() {
    check(
        fs_11,
        &PROTOCOL_V1,
        "tests/fixtures/doors/mp_carentan-dm-doors.txt",
    );
}

#[test]
fn doors_run_as_on_retail_15() {
    check(
        fs_15,
        &PROTOCOL_V6,
        "tests/fixtures/doors/mp_carentan-dm-doors-1.5.txt",
    );
}

#[test]
fn doors_run_as_on_retail_uo() {
    check(
        fs_uo,
        &PROTOCOL_V22,
        "tests/fixtures/doors/mp_carentan-dm-doors-uo.txt",
    );
}
