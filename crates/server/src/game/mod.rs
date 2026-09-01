//! The game module: the script runtime and the CoD-specific host the VM
//! calls into. `server.rs` keeps the engine side.

pub mod builtins;
pub mod damage;
pub mod entity;
pub mod fields;
pub mod host;
pub mod script;
pub mod spawn;
pub mod wire;

#[cfg(test)]
pub mod testing {
    use crate::game::host::GameHost;
    use vcod_gsc::Vm;

    /// A VM and a host with an empty object table, for the game module's tests.
    pub fn fixture() -> (Vm, GameHost) {
        (Vm::new(), GameHost::new(vec![String::new(); 2048]))
    }
}
