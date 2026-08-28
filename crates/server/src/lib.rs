//! The CoD 1.1 dedicated server. Transport lives in `main.rs`, so the same
//! `Server` drives the UDP loop and the tests.

pub mod client;
pub mod configstrings;
pub mod game;
pub mod server;
pub mod spectate;
pub mod world;

pub use server::{Server, ServerConfig};
