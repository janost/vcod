//! Every physics model element (type 5, flag 0x8000000) of `common_mp`
//! and the MP map zones, with what 0x4a1ff0 reads off it: looping or
//! one-shot, the run bits (flags 0xc0), the spawn offset (0x30), flag 0x2,
//! nonzero `spawnAngles`, the effect's `msecLoopingLife`, whether its seed
//! is timed (0x4a0ea0) and the effects whose chains (impact, death,
//! emitted or runner effect) play it (cod4-effects.md 9.1.1).
//!
//!     COD4_DIR=... cargo run -p vcod-cod4 --example fx_phys_census

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use vcod_cod4::assets::fx::FxVisual;
use vcod_cod4::fx::{ElemDef, flags};
use vcod_cod4::fx_phys::{effects, is_physics};
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;

fn main() -> Result<()> {
    let install = cod4_install().context("COD4_DIR")?;
    let mut names: Vec<String> = install
        .zones("english")?
        .iter()
        .filter_map(|p| Some(p.file_stem()?.to_str()?.to_string()))
        .filter(|n| n == "common_mp" || (n.starts_with("mp_") && !n.ends_with("_load")))
        .collect();
    names.sort();
    let mut zones = Vec::new();
    for n in &names {
        let z = Zone::open_path(&install.zone("english", n))?;
        zones.push((n.clone(), walk(z, WalkOptions { keep_bulk: false })?.assets));
    }
    // effect name -> the effects any element names (impact, death,
    // emitted, runner visuals)
    let mut chains: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (_, a) in &zones {
        for fx in &a.fx {
            let set = chains.entry(fx.name.to_ascii_lowercase()).or_default();
            for e in &fx.elems {
                let runners = e.visuals.iter().filter_map(|v| match v {
                    FxVisual::Effect(Some(n)) => Some(n),
                    _ => None,
                });
                for n in [&e.effect_on_impact, &e.effect_on_death, &e.effect_emitted]
                    .into_iter()
                    .flatten()
                    .chain(runners)
                {
                    set.insert(n.to_ascii_lowercase());
                }
            }
        }
    }
    let all: Vec<_> = zones.iter().map(|(_, a)| a).collect();
    let phys = effects(&all);
    let mut seen = BTreeSet::new();
    for (zone, a) in &zones {
        for fx in &a.fx {
            let looping = fx.looping.max(0) as usize;
            for (i, e) in fx.elems.iter().enumerate() {
                let d = ElemDef::of(e);
                if !is_physics(&d) || !seen.insert((fx.name.clone(), i)) {
                    continue;
                }
                let kind = if i < looping {
                    "looping"
                } else if i < looping + fx.one_shot.max(0) as usize {
                    "one-shot"
                } else {
                    "emitted"
                };
                let angles = d
                    .spawn_angles
                    .iter()
                    .any(|r| r.base != 0.0 || r.amplitude != 0.0);
                let offset = match d.flags & flags::SPAWN_OFFSET_MASK {
                    0 => "none",
                    flags::SPAWN_OFFSET_SPHERE => "sphere",
                    _ => "cylinder",
                };
                let parents: Vec<&String> = chains
                    .iter()
                    .filter(|(_, s)| s.contains(&fx.name.to_ascii_lowercase()))
                    .map(|(k, _)| k)
                    .collect();
                let timed = phys.get(&fx.name).is_some_and(|p| p.timed_seed);
                println!(
                    "{zone} {} elem {i} {kind} flags {:#010x} run {:#04x} offset {offset} \
                     rel2 {} angles {angles} spawn {:?} looping_life {} timed {timed} \
                     chained_from {parents:?}",
                    fx.name,
                    d.flags,
                    d.flags & flags::RUN_MASK,
                    d.flags & flags::SPAWN_RELATIVE_TO_EFFECT != 0,
                    d.spawn,
                    fx.msec_looping_life,
                );
            }
        }
    }
    Ok(())
}
