//! Every stock 1.7 fastfile, client (`$COD4_DIR/zone/english`) and
//! dedicated (`$COD4_LNXDED_DIR/zone/english`), read to its asset bodies,
//! every body walked and the stream inflated to its end
//! (`docs/research/cod4-fastfile.md`). The walk must end on the last byte
//! and leave every block where the `XFile`'s `blockSize` says. Prints each
//! zone's assets by type; `--nocapture` shows them. `COD4_ZONES=a,b` keeps
//! only the zones whose name holds one of those words.

use anyhow::{Result, ensure};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use vcod_cod4::testing::{cod4_install, cod4_lnxded_install};
use vcod_cod4::walk::{WalkOptions, Walked, walk};

/// Values worth printing from a walked zone.
fn spots(w: &Walked) -> Vec<String> {
    let a = &w.assets;
    let mut out = Vec::new();
    for r in &a.raw_files {
        if r.name.ends_with(".gsc") && out.is_empty() {
            out.push(format!("rawfile {} {} bytes", r.name, r.data.len()));
        }
    }
    out
}
use vcod_cod4::zone::{ASSET_LIST_SIZE, AssetType, Block, XFILE_SIZE, Zone};

struct Census {
    strings: usize,
    assets: usize,
    by_type: BTreeMap<AssetType, usize>,
    /// Offset tokens the walk could not name, by slot kind.
    unresolved: BTreeMap<&'static str, u32>,
    /// A few decoded values, printed and checked by the caller.
    spots: Vec<String>,
}

fn census(path: &Path) -> Result<Census> {
    let zone = Zone::open_path(path)?;

    // Nothing sits between the list and the first body: the list's own
    // bytes account for every byte before it.
    let string_bytes: u64 = zone
        .script_strings
        .iter()
        .map(|s| s.as_ref().map_or(0, |s| s.len() as u64 + 1))
        .sum();
    let list_end = XFILE_SIZE
        + ASSET_LIST_SIZE
        + 4 * zone.script_strings.len() as u64
        + string_bytes
        + 8 * zone.assets.len() as u64;
    ensure!(
        zone.body_offset() == list_end,
        "list ends at {:#x}, bodies at {list_end:#x}",
        zone.body_offset()
    );

    // Every header the loader reads is a body inline in the stream; the
    // types it skips carry whatever the linker left (snddriverglobals).
    for (i, a) in zone.assets.iter().enumerate() {
        ensure!(
            !a.kind.has_loader() || a.header.is_inline(),
            "asset {i} ({}) header {:?}",
            a.kind.name(),
            a.header
        );
    }
    let mut by_type = BTreeMap::new();
    for a in &zone.assets {
        *by_type.entry(a.kind).or_default() += 1;
    }
    let (strings, assets) = (zone.script_strings.len(), zone.assets.len());
    let entries = zone.assets.clone();
    let block_size = zone.header.block_size;
    let walked = walk(zone, WalkOptions::default())?;

    // Every block ends where the XFile sized it; the temp block rewinds on
    // each pop, so its size is its peak.
    for b in Block::ALL {
        let i = b as usize;
        // The linker counts the 16-byte XAssetList in the temp block,
        // which 1.7 reads into a static instead.
        let at = if b == Block::Temp {
            walked.stats.peak[i] + ASSET_LIST_SIZE as u32
        } else {
            walked.stats.end[i]
        };
        ensure!(
            at == block_size[i],
            "block {b:?} ends at {at:#x}, blockSize {:#x}",
            block_size[i]
        );
    }
    // Every offset token named something the walk had recorded.
    ensure!(
        walked.stats.unresolved.is_empty(),
        "unresolved offsets: {:?}",
        walked.stats.unresolved
    );
    // Each header slot holds an asset of its own type.
    for (i, (a, r)) in entries.iter().zip(&walked.list).enumerate() {
        match r {
            Some(r) => ensure!(
                r.kind() == a.kind,
                "asset {i}: {:?} for {}",
                r,
                a.kind.name()
            ),
            None => ensure!(
                !a.kind.has_loader(),
                "asset {i} ({}) loaded nothing",
                a.kind.name()
            ),
        }
    }
    Ok(Census {
        strings,
        assets,
        by_type,
        unresolved: walked
            .stats
            .unresolved
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect(),
        spots: spots(&walked),
    })
}

