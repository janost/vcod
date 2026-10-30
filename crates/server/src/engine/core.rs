//! The engine state both servers embed: what retail's `svs` and `sv` hold
//! that no game module reads by its own layout. `Server` (CoD1) and
//! `Cod2Server` are each an `Engine` with one as its `core`, `C` being the
//! game's client slot; both drive it through `engine::tick` (AGENTS.md,
//! "The server tick").

use std::collections::VecDeque;
use std::net::SocketAddr;
use vcod_common::net::huffman::Huffman;
use vcod_common::net::wire::Transport;

pub struct EngineCore<C> {
    /// The wire's transport: version, netchan framing, codec, paks. CoD1
    /// keeps it in step with its `Protocol` (`Server::set_protocol`).
    pub transport: &'static Transport,
    pub huff: Huffman,
    /// The datagrams this frame or packet produced, drained by the binary.
    pub outbox: Vec<(SocketAddr, Vec<u8>)>,
    /// The process's `rand()` stream: the challenges, the checksum feed
    /// and every draw the game borrows.
    pub rng: u64,
    /// `checksumFeed`, drawn at each spawn and sent in the gamestate.
    pub checksum_feed: i32,
    /// The configstring table: the game's writes as the engine sends them.
    pub configstrings: Vec<String>,
    /// The table as the clients last heard it, which is what a mid-level
    /// change is diffed against. Re-synced wherever a client is handed the
    /// whole table again.
    pub sent_configstrings: Vec<String>,
    /// The client slots, `sv_maxclients` of them; `None` is `CS_FREE`.
    pub clients: Vec<Option<C>>,
    /// Console lines waiting for `Cbuf_Execute`, drained by each server's
    /// `drain_console` (docs/research/cod11-map-cycle.md section 5.2).
    pub console: VecDeque<String>,
    pub challenges: super::challenge::Challenges,
    /// `ban.txt` and the stand-ins for the authorize server's list.
    pub bans: super::bans::BanFile,
    /// The temporary bans, stamped on each game's clock (1.5's `tempBan*`
    /// table, 0x83ccd84, on the game clock).
    pub temp_bans: super::bans::TempBans,
    /// The heartbeat's masters; each game starts from its own list.
    pub masters: crate::master::Masters,
    /// `NET_StringToAdr` for the masters; a test swaps in its own.
    pub resolver: fn(&str) -> Option<SocketAddr>,
    /// `rconPassword`'s attempts limiter.
    pub rcon: crate::rcon::Rcon,
    /// The game clock, `level.time` and every snapshot's `serverTime`. CoD1
    /// 1.1 and CoD2 step it one frame a tick; 1.5 advances it by
    /// `timescale` and keeps `svs.time` apart (`Server::svs_time_ms`).
    pub sv_time_ms: i32,
    /// A `quit` ran: the flatline is in the outbox and the binary exits once
    /// it has flushed it.
    pub quit: bool,
}

impl<C> EngineCore<C> {
    pub fn new(
        transport: &'static Transport,
        max_clients: usize,
        seed: u64,
        masters: crate::master::Masters,
    ) -> Self {
        EngineCore {
            transport,
            huff: Huffman::new(),
            outbox: Vec::new(),
            rng: seed,
            checksum_feed: 0,
            configstrings: Vec::new(),
            sent_configstrings: Vec::new(),
            clients: (0..max_clients).map(|_| None).collect(),
            console: VecDeque::new(),
            challenges: Default::default(),
            bans: Default::default(),
            temp_bans: Default::default(),
            masters,
            resolver: crate::master::resolve,
            rcon: Default::default(),
            sv_time_ms: 0,
            quit: false,
        }
    }
}

/// One `rand()` draw off `rng`: xorshift64*'s top 31 bits, 0..2^31 like
/// glibc's, so `(rand() << 16) ^ rand()` wraps into the sign bit and the
/// challenges come out signed like retail's.
pub fn rand_from(rng: &mut u64) -> i32 {
    (vcod_common::rng::xorshift(rng) >> 33) as i32
}
