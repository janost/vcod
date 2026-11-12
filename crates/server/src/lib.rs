//! The CoD 1.1 dedicated server. Transport lives in `main.rs`, so the same
//! `Server` drives the UDP loop and the tests.

/// `<package version>-<git short sha>`, set by `crates/version.rs`.
pub const VERSION: &str = env!("VCOD_VERSION");

pub mod archive;
pub mod area;
pub mod botmgmt;
pub mod bots;
pub mod client;
pub mod cod2;
pub mod cod4;
pub mod compass;
pub mod configstrings;
pub mod console;
pub mod cvars;
pub mod defs;
pub mod download;
pub mod engine;
pub mod follow;
pub mod game;
pub mod items;
pub mod master;
pub mod nav;
pub mod push;
pub mod rcon;
pub mod server;
pub mod spectate;
pub mod stdin_console;
pub mod tick_profile;
pub mod tick_trace;
pub mod weapons;
pub mod world;

pub use server::{Server, ServerConfig};
