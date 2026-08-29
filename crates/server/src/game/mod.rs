//! The game module: the script runtime and the CoD-specific host the VM
//! calls into. `server.rs` keeps the engine side.

pub mod builtins;
pub mod entity;
pub mod fields;
pub mod host;
pub mod script;
