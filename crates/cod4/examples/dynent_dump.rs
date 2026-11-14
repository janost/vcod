//! Prints a map zone's dynamic entities: each `DynEntityDef`'s words, its
//! model, destroyed pieces, effect and preset (cod4-world.md 13).
//!
//!     cargo run -p vcod-cod4 --example dynent_dump -- ZONE.ff

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let zone = std::env::args().nth(1).context("usage: ZONE.ff")?;
    let z = vcod_cod4::zone::Zone::open_path(std::path::Path::new(&zone))?;
    let a = vcod_cod4::walk::walk(z, Default::default())?.assets;
    let clip = a
        .clip_maps_pvs
        .first()
        .or(a.clip_maps.first().map(|c| &c.0));
    let Some(clip) = clip else { return Ok(()) };
    for (k, list) in clip.dyn_ents.iter().enumerate() {
        println!("list {k}: {}", list.len());
        for (i, d) in list.iter().enumerate() {
            let u = |o: usize| u32::from_le_bytes(d.raw[o..o + 4].try_into().unwrap());
            let f = |o: usize| f32::from_le_bytes(d.raw[o..o + 4].try_into().unwrap());
            let model = d.model.map(|m| a.xmodels[m.index()].name.as_str());
            let fx = d.destroy_fx.map(|m| a.fx[m.index()].name.as_str());
            let preset = d.preset.map(|p| a.phys_presets[p.index()].name.as_str());
            println!(
                "{k}.{i} type {} quat ({} {} {} {}) origin ({} {} {}) model {model:?} \
                 brush {} {} fx {fx:?} preset {preset:?} health {} words {:08x?}",
                u(0),
                f(4),
                f(8),
                f(12),
                f(16),
                f(20),
                f(24),
                f(28),
                u16::from_le_bytes([d.raw[36], d.raw[37]]),
                u16::from_le_bytes([d.raw[38], d.raw[39]]),
                u(52) as i32,
                (52..96).step_by(4).map(u).collect::<Vec<_>>(),
            );
            if let Some(p) = &d.pieces {
                for (m, off) in &p.pieces {
                    let m = m.map(|m| a.xmodels[m.index()].name.as_str());
                    println!("    piece {} {m:?} {off:?}", p.name);
                }
            }
        }
    }
    Ok(())
}
