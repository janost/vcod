//! Stock-pak census for the parsed shader library. Needs `COD_DIR`; skips
//! silently otherwise (same gate as every game-data test).

use std::collections::BTreeSet;
use vcod_common::{
    assets, bsp,
    pk3::Pk3Fs,
    shader::{ImageRef, ShaderLib},
};

/// A `Pk3Fs` over symlinks to only the stock `pak<N>.pk3` archives, so
/// third-party paks downloaded into main/ cannot move the census numbers.
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
fn stock_shader_corpus_census() {
    let Some((_tmp, fs)) = stock_paks() else {
        return;
    };
    // parses every .shader entry in the stock paks without panicking
    let lib = ShaderLib::load(&fs);
    // Measured 604 distinct authored blocks across the Deluxe 1.5 stock paks
    // (all of them in pak4/5/8/9); the plan's ">1200" estimate matches no
    // counting of main/ - even every pak including third-party ones totals
    // 1150 raw blocks. Floor sits well under the measurement so pak churn
    // cannot trip it while still catching parsers that silently drop blocks.
    assert!(
        lib.len() > 500,
        "only {} distinct authored blocks in stock paks",
        lib.len()
    );

    let maps = fs.find_maps();
    assert!(!maps.is_empty(), "no maps found in stock paks");

    let mut reference_sum = 0usize;
    let mut skyless = Vec::new();
    for map in &maps {
        let entry = fs.resolve_map(map).unwrap();
        let b = bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let mats: BTreeSet<String> = b
            .soups
            .iter()
            .map(|s| {
                b.materials[s.material as usize]
                    .name
                    .to_lowercase()
                    .replace('\\', "/")
            })
            .collect();
        if map.starts_with("mp_") && !lib.sky_blocks().any(|s| mats.contains(s.name.as_str())) {
            skyless.push(map.clone());
        }
        // references, not distinct names: shared terrain shaders count once
        // per map that draws them (measured 302 over this install's maps)
        reference_sum += mats.iter().filter(|n| lib.get(n).is_some()).count();
    }
    assert!(
        skyless.is_empty(),
        "MP maps with no sky-parms material among their soup materials: {skyless:?}"
    );
    assert!(
        (250..350).contains(&reference_sum),
        "{reference_sum} world-referenced shader blocks, expected the 250..350 band"
    );
}

/// Stage image paths allowed to miss every stock pak.
/// - `$dlight` is CoD's engine-generated dlight blob (perlight stages, e.g.
///   pak9 window.shader:29), never a file.
/// - `textures/battleship/deckflag_np.tga` (mp_ship flagfore/flagaft second
///   bundle, pak4 jeff.shader:603) ships in no stock pak under any extension;
///   retail binds its default image there too.
const UNRESOLVED_STAGE_IMAGES: &[&str] = &["$dlight", "textures/battleship/deckflag_np.tga"];

/// Every stage image path of every authored material on every stock MP map
/// must resolve through the renderer's probe chain
/// (`assets::resolve_bundle_image`). Fails listing unresolved paths - catches
/// probe regressions and silent checkerboard fallbacks.
#[test]
fn stock_mp_stage_images_resolve() {
    let Some((_tmp, fs)) = stock_paks() else {
        return;
    };
    let lib = ShaderLib::load(&fs);
    let maps: Vec<String> = fs
        .find_maps()
        .into_iter()
        .filter(|m| m.starts_with("mp_"))
        .collect();
    assert!(!maps.is_empty(), "no MP maps found in stock paks");

    let mut unresolved: BTreeSet<String> = BTreeSet::new();
    let mut checked = 0usize;
    for map in &maps {
        let entry = fs.resolve_map(map).unwrap();
        let b = bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let mats: BTreeSet<String> = b
            .soups
            .iter()
            .map(|s| {
                b.materials[s.material as usize]
                    .name
                    .to_lowercase()
                    .replace('\\', "/")
            })
            .collect();
        for mat in &mats {
            let Some(sh) = lib.get(mat) else { continue };
            for st in &sh.stages {
                for bundle in &st.bundles {
                    let mut paths: Vec<&str> = match &bundle.image {
                        ImageRef::Path(p) => vec![p.as_str()],
                        _ => vec![],
                    };
                    if let Some(anim) = &bundle.anim {
                        paths.extend(anim.paths.iter().map(String::as_str));
                    }
                    for p in paths {
                        checked += 1;
                        let resolved = assets::resolve_bundle_image(&fs, p).is_some();
                        if !resolved && !UNRESOLVED_STAGE_IMAGES.contains(&p) {
                            unresolved.insert(p.to_string());
                        }
                    }
                }
            }
        }
    }
    assert!(
        unresolved.is_empty(),
        "{checked} stage image paths checked, {} unresolved: {unresolved:?}",
        unresolved.len()
    );
}
