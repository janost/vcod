//! The client's tools: runs over a live server and the real client frame
//! that write a report rather than pass or fail. `examples/client_tools.rs`
//! runs them by name; the gates they share a rig with stay in the crate.

pub mod cod2_census;
pub mod cod2_soak;
pub mod cod4_sweep;

pub use crate::renderer::cod2_profile::{
    cod2_cell_walks_draw_alike, cod2_cull_profile, cod2_load_profile,
};
pub use crate::renderer::hud_shot::{
    cod2_dumped_hud_shot, cod2_main_menu_shot, cod2_player_hud_shot, cod2_ui_shot,
};
