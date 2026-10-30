//! Writes every rawfile of a zone under a directory, by its own path.
//!
//!     cargo run -p vcod-cod4 --example rawfile_dump -- ZONE.ff OUT_DIR

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let zone = args.next().context("usage: ZONE.ff OUT_DIR")?;
    let out = std::path::PathBuf::from(args.next().context("usage: ZONE.ff OUT_DIR")?);
    let z = vcod_cod4::zone::Zone::open_path(std::path::Path::new(&zone))?;
    let walked = vcod_cod4::walk::walk(z, Default::default())?;
    for r in &walked.assets.raw_files {
        let p = out.join(&r.name);
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, &r.data)?;
    }
    println!("{} rawfiles", walked.assets.raw_files.len());
    Ok(())
}
