//! Stock-pak census for the script parser. Needs `COD_DIR`; skips silently
//! otherwise, the same gate every game-data test in this workspace uses.

use vcod_common::pk3::Pk3Fs;

/// A `Pk3Fs` over symlinks to only the stock `pak<N>.pk3` archives, so a
/// third-party pak downloaded into `main/` cannot move the census numbers.
/// The tempdir rides along because `Pk3Fs` resolves entries lazily.
fn stock_paks() -> Option<(tempfile::TempDir, Pk3Fs)> {
    let main = vcod_common::testing::game_dir().join("main");
    if !main.is_dir() {
        return None;
    }
    let tmp = tempfile::tempdir().ok()?;
    for entry in std::fs::read_dir(&main).ok()? {
        let path = entry.ok()?.path();
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
        let link = tmp.path().join(path.file_name()?);
        std::os::unix::fs::symlink(&path, &link).ok()?;
    }
    let fs = Pk3Fs::open(tmp.path()).ok()?;
    Some((tmp, fs))
}

#[test]
fn every_stock_script_parses() {
    let Some((_tmp, fs)) = stock_paks() else {
        return;
    };
    let names = fs.names_with_suffix(".gsc");
    assert!(
        names.len() > 700,
        "expected the full corpus, got {}",
        names.len()
    );

    let mut failures = Vec::new();
    for name in &names {
        let Some(bytes) = fs.read(name) else { continue };
        let src = String::from_utf8_lossy(&bytes);
        if let Err(e) = vcod_gsc::parse::parse_file(&src) {
            failures.push(format!("{name}:{}: {}", e.line, e.msg));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} scripts failed to parse:\n{}",
        failures.len(),
        names.len(),
        failures.join("\n")
    );
}