fn histogram(by_type: &BTreeMap<AssetType, usize>) -> String {
    by_type
        .iter()
        .map(|(t, n)| format!("{} {n}", t.name()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[test]
fn every_stock_zone_walks_to_its_end() {
    let filter: Option<Vec<String>> = std::env::var("COD4_ZONES")
        .ok()
        .map(|v| v.split(',').map(str::to_owned).collect());
    let mut zones = Vec::new();
    for (label, install) in [
        ("client", cod4_install()),
        ("lnxded", cod4_lnxded_install()),
    ] {
        if let Some(install) = install {
            for path in install.zones("english").unwrap() {
                let name = path.file_stem().unwrap().to_string_lossy().into_owned();
                if filter
                    .as_ref()
                    .is_none_or(|f| f.iter().any(|w| name.contains(w.as_str())))
                {
                    zones.push((label, path));
                }
            }
        }
    }
    if zones.is_empty() {
        return;
    }

    let next = Mutex::new(zones.iter());
    let results = Mutex::new(BTreeMap::new());
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(4);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    // The queue's lock drops at the end of the statement.
                    let Some((label, path)) = next.lock().unwrap().next() else {
                        break;
                    };
                    let r = census(path);
                    results.lock().unwrap().insert((*label, path.clone()), r);
                }
            });
        }
    });

    let results = results.into_inner().unwrap();
    let mut total: BTreeMap<AssetType, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    for ((label, path), r) in &results {
        let name = path.file_stem().unwrap().to_string_lossy();
        match r {
            Ok(c) => {
                println!(
                    "{label} {name}: {} strings, {} assets: {}",
                    c.strings,
                    c.assets,
                    histogram(&c.by_type)
                );
                if !c.unresolved.is_empty() {
                    println!("  unresolved offsets: {:?}", c.unresolved);
                }
                for spot in &c.spots {
                    println!("  {spot}");
                }
                if *label == "client" {
                    for (t, n) in &c.by_type {
                        *total.entry(*t).or_default() += n;
                    }
                }
            }
            Err(e) => {
                println!("{label} {name}: FAILED {e:#}");
                failures.push(format!("{label} {name}: {e:#}"));
            }
        }
    }
    let mut ranked: Vec<_> = total.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1));
    println!(
        "{} zones; client total: {}",
        results.len(),
        ranked
            .iter()
            .map(|(t, n)| format!("{} {n}", t.name()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    assert!(
        failures.is_empty(),
        "{} zones failed:\n{}",
        failures.len(),
        failures.join("\n")
    );

    // The counts a first survey of the stock zones found.
    for ((_, path), r) in &results {
        let c = r.as_ref().unwrap();
        match path.file_stem().unwrap().to_str().unwrap() {
            "code_post_gfx_mp" => {
                assert_eq!((c.strings, c.assets), (165, 368));
                use AssetType::*;
                let want = [
                    (Material, 200),
                    (TechniqueSet, 94),
                    (StringTable, 30),
                    (RawFile, 23),
                    (Image, 6),
                    (XModel, 5),
                    (XAnimParts, 2),
                    (MenuList, 2),
                    (PhysPreset, 1),
                    (SndCurve, 1),
                    (SndDriverGlobals, 1),
                    (Fx, 1),
                    (ImpactFx, 1),
                    (LightDef, 1),
                ];
                assert_eq!(c.by_type, want.into_iter().collect());
            }
            "ui_mp" => {
                assert_eq!((c.strings, c.assets), (0, 113));
                use AssetType::*;
                let want = [
                    (Material, 109),
                    (TechniqueSet, 2),
                    (MenuList, 1),
                    (RawFile, 1),
                ];
                assert_eq!(c.by_type, want.into_iter().collect());
            }
            _ => {}
        }
    }
}
