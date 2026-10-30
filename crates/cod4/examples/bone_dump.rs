//! Prints each bone of the zone's xmodels whose name contains a filter:
//! its name, hit location (`partClassification`) and `XBoneInfo` (bounds,
//! offset, radius squared).
//!
//!     cargo run -p vcod-cod4 --example bone_dump -- ZONE.ff FILTER

use anyhow::{Context, Result};

fn f(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let zone = args.next().context("usage: ZONE.ff FILTER")?;
    let filter = args.next().unwrap_or_default().to_ascii_lowercase();
    let z = vcod_cod4::zone::Zone::open_path(std::path::Path::new(&zone))?;
    let walked = vcod_cod4::walk::walk(z, Default::default())?;
    let a = &walked.assets;
    for x in &a.xmodels {
        if !x.name.to_ascii_lowercase().contains(&filter) || x.bone_names.is_empty() {
            continue;
        }
        println!("{} ({} bones)", x.name, x.num_bones);
        for (i, &n) in x.bone_names.iter().enumerate() {
            let name = vcod_cod4::anim::script_string(a, n);
            let class = x.part_classification.get(i).copied().unwrap_or(0);
            let Some(b) = x.bone_info.get(0x28 * i..0x28 * i + 0x28) else {
                println!("  {i:3} {name} class {class}");
                continue;
            };
            let v: Vec<f32> = (0..10).map(|k| f(b, 4 * k)).collect();
            println!(
                "  {i:3} {name:<24} class {class:2} min {:?} max {:?} off {:?} r2 {}",
                &v[0..3],
                &v[3..6],
                &v[6..9],
                v[9]
            );
        }
    }
    Ok(())
}
