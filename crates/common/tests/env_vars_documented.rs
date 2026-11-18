//! Every `VCOD_*` and `VC_*` name the code spells in quotes is in
//! `docs/dev/running.md`'s env var table: a knob nobody can find is a knob
//! nobody uses, or one somebody sets by accident.

use std::path::Path;

/// The quoted `VCOD_*` / `VC_*` names in the `.rs` files under `dir`.
fn quoted_names(dir: &Path, out: &mut std::collections::BTreeSet<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            quoted_names(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            for (i, _) in text.match_indices("\"VC") {
                let rest = &text[i + 1..];
                let end = rest
                    .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
                    .unwrap_or(rest.len());
                let name = &rest[..end];
                // A bare prefix (a `starts_with` test) names no variable.
                let is_env = ["VCOD_", "VC_"]
                    .iter()
                    .any(|p| name.len() > p.len() && name.starts_with(p));
                if is_env && rest[end..].starts_with('"') {
                    out.insert(name.to_string());
                }
            }
        }
    }
}

#[test]
fn every_env_var_is_in_running_md() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut names = std::collections::BTreeSet::new();
    quoted_names(&root.join("crates"), &mut names);
    assert!(names.contains("VCOD_NETSIM"), "the scan found {names:?}");
    let doc = std::fs::read_to_string(root.join("docs/dev/running.md")).unwrap();
    let missing: Vec<&String> = names
        .iter()
        .filter(|n| !doc.contains(&format!("`{n}`")) && !doc.contains(&format!("`{n}=")))
        .collect();
    assert!(
        missing.is_empty(),
        "add these to docs/dev/running.md's env var table: {missing:?}"
    );
}
