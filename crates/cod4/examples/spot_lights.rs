//! Prints a stock map's spot and omni primary lights (cod4-world.md 3.5):
//! index, type, origin, the beam's direction, radius, the outer cone's
//! half angle, how many world surfaces store the light and its
//! `canUseShadowMap`, for finding
//! where a light's draw (or its shadow map) shows in a map:
//! `cargo run -p vcod-cod4 --example spot_lights [map...]` (default
//! `mp_crash`).

use anyhow::{Context, Result};
use vcod_cod4::map::Map;
use vcod_cod4::testing::cod4_install;

fn main() -> Result<()> {
    let mut maps: Vec<String> = std::env::args().skip(1).collect();
    if maps.is_empty() {
        maps.push("mp_crash".into());
    }
    let install = cod4_install().context("COD4_DIR is not a CoD4 install")?;
    for name in &maps {
        let (map, _) = Map::load(&install, "english", name)?;
        let mut stored = vec![0usize; map.primary_lights.len()];
        for s in &map.surfaces {
            if let Some(n) = stored.get_mut(usize::from(s.primary_light)) {
                *n += 1;
            }
        }
        for (i, l) in map.primary_lights.iter().enumerate() {
            let kind = match l.kind {
                2 => "spot",
                3 => "omni",
                _ => continue,
            };
            // `dir` points back at the light: the beam runs against it
            let beam = l.dir.map(|c| -c);
            println!(
                "{name} light {i} {kind} origin ({:.0} {:.0} {:.0}) beam ({:.2} {:.2} {:.2}) \
                 radius {:.0} half angle {:.1} surfaces {} shadow {}",
                l.origin[0],
                l.origin[1],
                l.origin[2],
                beam[0],
                beam[1],
                beam[2],
                l.radius,
                l.cos_half_fov_outer.clamp(-1.0, 1.0).acos().to_degrees(),
                stored[i],
                u8::from(l.can_shadow)
            );
        }
    }
    Ok(())
}
