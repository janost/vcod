//! Stock-pak census for the script parser. Needs `COD_DIR`; skips silently
//! otherwise, the same gate every game-data test in this workspace uses.

use vcod_common::pk3::Pk3Fs;

/// A `Pk3Fs` over symlinks to only the stock `pak<N>.pk3` archives, so a
/// third-party pak downloaded into `main/` cannot move the census numbers.
/// The tempdir rides along because `Pk3Fs` resolves entries lazily.
///
/// `None` only when there is no game install at all (`main/` missing) — the
/// same skip every game-data test in this workspace uses so CI stays green
/// without `COD_DIR`. Once `main/` is confirmed present, every further step
/// is expected to succeed: a `main/` that exists but is missing its stock
/// paks, unreadable, or otherwise broken must fail the test loudly rather
/// than silently degrade to an empty corpus that still reports green.
fn stock_paks() -> Option<(tempfile::TempDir, Pk3Fs)> {
    let main = vcod_common::testing::game_dir().join("main");
    if !main.is_dir() {
        return None;
    }
    let tmp = tempfile::tempdir().expect("create a tempdir for stock pak symlinks");
    for entry in std::fs::read_dir(&main).expect("read main/") {
        let path = entry.expect("read a main/ directory entry").path();
        let is_stock_pak = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|stem| stem.strip_prefix("pak"))
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("pk3"));
        if !is_stock_pak {
            continue;
        }
        let name = path.file_name().expect("a pak path has a file name");
        let link = tmp.path().join(name);
        std::os::unix::fs::symlink(&path, &link)
            .unwrap_or_else(|e| panic!("symlink {}: {e}", path.display()));
    }
    let fs =
        Pk3Fs::open(tmp.path()).unwrap_or_else(|e| panic!("open the stock-pak symlink dir: {e}"));
    Some((tmp, fs))
}

#[test]
fn every_stock_script_parses() {
    let Some((_tmp, fs)) = stock_paks() else {
        return;
    };
    let names = fs.names_with_suffix(".gsc");
    // Pinned to the measured corpus size (docs/research/cod11-gsc-language.md
    // §1): pak0 (17) + pak4 (162) + pak5 (620) = 799. A corrupt or partial
    // pak set changes this count and must fail loudly, not just shrink the
    // corpus the census below silently checks.
    assert_eq!(
        names.len(),
        799,
        "expected the full 799-file stock corpus, got {}",
        names.len()
    );

    let mut failures = Vec::new();
    let mut unreadable = 0usize;
    for name in &names {
        let Some(bytes) = fs.read(name) else {
            unreadable += 1;
            continue;
        };
        let src = String::from_utf8_lossy(&bytes);
        if let Err(e) = vcod_gsc::parse::parse_file(&src) {
            failures.push(format!("{name}:{}: {}", e.line, e.msg));
        }
    }
    assert_eq!(
        unreadable,
        0,
        "{unreadable} of {} stock scripts could not be read from their pak",
        names.len()
    );
    assert!(
        failures.is_empty(),
        "{} of {} scripts failed to parse:\n{}",
        failures.len(),
        names.len(),
        failures.join("\n")
    );
}
