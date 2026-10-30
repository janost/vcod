//! Engine housekeeping both servers run, `Server` (CoD1 1.1, 1.5, UO) and
//! `Cod2Server`, and the skeleton a server is (`game`: `Engine<G>`, the
//! tick and packet order over a `GameModule`; both games run on it): a client's link (LAN test, rate, pacing, ping), the frame
//! archive's ring, `map_rotate`'s walk, the challenge table and the bans. Where retail's engines differ
//! the caller passes the game's rule, and the rule's doc names both
//! addresses.

pub mod archive;
pub mod bans;
pub mod challenge;
pub mod core;
pub mod game;
pub mod gamelog;
pub mod link;
pub mod rotation;
pub mod slot;

pub use core::EngineCore;
pub use game::{
    Arrival, Engine, GameModule, HeadOrder, StatusQuery, ThinkTiming, answer_status, packet, tick,
};
pub use link::Lan;
