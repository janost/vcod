//! Prints the parsed stages of named shader blocks, e.g.
//! `COD_DIR=... cargo run -p vcod-common --example dump_shader -- textures/sfx/mp_ship_ocean`.
use vcod_common::pk3::Pk3Fs;

fn main() {
    let dir = std::path::Path::new(&std::env::var("COD_DIR").expect("COD_DIR")).join("main");
    let fs = Pk3Fs::open(&dir).expect("fs");
    let lib = vcod_common::shader::ShaderLib::load(&fs);
    for name in std::env::args().skip(1) {
        let Some(sh) = lib.get(&name) else {
            println!("{name}: NOT IN LIB");
            continue;
        };
        println!(
            "{name}: stages {} dropped {} sort {:?} surface {:?} sky {:?} sunfile {:?}",
            sh.stages.len(),
            sh.dropped_stages,
            sh.sort,
            sh.surface,
            sh.sky.as_ref().map(|s| s.env.as_str()),
            sh.sunfile
        );
        for (i, st) in sh.stages.iter().enumerate() {
            println!(
                "  stage {i}: bundles {} blend {:?} depth_write {:?} rgb {:?} alpha {:?} af {:?}",
                st.bundles.len(),
                st.blend,
                st.depth_write,
                st.rgb_gen,
                st.alpha_gen,
                st.alpha_func
            );
            for b in &st.bundles {
                println!(
                    "    bundle {:?} clamp {} tcmods {}",
                    b.image,
                    b.clamp,
                    b.tcmods.len()
                );
            }
        }
    }
}
