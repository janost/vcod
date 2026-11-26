//! The client's tools (`vcod::tools`), one per run:
//! `cargo run -p vcod --example client_tools -- <tool>`. Each tool's
//! module doc names its environment.

fn main() {
    let tool = std::env::args().nth(1).unwrap_or_default();
    let run: fn() = match tool.as_str() {
        "cod4_map_sweep" => vcod::tools::cod4_sweep::cod4_map_sweep,
        "cod4_soak" => vcod::tools::cod4_sweep::cod4_soak,
        "cod2_soak" => vcod::tools::cod2_soak::cod2_soak,
        _ => {
            eprintln!("usage: client_tools cod4_map_sweep|cod4_soak|cod2_soak");
            std::process::exit(2);
        }
    };
    run();
}
