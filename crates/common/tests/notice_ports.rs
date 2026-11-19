//! NOTICE's list of ported files against the sources that say they are
//! ports: every `.rs` under `crates/` whose `//!` header sends the reader to
//! NOTICE must be listed there, and every listed file must carry such a header.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        let name = path.file_name().unwrap().to_string_lossy();
        if path.is_dir() {
            // Fixtures hold probe sources and captures, not vcod code.
            if name != "target" && name != "fixtures" {
                rs_files(&path, out);
            }
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

#[test]
fn notice_lists_every_ported_file() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    rs_files(&root.join("crates"), &mut files);
    let headed: BTreeSet<String> = files
        .iter()
        .filter(|p| {
            let text = std::fs::read_to_string(p).unwrap_or_default();
            text.lines()
                .take_while(|l| l.starts_with("//!"))
                .any(|l| l.contains("See NOTICE."))
        })
        .map(|p| {
            p.strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert!(!headed.is_empty(), "no ported files found under crates/");

    let notice = std::fs::read_to_string(root.join("NOTICE")).expect("read NOTICE");
    let listed: BTreeSet<String> = notice
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("crates/") && l.ends_with(".rs"))
        .map(String::from)
        .collect();

    let unlisted: Vec<_> = headed.difference(&listed).collect();
    let unheaded: Vec<_> = listed.difference(&headed).collect();
    assert!(
        unlisted.is_empty() && unheaded.is_empty(),
        "ported files missing from NOTICE: {unlisted:?}; listed in NOTICE without the header: {unheaded:?}"
    );
}
