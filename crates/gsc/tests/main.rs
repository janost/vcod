//! The crate's integration tests as one binary: linked once, not once per file.

mod client_probes_parse;
mod cod4_waits;
mod corpus;
mod semantics_ab;

/// A test file left out of the list above would never build or run.
#[test]
fn every_test_file_is_a_module() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests");
    let main = std::fs::read_to_string(format!("{dir}/main.rs")).unwrap();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().is_some_and(|e| e == "rs") && stem != "main" {
            assert!(
                main.contains(&format!("\nmod {stem};")),
                "tests/{stem}.rs is not in main.rs"
            );
        }
    }
}
