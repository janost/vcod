//! The United Offensive vehicle script surface no stock MP script uses
//! against retail UO 1.51b (`client-probes/probe_vehscript.gsc` on
//! mp_kharkov, `COD_VERSION=uo tools/run_probe.sh`, 2026-11-17;
//! docs/research/coduo-vehicles.md 17): `closespeed`, `harc` and `speed`
//! read and written, every wheel's `getWheelSurface`, the path speed
//! setters, `freeVehicle`'s corpse, and the error each path method and
//! wheel name raises. Retail's builtin errors throw past the module's
//! frames and abort the process before the console prints them, so the
//! messages were read under gdb at `Scr_Error` and `Scr_ParamError`.
//!
//! Needs `COD_DIR` at a UO install; without one it returns early.

use crate::common;

const READ: &str = include_str!("fixtures/vehicles/mp_kharkov-vehscript-read-uo.txt");
const FREE: &str = include_str!("fixtures/vehicles/mp_kharkov-vehscript-free-uo.txt");

/// Each erroring case: its last line before the error, and the message
/// retail's `Scr_Error` or `Scr_ParamError` took. `None` where the error
/// is the engine's own argument read (a non-entity node, a missing
/// argument), which no module breakpoint sees.
const FATAL: &[(&str, Option<&str>)] = &[
    ("attachpath_ent", Some("Not a vehicle node")),
    ("attachpath_int", None),
    (
        "startpath",
        Some("Can't start path on a vehicle that hasn't been attached"),
    ),
    ("setswitchnode", Some("Not a vehicle node")),
    ("setwaitnode", Some("Not a vehicle node")),
    ("setspeed_neg", Some("Cannot set negative speed on vehicle")),
    (
        "setspeed_accel",
        Some("Cannot set negative acceleration on vehicle"),
    ),
    ("setspeed_one", None),
    (
        "setwaitspeed_neg",
        Some("Cannot have a negative wait speed on a vehicle"),
    ),
    (
        "resumespeed_neg",
        Some("Cannot set negative acceleration on vehicle"),
    ),
    (
        "wheel_bad",
        Some(
            "Valid wheel names are: [front_left, front_right, back_left, back_right, middle_left, middle_right]",
        ),
    ),
    ("wheel_middle", Some("Vehicle has no middle wheels")),
    (
        "wheel_int",
        Some(
            "Valid wheel names are: [front_left, front_right, back_left, back_right, middle_left, middle_right]",
        ),
    ),
];

/// The probe on ours: its `PROBE` lines and the threads' errors.
fn ours(case: &str, ms: i64) -> Option<(Vec<String>, Vec<String>)> {
    let fs = vcod_common::testing::uo_fs()?;
    let fs = std::rc::Rc::new(fs);
    let src = std::fs::read_to_string(
        "../gsc/tests/fixtures/semantics/client-probes/probe_vehscript.gsc",
    )
    .expect("the probe");
    let (mut sv, mut now) = common::Boot {
        proto: Some(&vcod_common::net::protocol::PROTOCOL_V22),
        cvars: &[("developer", "1"), ("probe_case", case)],
        script: Some(&src),
        ..common::Boot::new("mp_kharkov", "probe_vehscript")
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, ms);
    let lines = sv
        .script_log()
        .iter()
        .map(|l| l.trim().to_string())
        .filter(|l| l.starts_with("PROBE"))
        .collect();
    Some((lines, sv.script_aborts()))
}

fn retail(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.starts_with("PROBE"))
        .map(str::to_string)
        .collect()
}

#[test]
fn the_fields_wheels_and_speed_setters_read_as_retail_uo_s() {
    let Some((lines, aborts)) = ours("read", 3_000) else {
        return;
    };
    assert_eq!(aborts, Vec::<String>::new());
    assert_eq!(lines, retail(READ));
}

#[test]
fn free_vehicle_leaves_a_corpse_as_retail_uo_does() {
    let Some((lines, aborts)) = ours("free", 3_000) else {
        return;
    };
    assert_eq!(aborts, Vec::<String>::new());
    assert_eq!(lines, retail(FREE));
}

#[test]
fn the_path_methods_and_wheel_names_raise_retail_uo_s_errors() {
    for (case, message) in FATAL {
        let Some((lines, aborts)) = ours(case, 2_000) else {
            return;
        };
        assert_eq!(lines, vec![format!("PROBE at {case}")], "{case}");
        assert_eq!(aborts.len(), 1, "{case}: {aborts:?}");
        if let Some(m) = message {
            assert!(aborts[0].contains(m), "{case}: {}", aborts[0]);
        }
    }
}

#[test]
fn a_freed_vehicle_s_handle_is_dead() {
    let Some((lines, aborts)) = ours("freevehicle_twice", 2_000) else {
        return;
    };
    assert_eq!(
        lines,
        vec![
            "PROBE at freevehicle_twice".to_string(),
            "PROBE after first".to_string()
        ]
    );
    assert_eq!(aborts.len(), 1, "{aborts:?}");
    assert!(
        aborts[0].contains("dead entity is not an entity"),
        "{}",
        aborts[0]
    );
}
