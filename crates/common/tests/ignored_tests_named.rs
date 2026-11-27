//! Every `#[ignore]`d test is named in `docs/dev/testing.md` and gives
//! cargo a reason: an ignored test nobody can find never runs, and a bare
//! `#[ignore]` prints no hint of what it needs.
//!
//! A test counts as named when the doc spells its name, a prefix of it in
//! backticks (`cod2_live_`), or a filter that reaches it: its file's stem
//! or its parent module followed by `::` (`bots::`, `cod4::bots::`).

use std::path::{Path, PathBuf};

/// Every `.rs` file under `crates/`, skipping build output.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy();
        if path.is_dir() {
            if name != "target" && !name.starts_with('.') {
                rust_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Each ignored test in `src`: its fn name and whether its attribute
/// carries a reason.
fn ignored(src: &str) -> Vec<(String, bool)> {
    let lines: Vec<&str> = src.lines().map(str::trim).collect();
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if !l.starts_with("#[ignore") {
            continue;
        }
        let reason = l.starts_with("#[ignore =");
        let Some(name) = lines[i + 1..]
            .iter()
            .find(|l| !l.starts_with("#[") && !l.starts_with("//"))
            .and_then(|l| l.split("fn ").nth(1))
            .map(|rest| rest.split(['(', '<']).next().unwrap().to_string())
        else {
            continue;
        };
        out.push((name, reason));
    }
    out
}

/// Whether `doc` names the test `name` in `file`.
fn named(doc: &str, file: &Path, name: &str) -> bool {
    if doc.contains(name) {
        return true;
    }
    let stem = file.file_stem().unwrap().to_string_lossy();
    let parent = file
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy();
    if doc.contains(&format!("{stem}::")) && stem != "mod" && stem != "tests" {
        return true;
    }
    if (stem == "mod" || stem == "tests") && doc.contains(&format!("{parent}::")) {
        return true;
    }
    doc.split('`')
        .skip(1)
        .step_by(2)
        .any(|code| code.len() > 4 && code.ends_with('_') && name.starts_with(code))
}

#[test]
fn every_ignored_test_is_named_in_testing_md_with_a_reason() {
    let root = Path::new(vcod_common::manifest_dir!()).join("../..");
    let doc = std::fs::read_to_string(root.join("docs/dev/testing.md")).unwrap();
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    files.sort();
    let mut seen = 0;
    let mut wrong = Vec::new();
    for file in &files {
        let src = std::fs::read_to_string(file).unwrap();
        let rel = file.strip_prefix(&root).unwrap().display().to_string();
        // This file's own sample attributes are not tests.
        if rel.ends_with("ignored_tests_named.rs") {
            continue;
        }
        for (name, reason) in ignored(&src) {
            seen += 1;
            if !reason {
                wrong.push(format!("{rel}: {name} has a bare #[ignore]"));
            }
            if !named(&doc, file, &name) {
                wrong.push(format!("{rel}: {name} is not in docs/dev/testing.md"));
            }
        }
    }
    assert!(seen > 20, "found only {seen} ignored tests");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn the_check_reads_names_reasons_and_filters() {
    let src = "#[test]\n#[ignore]\nfn bare() {}\n#[test]\n#[ignore = \"why\"]\n\
               #[cfg(unix)]\npub fn given<T>() {}\n";
    let got = ignored(src);
    assert_eq!(got, [("bare".into(), false), ("given".into(), true)]);
    let doc = "run `bots::`, `cod2_live_` and `cod4::bots::`";
    assert!(named(doc, Path::new("server/tests/bots.rs"), "a_bot"));
    assert!(named(
        doc,
        Path::new("client/play/cod2_soak.rs"),
        "cod2_live_x"
    ));
    assert!(named(doc, Path::new("server/src/cod4/bots.rs"), "join"));
    assert!(!named(doc, Path::new("server/src/game/x.rs"), "other"));
}
