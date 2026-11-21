//! The engine state both servers embed: what retail's `svs` and `sv` hold
//! that no game module reads by its own layout. `Server` (CoD1) and
//! `Cod2Server` are each an `Engine` with one as its `core`, `C` being the
//! game's client slot; both drive it through `engine::tick`
//! (docs/dev/server-tick.md, "One skeleton for every game").

use std::cell::RefCell;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::rc::Rc;
use vcod_common::net::huffman::Huffman;
use vcod_common::net::wire::Transport;

pub struct EngineCore<C> {
    /// The wire's transport: version, netchan framing, codec, paks. CoD1
    /// keeps it in step with its `Cod1Wire` (`Server::set_protocol`).
    pub transport: &'static Transport,
    pub huff: Huffman,
    /// The datagrams this frame or packet produced, drained by the binary.
    pub outbox: Vec<(SocketAddr, Vec<u8>)>,
    /// The process's `rand()` stream: the challenges, the checksum feed
    /// and every draw the game borrows. CoD1's is glibc's, shared with its
    /// game host (map-cycle doc 4.8.1); CoD2's and CoD4's engines keep
    /// vcod's xorshift until a level's own stream is up.
    pub rng: LibcRand,
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
    /// The `getinfo`, `getstatus` and `getchallenge` limiter.
    pub limiter: super::ratelimit::RateLimiter,
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
    /// The panics the boundaries caught (`engine::contain`).
    pub panics: super::contain::Panics,
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
            rng: LibcRand::xorshift(seed),
            checksum_feed: 0,
            configstrings: Vec::new(),
            sent_configstrings: Vec::new(),
            clients: (0..max_clients).map(|_| None).collect(),
            console: VecDeque::new(),
            challenges: Default::default(),
            limiter: Default::default(),
            bans: Default::default(),
            temp_bans: Default::default(),
            masters,
            resolver: crate::master::resolve,
            rcon: Default::default(),
            sv_time_ms: 0,
            quit: false,
            panics: Default::default(),
        }
    }
}

/// One `rand()` draw off `rng`: xorshift64*'s top 31 bits, 0..2^31 like
/// glibc's, so `(rand() << 16) ^ rand()` wraps into the sign bit and the
/// challenges come out signed like retail's.
pub fn rand_from(rng: &mut u64) -> i32 {
    (vcod_common::rng::xorshift(rng) >> 33) as i32
}

/// A libc `rand()` stream as a handle: [`LibcRand::share`] hands out the
/// same stream, so the engine and a game host draw in one sequence, as
/// retail's game module calls its engine's libc. CoD1's is glibc's
/// (`vcod_common::rng::GlibcRand`); the xorshift variant is vcod's own
/// stream for the games and tests that keep it, with the draws it always
/// gave.
#[derive(Debug)]
pub struct LibcRand(Rc<RefCell<Stream>>);

#[derive(Clone, Debug, PartialEq)]
enum Stream {
    Glibc(vcod_common::rng::GlibcRand),
    Xorshift(u64),
}

impl LibcRand {
    /// glibc's generator after `srand(seed)`.
    pub fn glibc(seed: u32) -> Self {
        Self::of(Stream::Glibc(vcod_common::rng::GlibcRand::new(seed)))
    }

    /// vcod's xorshift64* from `state`, which must not be 0 (the state
    /// xorshift never leaves).
    pub fn xorshift(state: u64) -> Self {
        Self::of(Stream::Xorshift(state))
    }

    fn of(s: Stream) -> Self {
        LibcRand(Rc::new(RefCell::new(s)))
    }

    /// Another handle on the same stream.
    pub fn share(&self) -> Self {
        LibcRand(Rc::clone(&self.0))
    }

    /// A new stream that draws what this one would from here on.
    pub fn fork(&self) -> Self {
        Self::of(self.0.borrow().clone())
    }

    /// `rand()`: `0..2^31`.
    pub fn rand(&mut self) -> i32 {
        match &mut *self.0.borrow_mut() {
            Stream::Glibc(g) => g.next_u31() as i32,
            Stream::Xorshift(s) => rand_from(s),
        }
    }

    /// A uniform draw in `[0, 1)`: one `rand()` over 2^31 on glibc's
    /// stream, the whole xorshift word on vcod's
    /// ([`crate::game::host::rand_unit`]).
    pub fn unit(&mut self) -> f32 {
        match &mut *self.0.borrow_mut() {
            Stream::Glibc(g) => {
                let u = f64::from(g.next_u31()) / 2_147_483_648.0;
                // Above 2^31 - 64 the quotient rounds to 1 as f32.
                (u as f32).min(f32::from_bits(0x3f7f_ffff))
            }
            Stream::Xorshift(s) => crate::game::host::rand_unit(s),
        }
    }

    /// `srand(seed)`: glibc's takes the low 32 bits; the xorshift stream
    /// restarts at `seed | 1`.
    pub fn srand(&mut self, seed: u64) {
        let mut s = self.0.borrow_mut();
        *s = match &*s {
            Stream::Glibc(_) => Stream::Glibc(vcod_common::rng::GlibcRand::new(seed as u32)),
            Stream::Xorshift(_) => Stream::Xorshift(seed | 1),
        };
    }
}

/// Two handles are equal when their streams would draw alike from here.
impl PartialEq for LibcRand {
    fn eq(&self, other: &Self) -> bool {
        *self.0.borrow() == *other.0.borrow()
    }
}
