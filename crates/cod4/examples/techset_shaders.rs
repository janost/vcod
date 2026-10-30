//! Writes a technique set's shader bytecode out of a map zone, one file a
//! technique pass (`<techset>_t<technique>_p<pass>.ps` and `.vs`), and
//! prints each pass's shader names and arguments, for
//! `tools/re/d3d9dis.py`: `cargo run -p vcod-cod4 --example
//! techset_shaders [map] [techset] [out dir]` (default `mp_crash`,
//! `wc_l_sm_r0c0`, the workspace's `tmp/cod4-shaders/`). A techset of
//! `list` prints the zone's set names; any zone works as the map
//! (`code_post_gfx_mp` holds the post sets).

use anyhow::{Context, Result};
use std::path::PathBuf;
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let map = args.next().unwrap_or_else(|| "mp_crash".into());
    let want = args.next().unwrap_or_else(|| "wc_l_sm_r0c0".into());
    let out = args.next().map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tmp/cod4-shaders")
    });
    let install = cod4_install().context("COD4_DIR is not a CoD4 install")?;
    let zone = Zone::open_path(&install.zone("english", &map))?;
    let a = walk(zone, WalkOptions { keep_bulk: true })?.assets;
    if want == "list" {
        for t in &a.techsets {
            println!("{}", t.name);
        }
        return Ok(());
    }
    std::fs::create_dir_all(&out)?;
    let set = a
        .techsets
        .iter()
        .find(|t| t.name.trim_start_matches(',') == want)
        .with_context(|| format!("{map} has no techset {want}"))?;
    // the materials drawn with the set, and their techniques 4 and 5's
    // (unlit, emissive) state
    for m in &a.materials {
        if m.techset
            .is_some_and(|t| a.techsets[t.index()].name == set.name)
        {
            for tech in [4, 5] {
                let st = m
                    .state_bits_entry
                    .get(tech)
                    .and_then(|&e| m.state_bits.get(usize::from(e)));
                let state = st.map(|&b| vcod_cod4::state::GfxState::decode(b));
                println!("material {}: technique {tech} state {state:?}", m.name);
            }
            for c in &m.constants {
                println!(
                    "material {}: constant {} {:#x} {:?}",
                    m.name,
                    String::from_utf8_lossy(&c.name).trim_end_matches('\0'),
                    c.name_hash,
                    c.literal
                );
            }
        }
    }
    for (i, t) in set.techniques.iter().enumerate() {
        let Some(t) = t else { continue };
        let t = &a.techniques[t.index()];
        for (pi, p) in t.passes.iter().enumerate() {
            let name = |s: &Option<vcod_cod4::assets::material::Shader>| {
                s.as_ref().map_or("-".to_string(), |s| s.name.clone())
            };
            println!(
                "technique {i} {} pass {pi}: vs {} ps {} args {:?}",
                t.name,
                name(&p.vertex_shader),
                name(&p.pixel_shader),
                p.args
                    .iter()
                    .map(|a| (a.kind, a.dest, a.value))
                    .collect::<Vec<_>>()
            );
            for (ext, sh) in [("ps", &p.pixel_shader), ("vs", &p.vertex_shader)] {
                if let Some(prog) = sh.as_ref().and_then(|s| s.program.as_ref()) {
                    std::fs::write(out.join(format!("{want}_t{i}_p{pi}.{ext}")), prog)?;
                }
            }
        }
    }
    Ok(())
}
