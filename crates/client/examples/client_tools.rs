//! The client's tools (`vcod::tools`), one per run: `cargo run -p vcod
//! --features testing --example client_tools -- <tool>`. Each tool's doc
//! names its environment (docs/dev/testing.md, "Client tools").

use vcod::tools;

const TOOLS: &[(&str, fn())] = &[
    ("cod4_map_sweep", tools::cod4_sweep::cod4_map_sweep),
    ("cod4_soak", tools::cod4_sweep::cod4_soak),
    ("cod2_soak", tools::cod2_soak::cod2_soak),
    ("cod2_map_census", tools::cod2_census::cod2_map_census),
    ("cod2_main_menu_shot", tools::cod2_main_menu_shot),
    ("cod2_dumped_hud_shot", tools::cod2_dumped_hud_shot),
    ("cod2_player_hud_shot", tools::cod2_player_hud_shot),
    ("cod2_ui_shot", tools::cod2_ui_shot),
];

fn main() {
    let name = std::env::args().nth(1).unwrap_or_default();
    match TOOLS.iter().find(|(n, _)| *n == name) {
        Some((_, run)) => run(),
        None => {
            let names: Vec<&str> = TOOLS.iter().map(|(n, _)| *n).collect();
            eprintln!("usage: client_tools <{}>", names.join("|"));
            std::process::exit(2);
        }
    }
}
