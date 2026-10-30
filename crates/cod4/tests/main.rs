//! The crate's integration tests as one binary: linked once, not once per file.

mod anim_census;
mod clip_census;
mod cull_census;
mod drawsurf_census;
mod fx_census;
mod hud_assets;
mod iwi_census;
mod lightgrid_census;
mod look_census;
mod map_census;
mod map_spots;
mod mapworld_census;
mod mesh_pins;
mod mod_census;
mod moves_ab;
mod net_decode;
mod render_pins;
mod sound_census;
mod sunshadow_census;
mod view_census;
mod vis_rays;
mod walk_spots;
mod weapon_files;
mod weapon_view;
mod zone_census;

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
