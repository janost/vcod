//! The client's tools: runs over a live server and the real client frame
//! that write a report rather than pass or fail. `examples/client_tools.rs`
//! runs them by name; the gates they share a rig with stay in the crate.

pub mod cod2_soak;
pub mod cod4_sweep;
