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
#[cfg(unix)]
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

/// No unix-style symlinks off Windows; every census in this file goes
/// through this gate, so without it the whole test crate fails to *compile*
/// on Windows (not just "the census can't run"), even though `nightly.yml`
/// ships a Windows binary. Skips there, the same as an absent `COD_DIR`.
#[cfg(not(unix))]
fn stock_paks() -> Option<(tempfile::TempDir, Pk3Fs)> {
    None
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

/// The strongest check this crate has: not just "parses," but "compiles to
/// bytecode `stack_depth` accepts." `stack_depth` is an abstract-interpretation
/// walk that catches stack-discipline and out-of-range-jump defects a parse
/// alone cannot see, and until this test existed it had only ever run
/// against hand-written snippets in bytecode.rs's own unit tests, never real
/// scripts.
#[test]
fn every_stock_script_compiles() {
    let Some((_tmp, fs)) = stock_paks() else {
        return;
    };
    let names = fs.names_with_suffix(".gsc");
    let mut interner = vcod_gsc::Interner::default();
    let mut failures = Vec::new();
    let mut functions = 0usize;
    let mut unreadable = 0usize;

    for name in &names {
        let Some(bytes) = fs.read(name) else {
            unreadable += 1;
            continue;
        };
        let src = String::from_utf8_lossy(&bytes);
        let ast = match vcod_gsc::parse::parse_file(&src) {
            Ok(a) => a,
            Err(e) => {
                failures.push(format!("{name}:{}: parse: {}", e.line, e.msg));
                continue;
            }
        };
        let path = vcod_gsc::canonical(name);
        match vcod_gsc::compile::compile_file(&ast, &path, &mut interner) {
            Ok(fns) => {
                for f in &fns {
                    if let Err(e) = vcod_gsc::bytecode::stack_depth(f) {
                        failures.push(format!(
                            "{name}: stack_depth: {}::{}: {e}",
                            interner.resolve(f.file),
                            interner.resolve(f.name)
                        ));
                    }
                }
                functions += fns.len();
            }
            Err(e) => failures.push(format!("{name}:{}: compile: {}", e.line, e.msg)),
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
        "{} of {} scripts failed:\n{}",
        failures.len(),
        names.len(),
        failures.join("\n")
    );
    // Measured (docs/research/cod11-gsc-language.md §1): 799 files compile
    // to 4834 functions. Margin of a few hundred either way absorbs a stock
    // pak revision; a real change to the compiler's function count (e.g. a
    // desugar that starts or stops synthesizing helper functions) should
    // move this deliberately, not silently.
    assert!(
        (4700..5000).contains(&functions),
        "function count moved to {functions}, expected roughly 4834"
    );
}

/// The builtin surface the stock corpus actually calls, as a tripwire: if a
/// grammar change starts mis-classifying script calls as builtins, this
/// number moves. It is not a target, just a pinned observation.
#[test]
fn the_corpus_builtin_surface_is_stable() {
    let Some((_tmp, fs)) = stock_paks() else {
        return;
    };
    let mut interner = vcod_gsc::Interner::default();
    let mut names = std::collections::BTreeSet::new();
    for path in fs.names_with_suffix(".gsc") {
        let Some(bytes) = fs.read(&path) else {
            continue;
        };
        let src = String::from_utf8_lossy(&bytes);
        let Ok(ast) = vcod_gsc::parse::parse_file(&src) else {
            continue;
        };
        let canon = vcod_gsc::canonical(&path);
        let Ok(fns) = vcod_gsc::compile::compile_file(&ast, &canon, &mut interner) else {
            continue;
        };
        for f in &fns {
            for op in &f.code {
                if let vcod_gsc::bytecode::Op::CallBuiltin { name, .. } = op {
                    names.insert(interner.resolve(*name).to_string());
                }
            }
        }
    }
    // Measured (docs/research/cod11-gsc-language.md §1): 339 distinct names.
    // The engine's own name table holds 144 (§7); the corpus surface is
    // larger because this per-file pass compiles each file in isolation
    // with no cross-file resolution, so every unqualified call to a
    // function this file does not itself define — a genuine call to
    // another script's function, not just a real engine builtin — compiles
    // to `CallBuiltin` and is counted here. A grammar change to call
    // classification, or a change to what this scan resolves before
    // counting, is what should move this number.
    assert!(
        (300..400).contains(&names.len()),
        "builtin surface moved to {}: {:?}",
        names.len(),
        names
    );
}
