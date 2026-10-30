//! CoD4 1.7's wire on top of vcod-common's netchan: the Huffman code, the
//! netfield tables, the delta codec and the server message parse
//! (docs/research/cod4-netfields.md, cod4-handshake.md).

pub mod browser;
pub mod capture;
pub mod clmsg;
pub mod conn;
pub mod constant_cs;
pub mod delta;
pub mod demo;
pub mod download;
pub mod events;
pub mod fields;
pub mod http;
pub mod huffman;
pub mod parse;
pub mod pure;
pub mod stats;
pub mod voice;
pub mod wire;
