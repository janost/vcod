//! No source reads `CARGO_MANIFEST_DIR` at compile time except through
//! `vcod_common::manifest`: a worktree seeded with another checkout's
//! `target/` keeps unit builds that name the seed's path, so tests would read
//! the other checkout's docs and fixtures.

use std::path::{Path, PathBuf};

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != "target") {
                rs_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn manifest_dir_is_read_at_run_time() {
    let root = PathBuf::from(vcod_common::manifest_dir!()).join("../..");
    let mut files = Vec::new();
    rs_files(&root.join("crates"), &mut files);
    // Built in pieces so this file does not match itself.
    let needle = ["env!(\"CARGO_MANIFEST", "_DIR\")"].concat();
    let mut bad = Vec::new();
    for path in files {
        if path.ends_with("common/src/manifest.rs") || path.ends_with("tests/manifest_dir.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        // The stripped text lets a call split across lines match; an
        // include_str!/include_bytes! path is tracked by cargo and stays.
        let flat: String = text.split_whitespace().collect();
        let mut from = 0;
        while let Some(at) = flat[from..].find(&needle) {
            let at = from + at;
            let before = &flat[..at];
            if !(before.ends_with("include_str!(concat!(")
                || before.ends_with("include_bytes!(concat!("))
            {
                bad.push(path.strip_prefix(&root).unwrap().display().to_string());
                break;
            }
            from = at + needle.len();
        }
    }
    assert!(
        bad.is_empty(),
        "use vcod_common::{{manifest_dir!, manifest_path!, manifest_const!}} (crates/common/src/manifest.rs) \
         instead of the compile-time CARGO_MANIFEST_DIR in {bad:?}; tmp/mg/rewrite-manifest-dir.py converts them"
    );
}

#[test]
fn run_time_dir_follows_the_cargo_environment() {
    let dir = vcod_common::manifest::resolve("/seed/checkout/crates/common");
    // cargo sets CARGO_MANIFEST_DIR to this crate; the sibling named by the
    // compiled-in path is taken from its parent.
    let here = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    assert_eq!(PathBuf::from(dir), here.parent().unwrap().join("common"));
}
