//! The game module: the script runtime and the CoD-specific host the VM
//! calls into. `server.rs` keeps the engine side.

pub mod bodies;
pub mod builtins;
pub mod combat;
pub mod entity;
pub mod fields;
pub mod hitrig;
pub mod host;
pub mod missile;
pub mod script;
pub mod spawn;
pub mod temp_entity;
pub mod trigger;
pub mod wire;

/// `testing::fixture` with a two-submodel world attached, for the paths that
/// read brush bounds. Model 1 is a 32x32x72 box, model 2 a 64-unit cube;
/// both are the shapes the trigger tests assert against.
#[cfg(test)]
pub fn testing_world_fixture() -> (vcod_gsc::Vm, host::GameHost) {
    let (vm, mut host) = testing::fixture();
    host.model_bounds = vec![
        ([0.0; 3], [0.0; 3]),
        ([-16.0, -16.0, 0.0], [16.0, 16.0, 72.0]),
        ([-32.0; 3], [32.0; 3]),
    ];
    (vm, host)
}

#[cfg(test)]
pub mod testing {
    use crate::game::host::GameHost;
    use vcod_gsc::Vm;

    /// A VM and a host with an empty object table, for the game module's tests.
    pub fn fixture() -> (Vm, GameHost) {
        (Vm::new(), GameHost::new(vec![String::new(); 2048]))
    }
}
