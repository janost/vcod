//! Prints the `WeaponDef` fields the weapon half reads
//! (`vcod_cod4::pmove::weapon::WeaponDef`) for each weapon of a zone whose
//! name contains a filter.
//!
//!     cargo run -p vcod-cod4 --example weapon_dump -- ZONE.ff [FILTER]

use anyhow::{Context, Result};
use vcod_cod4::pmove::weapon::WeaponDef;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let zone = args.next().context("usage: ZONE.ff [FILTER]")?;
    let filter = args.next().unwrap_or_default().to_ascii_lowercase();
    let z = vcod_cod4::zone::Zone::open_path(std::path::Path::new(&zone))?;
    let walked = vcod_cod4::walk::walk(z, Default::default())?;
    for w in &walked.assets.weapons {
        if !w.name.to_ascii_lowercase().contains(&filter) {
            continue;
        }
        match WeaponDef::of(w) {
            Some(d) => println!("{d:#?}"),
            None => println!("{}: {} bytes", w.name, w.raw.len()),
        }
    }
    Ok(())
}
