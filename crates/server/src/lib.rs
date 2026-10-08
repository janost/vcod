//! The CoD 1.1 dedicated server. Transport lives in `main.rs`, so the same
//! `Server` drives the UDP loop and the tests.

pub mod archive;
pub mod area;
pub mod bans;
pub mod bots;
pub mod client;
pub mod compass;
pub mod configstrings;
pub mod console;
pub mod cvars;
pub mod follow;
pub mod game;
pub mod items;
pub mod master;
pub mod nav;
pub mod push;
pub mod rcon;
pub mod server;
pub mod spectate;
pub mod weapons;
pub mod world;

pub use server::{Server, ServerConfig};
