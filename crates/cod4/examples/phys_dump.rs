//! Prints a zone's physics presets (`PhysPreset`'s floats) and every
//! xmodel that names one: its bounds and its `physGeoms`, if any
//! (cod4-gsc.md 3.2).
//!
//!     cargo run -p vcod-cod4 --example phys_dump -- ZONE.ff [MODEL_PREFIX]

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let zone = args.next().context("usage: ZONE.ff [MODEL_PREFIX]")?;
    let prefix = args.next().unwrap_or_default();
    let z = vcod_cod4::zone::Zone::open_path(std::path::Path::new(&zone))?;
    let a = vcod_cod4::walk::walk(z, Default::default())?.assets;
    let f = |b: &[u8], o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    for p in &a.phys_presets {
        let r = &p.raw;
        println!(
            "preset {} type {} mass {} bounce {} friction {} bulletForceScale {} \
             explosiveForceScale {} sndAliasPrefix {:?} spread {} upward {} cylinder {}",
            p.name,
            i32::from_le_bytes(r[4..8].try_into().unwrap()),
            f(r, 8),
            f(r, 12),
            f(r, 16),
            f(r, 20),
            f(r, 24),
            p.snd_alias_prefix,
            f(r, 32),
            f(r, 36),
            r[40],
        );
    }
    for m in a.xmodels.iter().filter(|m| m.name.starts_with(&prefix)) {
        let Some(p) = m.phys_preset else { continue };
        println!(
            "model {} preset {} mins {:?} maxs {:?}",
            m.name,
            a.phys_presets[p.index()].name,
            m.mins,
            m.maxs
        );
        if let Some(g) = &m.phys_geoms {
            println!(
                "  com {:?} moments {:?} products {:?}",
                g.center_of_mass, g.moments, g.products
            );
            for g in &g.geoms {
                println!(
                    "  geom brush {} type {} offset {:?} half {:?} axis {:?}",
                    g.brush, g.kind, g.offset, g.half_lengths, g.orientation
                );
            }
        }
    }
    Ok(())
}
