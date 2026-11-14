//! The server's integration tests as one binary: linked once, not once per file.

mod animtimers_ab;
mod blast_entity_uo_ab;
mod bootstrap;
mod bot_admin;
mod bot_objectives;
mod bot_threads;
mod bot_vehicles;
mod bots;
mod bump_ab;
mod client_fields;
mod combat;
mod common;
mod compass;
mod configstrings_15_ab;
mod configstrings_ab;
mod configstrings_uo_ab;
mod connect;
mod cull15_ab;
mod demos;
mod dmgtrig_ab;
mod dyingpose_ab;
mod entities_ab;
mod entity_vis_ab;
mod fall_ab;
mod flood_protect;
mod fx_ab;
mod gametypes_ab;
mod gunkick_ab;
mod hitching_client;
mod hud;
mod hurtfall_ab;
mod istouching_ab;
mod itemdrop_ab;
mod kill_cmds_ab;
mod killcam;
mod killcam_ab;
mod ladder_ab;
mod ladder_yawcap_ab;
mod lean_ab;
mod leanhit_ab;
mod leanpvs_ab;
mod legs_ab;
mod link;
mod linkto_ab;
mod mapchange_ab;
mod menuclock_ab;
mod minefield_uo_ab;
mod missile_ab;
mod missile_edge_ab;
mod model_bounds_ab;
mod mount_uo_ab;
mod movers_ab;
mod moves_ab;
mod moveshot_ab;
mod nav_census;
mod noclass_ab;
mod pickup_ab;
mod placespawn_ab;
mod player_clip;
mod playerstate_ab;
mod playerstate_combat_ab;
mod playerstate_motion_ab;
mod playerstate_slope_ab;
mod playerstate_uo_ab;
mod playsound_broadcast;
mod predict_ab;
mod pronecrouch_ab;
mod pronepose_ab;
mod push_ab;
mod push_candidate_ab;
mod push_order_ab;
mod restart_first_snapshot;
mod ride_ab;
mod rise_ab;
mod roundrestart_ab;
mod script_configstrings;
mod scriptmodel_ab;
mod sd_plant_ab;
mod semantics_ents;
mod shellshock_ab;
mod sine_ab;
mod spawnbody_ab;
mod spawnclip_ab;
mod spawnint_uo_ab;
mod spawnview_ab;
mod specspawn_ab;
mod stuck_ab;
mod suicide_playing;
mod testclient;
mod threadorder_ab;
mod timeout_15_ab;
mod touchorder_ab;
mod triggers_ab;
mod turret_ab;
mod two_clients;
mod uo_weapons_ab;
mod uocull_ab;
mod vehicle_ab;
mod vehicle_armor_ab;
mod vehicle_blast_ab;
mod vehicle_die_ab;
mod vehicle_drive_ab;
mod vehicle_fire_ab;
mod vehicle_fuse_ab;
mod vehicle_hits_ab;
mod vehicle_ram_ab;
mod vehicle_runover_ab;
mod vehicle_shell_ab;
mod vehicle_wreck_ab;
mod votes_ab;
mod weapsel_ab;
mod worldcopy_ab;
mod www_download;

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
