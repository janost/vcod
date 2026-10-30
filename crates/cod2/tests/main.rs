//! The crate's integration tests as one binary: linked once, not once per file.

mod anim_ab;
mod collision_maps;
mod cookie_census;
mod d3dbsp_maps;
mod demo;
mod efx_stock;
mod feather_census;
mod fonts;
mod lightgrid_maps;
mod material_stock;
mod materials_lib;
mod model_load;
mod moves_ab;
mod player_rigs;
mod pmove_walk;
mod pure;
mod replay;
mod trace_ab;
mod view;
mod view_rigs;
mod weapons;
mod world_blends;
mod world_maps;
mod xmodel_census;

/// A test file left out of the list above would never build or run.
#[test]
fn every_test_file_is_a_module() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests");
    let main = std::fs::read_to_string(format!("{dir}/main.rs")).unwrap();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().is_some_and(|e| e == "rs") && stem != "main" {
            assert!(
                main.contains(&format!("\nmod {stem};")),
                "tests/{stem}.rs is not in main.rs"
            );
        }
    }
}
