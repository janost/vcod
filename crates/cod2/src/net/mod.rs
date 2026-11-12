//! The CoD2 1.3 wire, protocol 118 (docs/research/cod2-protocol.md): its
//! states, usercmd, parser and client over vcod-common's message codec,
//! netchan ([`vcod_common::net::netchan::NetchanParams::COD2`]) and Huffman
//! tree (equal to CoD2's, 2.1).

pub mod capture;
pub mod client;
pub mod delta;
pub mod demo;
pub mod download;
pub mod events;
pub mod fields;
pub mod flags;
pub mod oob;
pub mod ops;
pub mod parse;
pub mod protocol;
pub mod pure;
pub mod usercmd;
pub mod view;
pub mod voice;
pub mod write;

pub use client::Client;
pub use delta::{ClientState2, EntityState2, HudElem2, Objective2, PlayerState2};
pub use parse::{Gamestate2, Parsed, Parser, Snapshot2};
pub use protocol::{Cod2Wire, PROTOCOL_118};
pub use usercmd::UserCmd2;
pub use view::{ConfigStrings, EntityView, Frame, NetView, PlayerView, SnapshotView};

pub use vcod_common::text::latin1;
