//! The engine over one game: [`Engine`] is the state both halves share
//! ([`EngineCore`]) beside the game's own ([`GameModule`]), and [`tick`]
//! is `SV_Frame`'s order, calling into the game at each step.

use super::EngineCore;
use std::net::SocketAddr;
use std::time::Instant;
use vcod_common::net::protocol::WireGame;

/// A server: the engine's state and the game it runs. One game per
/// process, so each game's `Engine` is its own type.
pub struct Engine<G: GameModule> {
    pub core: EngineCore<G::Client>,
    pub game: G,
}

/// What every server answers the same over its core: the binary's loop
/// and the tests drive both games through these.
impl<G: GameModule> Engine<G> {
    /// `SV_Frame` for `frames` owed game frames ([`tick`]).
    pub fn tick_frames(&mut self, now: Instant, frames: u32) {
        tick(self, now, frames);
    }

    /// `SV_PacketEvent` for a packet handled the moment it arrived
    /// ([`packet`]).
    pub fn handle_packet(&mut self, from: SocketAddr, pkt: &[u8], now: Instant) {
        packet(self, from, pkt, Arrival::at(now));
    }

    /// A packet the loop read off the socket ([`GameModule::packet_read`]).
    pub fn packet_read(&mut self, from: SocketAddr, pkt: &[u8], at: Arrival) {
        G::packet_read(self, from, pkt, at);
    }

    /// Queues `line` for the next `Cbuf_Execute`, as `Cbuf_AddText`'s
    /// `EXEC_APPEND` does.
    pub fn push_console(&mut self, line: &str) {
        self.core.console.push_back(line.to_string());
    }

    /// The datagrams queued since the last call.
    pub fn take_outgoing(&mut self) -> Vec<(SocketAddr, Vec<u8>)> {
        std::mem::take(&mut self.core.outbox)
    }

    /// Whether `quit` ran; the binary flushes the outbox and exits.
    pub fn quit_requested(&self) -> bool {
        self.core.quit
    }

    /// Replaces the master-name resolver, so a test can heartbeat without
    /// touching DNS.
    pub fn set_master_resolver(&mut self, resolve: fn(&str) -> Option<SocketAddr>) {
        self.core.resolver = resolve;
    }

    /// `rand()`: the challenges', the checksum feed's and the game's draws.
    pub(crate) fn rand(&mut self) -> i32 {
        super::core::rand_from(&mut self.core.rng)
    }
}

/// When a client's usercmds are thought.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThinkTiming {
    /// Queued as the packet arrives, replayed at the head of the next game
    /// frame on the last frame's `level.time` (CoD1, protocol-1.1.md "What
    /// vcod does").
    InFrame,
    /// `ClientThink` per cmd inside `SV_UserMove`, as the packet arrives
    /// (CoD2, `SV_UserMove` cod2_lnxded 0x8090e34).
    OnArrival,
}

/// What `SV_Frame` runs ahead of the frames, in order, when a frame is
/// owed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeadOrder {
    /// The bots, then `SV_CalcPings` (CoD1).
    BotsPings,
    /// Configstrings 0 and 1 and the mirror, the bots, `SV_CalcPings`
    /// (CoD2 0x80969b0).
    CsBotsPings,
    /// `SV_CalcPings`, then `SV_PreFrame`: the bots, configstrings 0 and 1,
    /// the mirror (CoD4 1.7 0x8177318).
    PingsBotsCs,
}

/// The connectionless queries [`GameModule::status`] answers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusQuery {
    /// `getinfo`, `SVC_Info`.
    Info,
    /// `getstatus`, `SVC_Status`; also what a heartbeat's master asks.
    Status,
}

/// When the loop took a packet: `now`, the tick's moment; `arrived`, when
/// the reader thread read it off the socket; `read`, when retail's event
/// loop would have handled it (`main.rs`'s `handle_queued`).
#[derive(Clone, Copy, Debug)]
pub struct Arrival {
    pub now: Instant,
    pub arrived: Instant,
    pub read: Instant,
}

impl Arrival {
    /// A packet handled the moment it arrived.
    pub fn at(now: Instant) -> Self {
        Arrival {
            now,
            arrived: now,
            read: now,
        }
    }
}

/// What a game hands the engine. The hooks take the whole [`Engine`]: a
/// game's steps read its own state and the core's together. Each hook
/// traces its own step lines where their text is the game's.
pub trait GameModule: Sized {
    /// The game's client slot, [`EngineCore::clients`]' element.
    type Client;
    /// The game's wire descriptor.
    type Wire: WireGame;

    // Ordering points, CoD1's by default.
    const THINK: ThinkTiming = ThinkTiming::InFrame;
    /// What runs ahead of the frames, and whether configstrings 0 and 1
    /// and the dvar mirror's block go out there rather than with the
    /// frame's changes.
    const HEAD: HeadOrder = HeadOrder::BotsPings;
    /// Timeouts, messages (and the archive after them) and the heartbeat
    /// follow every owed frame, not once a `SV_Frame` (CoD4 1.7
    /// `SV_FrameInternal`, cod4-server-frame.md).
    const POST_EACH_FRAME: bool = false;
    /// The frames a `SV_Frame` runs when none is owed: CoD1's runs one
    /// whatever the count, CoD2's none.
    const MIN_FRAMES: u32 = 1;
    /// The last frame's `SV_ArchiveSnapshot` follows the messages (CoD2,
    /// Mac 0x15bf3c); without it the game's snapshot pass archives the
    /// frame it built (CoD1's [`Self::snapshots_end`]).
    const ARCHIVE_AFTER_MESSAGES: bool = false;
    /// `SV_SendClientMessages` sends every due slot's fragment in one pass
    /// and the new messages in a second (CoD4 1.7 0x817afee,
    /// cod4-server-frame.md 4), not each slot's in turn.
    const FRAGMENTS_FIRST: bool = false;

    /// What `SV_SendClientMessages` builds once for the snapshots of a
    /// turn ([`send_messages`]).
    type Snapshots;

    /// The wire the server speaks (one per process).
    fn wire(e: &Engine<Self>) -> &'static Self::Wire;
    /// `SVC_Info` or `SVC_Status`: the reply's text for `challenge`, `None`
    /// for no reply ([`answer_status`] sends it).
    fn status(e: &mut Engine<Self>, query: StatusQuery, challenge: &str) -> Option<String>;

    /// A packet the loop read off the socket: [`packet`], unless the game
    /// wraps it (CoD1 stamps the move acks with the frame sent before the
    /// packet arrived, `Server::ack_stamp`).
    fn packet_read(e: &mut Engine<Self>, from: SocketAddr, pkt: &[u8], at: Arrival) {
        packet(e, from, pkt, at);
    }
    /// Whether `SV_PacketEvent` reads packets at all (`sv_running`; a
    /// CoD1 `killserver` clears it).
    fn accepts_packets(_e: &Engine<Self>) -> bool {
        true
    }
    /// Whether `pkt` is a connectionless packet: its `ff ff ff ff`, and on
    /// CoD1 a command that reads as text (anything else goes to the
    /// netchan).
    fn is_connectionless(pkt: &[u8]) -> bool {
        pkt.starts_with(&[0xff; 4])
    }
    /// `SV_ConnectionlessPacket`: `pkt` is the whole packet, its
    /// `ff ff ff ff` included.
    fn connectionless(e: &mut Engine<Self>, from: SocketAddr, pkt: &[u8], at: Arrival);
    /// A netchan packet: the slot it is from, then
    /// `SV_ExecuteClientMessage`.
    fn client_packet(e: &mut Engine<Self>, from: SocketAddr, pkt: &[u8], at: Arrival);
    /// `Cbuf_Execute`: the queued console lines. The loop runs it ahead
    /// of counting the frames owed (`Com_Frame`).
    fn drain_console(e: &mut Engine<Self>, now: Instant);
    /// What the tty console's tab completes against: the commands, then
    /// the cvars (`Cmd_CommandCompletion`, `Cvar_CommandCompletion`).
    /// Only CoD1 runs that console.
    fn console_names(_e: &Engine<Self>) -> Vec<String> {
        Vec::new()
    }

    /// `svs.time`, the network clock `SV_Frame`'s trace prints: the game
    /// clock unless the game keeps them apart (CoD1 1.5's match timeout).
    fn svs_time_ms(e: &Engine<Self>) -> i32 {
        e.core.sv_time_ms
    }
    /// What the game runs at `SV_Frame`'s head, before anything else, with
    /// the frames owed. False: the server stops here (CoD1's `Cbuf_Execute`
    /// ran a `quit`, or a `killserver` left it without a level).
    fn frame_head(_e: &mut Engine<Self>, _now: Instant, _frames: u32) -> bool {
        true
    }

    /// Under [`HeadOrder::CsBotsPings`] or [`HeadOrder::PingsBotsCs`]: the
    /// engine configstrings' changes.
    fn configstrings_head(_e: &mut Engine<Self>) {}
    /// The bots' cmds, once a `SV_Frame` ahead of the frames.
    fn bots(e: &mut Engine<Self>);
    /// `SV_CalcPings`.
    fn calc_pings(e: &mut Engine<Self>);
    /// How far a frame steps the game clock.
    fn frame_step_ms(e: &mut Engine<Self>) -> i32;
    /// The wall time a frame takes, `1000 / sv_fps`: the frame loop's
    /// slot. CoD1 and CoD2 run at a fixed `sv_fps` 20.
    fn wall_frame_ms(_e: &Engine<Self>) -> i32 {
        50
    }
    /// Under [`ThinkTiming::InFrame`]: the queued cmds, ahead of the frame.
    fn replay_moves(_e: &mut Engine<Self>) {}
    /// `G_RunFrame` and what the engine does for it. False when a
    /// `G_Error` dropped the level: the rest of the `SV_Frame` is skipped.
    fn run_frame(e: &mut Engine<Self>) -> bool;
    /// `SV_ArchiveSnapshot`.
    fn archive(e: &mut Engine<Self>);
    /// What the frame queued for the wire, out to the clients.
    fn frame_out(_e: &mut Engine<Self>) {}
    /// `SV_CheckTimeouts`, the stale zombies freed among them.
    fn check_timeouts(e: &mut Engine<Self>);
    /// [`send_messages`]' head, before the clients' turns.
    fn snapshots_begin(e: &mut Engine<Self>) -> Self::Snapshots;
    /// Whether `slot` takes a turn: a client with a socket, in any state
    /// from zombie to active, whose `nextSnapshotTime` came.
    fn message_due(e: &Engine<Self>, slot: usize) -> bool;
    /// A turn with fragments of a message left: the next one and where it
    /// goes, its pacing set.
    fn next_fragment(e: &mut Engine<Self>, slot: usize) -> Option<(SocketAddr, Vec<u8>)>;
    /// `SV_SendClientSnapshot`: the client's next message, its first
    /// packet into the outbox and its pacing set. It serves every state:
    /// the snapshot and commands for an active client or a zombie, the
    /// download blocks for any but a zombie.
    fn send_client_snapshot(e: &mut Engine<Self>, slot: usize, snaps: &mut Self::Snapshots);
    /// After every client's turn.
    fn snapshots_end(_e: &mut Engine<Self>, _snaps: Self::Snapshots) {}
    /// `SV_MasterHeartbeat`.
    fn heartbeat(e: &mut Engine<Self>);

    /// A level load failed with the level already torn down: the loop
    /// ends the process, as retail's `Com_Error` does.
    fn take_fatal(_e: &mut Engine<Self>) -> Option<anyhow::Error> {
        None
    }
    /// The loop put the tick's outbox on the socket at `at`.
    fn frame_sent(_e: &mut Engine<Self>, _at: Instant) {}
}

/// `SV_PacketEvent`: a connectionless packet or a client's.
pub fn packet<G: GameModule>(e: &mut Engine<G>, from: SocketAddr, pkt: &[u8], at: Arrival) {
    if !G::accepts_packets(e) {
        return;
    }
    if G::is_connectionless(pkt) {
        G::connectionless(e, from, pkt, at);
    } else {
        G::client_packet(e, from, pkt, at);
    }
}

/// A `getinfo` or `getstatus` from `to`: the game's reply, if it has one,
/// as a connectionless packet.
pub fn answer_status<G: GameModule>(
    e: &mut Engine<G>,
    to: SocketAddr,
    query: StatusQuery,
    challenge: &str,
) {
    if let Some(text) = G::status(e, query, challenge) {
        let pkt = G::wire(e).oob_packet(&text);
        e.core.outbox.push((to, pkt));
    }
}

/// `SV_SendClientMessages`: in slot order, each client whose turn came
/// gets the next fragment of the message it is being sent, or a new one.
/// One pass serves every state, zombies and clients not yet in the world
/// on their own slot's turn (1.1d 0x809045c, CoD2 0x809bcce;
/// cod11-server-handshake.md, "The order of a frame's messages").
/// Under [`GameModule::FRAGMENTS_FIRST`] the fragments all go first.
pub fn send_messages<G: GameModule>(e: &mut Engine<G>) {
    let mut snaps = G::snapshots_begin(e);
    let mut marked = Vec::new();
    for slot in 0..e.core.clients.len() {
        if !G::message_due(e, slot) {
            continue;
        }
        if let Some((to, pkt)) = G::next_fragment(e, slot) {
            crate::tick_trace::step!("SV_SendClientMessages fragment slot {slot}");
            e.core.outbox.push((to, pkt));
            continue;
        }
        if G::FRAGMENTS_FIRST {
            marked.push(slot);
            continue;
        }
        crate::tick_trace::step!("SV_SendClientSnapshot slot {slot}");
        G::send_client_snapshot(e, slot, &mut snaps);
    }
    for slot in marked {
        crate::tick_trace::step!("SV_SendClientSnapshot slot {slot}");
        G::send_client_snapshot(e, slot, &mut snaps);
    }
    G::snapshots_end(e, snaps);
}

/// `SV_Frame` for `frames` owed game frames (at least
/// [`GameModule::MIN_FRAMES`]), then one round of timeouts and messages,
/// or one after each frame under [`GameModule::POST_EACH_FRAME`]
/// (AGENTS.md, "The server tick"). `now` is the tick's moment.
pub fn tick<G: GameModule>(e: &mut Engine<G>, now: Instant, frames: u32) {
    crate::tick_trace::step!("SV_Frame frames {frames} svs {}", G::svs_time_ms(e));
    if !G::frame_head(e, now, frames) {
        return;
    }
    let frames = frames.max(G::MIN_FRAMES);
    if frames > 0 {
        let _p = crate::tick_profile::phase("head");
        head(e);
    }
    for k in 0..frames {
        let step = G::frame_step_ms(e);
        e.core.sv_time_ms = e.core.sv_time_ms.wrapping_add(step);
        if G::THINK == ThinkTiming::InFrame {
            G::replay_moves(e);
        }
        let ran = {
            let _p = crate::tick_profile::phase("frame");
            G::run_frame(e)
        };
        if !ran {
            return;
        }
        if G::POST_EACH_FRAME {
            {
                let _p = crate::tick_profile::phase("frame out");
                G::frame_out(e);
            }
            post_frame(e, true);
            continue;
        }
        // Between frames; the last frame's archive follows the messages.
        if k + 1 < frames {
            G::archive(e);
        }
        G::frame_out(e);
    }
    if !G::POST_EACH_FRAME {
        post_frame(e, frames > 0);
    }
}

/// The head's bot pass, timed.
fn bots<G: GameModule>(e: &mut Engine<G>) {
    let _p = crate::tick_profile::phase("head/bots");
    G::bots(e);
}

/// [`GameModule::HEAD`]'s steps.
fn head<G: GameModule>(e: &mut Engine<G>) {
    let cs = |e: &mut Engine<G>| {
        crate::tick_trace::step!("SV_SetConfigstring 0/1/mirror");
        let _p = crate::tick_profile::phase("head/configstrings");
        G::configstrings_head(e);
    };
    let pings = |e: &mut Engine<G>| {
        crate::tick_trace::step!("SV_CalcPings");
        let _p = crate::tick_profile::phase("head/pings");
        G::calc_pings(e);
    };
    match G::HEAD {
        HeadOrder::BotsPings => {
            bots(e);
            pings(e);
        }
        HeadOrder::CsBotsPings => {
            cs(e);
            bots(e);
            pings(e);
        }
        HeadOrder::PingsBotsCs => {
            pings(e);
            bots(e);
            cs(e);
        }
    }
}

/// A `SV_Frame`'s tail (CoD1, CoD2), or each frame's (CoD4 1.7 0x8177324):
/// timeouts, the messages, the archive when a frame ran and the game
/// archives after the messages, the heartbeat.
fn post_frame<G: GameModule>(e: &mut Engine<G>, archive: bool) {
    crate::tick_trace::step!("SV_CheckTimeouts");
    {
        let _p = crate::tick_profile::phase("timeouts");
        G::check_timeouts(e);
    }
    crate::tick_trace::step!("SV_SendClientMessages");
    {
        let _p = crate::tick_profile::phase("messages");
        send_messages(e);
    }
    if archive && G::ARCHIVE_AFTER_MESSAGES {
        let _p = crate::tick_profile::phase("archive");
        G::archive(e);
    }
    crate::tick_trace::step!("SV_MasterHeartbeat");
    let _p = crate::tick_profile::phase("heartbeat");
    G::heartbeat(e);
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_cod2::net::{Cod2Wire, PROTOCOL_118};

    /// Records the steps [`tick`] calls, under CoD4 1.7's ordering points.
    #[derive(Default)]
    struct Steps(Vec<&'static str>);

    impl GameModule for Steps {
        type Client = ();
        type Wire = Cod2Wire;
        type Snapshots = ();
        const THINK: ThinkTiming = ThinkTiming::OnArrival;
        const HEAD: HeadOrder = HeadOrder::PingsBotsCs;
        const POST_EACH_FRAME: bool = true;
        const MIN_FRAMES: u32 = 0;
        const ARCHIVE_AFTER_MESSAGES: bool = true;

        fn wire(_e: &Engine<Self>) -> &'static Cod2Wire {
            &PROTOCOL_118
        }
        fn status(_e: &mut Engine<Self>, _q: StatusQuery, _c: &str) -> Option<String> {
            None
        }
        fn connectionless(_e: &mut Engine<Self>, _f: SocketAddr, _p: &[u8], _a: Arrival) {}
        fn client_packet(_e: &mut Engine<Self>, _f: SocketAddr, _p: &[u8], _a: Arrival) {}
        fn drain_console(_e: &mut Engine<Self>, _now: Instant) {}
        fn configstrings_head(e: &mut Engine<Self>) {
            e.game.0.push("cs");
        }
        fn bots(e: &mut Engine<Self>) {
            e.game.0.push("bots");
        }
        fn calc_pings(e: &mut Engine<Self>) {
            e.game.0.push("pings");
        }
        fn frame_step_ms(_e: &mut Engine<Self>) -> i32 {
            50
        }
        fn run_frame(e: &mut Engine<Self>) -> bool {
            e.game.0.push("frame");
            true
        }
        fn archive(e: &mut Engine<Self>) {
            e.game.0.push("archive");
        }
        fn check_timeouts(e: &mut Engine<Self>) {
            e.game.0.push("timeouts");
        }
        fn snapshots_begin(e: &mut Engine<Self>) {
            e.game.0.push("messages");
        }
        fn message_due(_e: &Engine<Self>, _slot: usize) -> bool {
            false
        }
        fn next_fragment(_e: &mut Engine<Self>, _slot: usize) -> Option<(SocketAddr, Vec<u8>)> {
            None
        }
        fn send_client_snapshot(_e: &mut Engine<Self>, _slot: usize, _s: &mut ()) {}
        fn heartbeat(e: &mut Engine<Self>) {
            e.game.0.push("heartbeat");
        }
    }

    fn engine() -> Engine<Steps> {
        let masters = crate::master::Masters::new(vcod_common::net::master::COD2_MASTER);
        Engine {
            core: EngineCore::new(&PROTOCOL_118.transport, 1, 1, masters),
            game: Steps::default(),
        }
    }

    #[test]
    fn post_each_frame_follows_every_owed_frame_after_a_pings_bots_cs_head() {
        let mut e = engine();
        tick(&mut e, Instant::now(), 2);
        let post = ["timeouts", "messages", "archive", "heartbeat"];
        let mut want = vec!["pings", "bots", "cs"];
        for _ in 0..2 {
            want.push("frame");
            want.extend(post);
        }
        assert_eq!(e.game.0, want);
        assert_eq!(e.core.sv_time_ms, 100);
    }

    #[test]
    fn no_owed_frame_runs_nothing_under_post_each_frame() {
        let mut e = engine();
        tick(&mut e, Instant::now(), 0);
        assert!(e.game.0.is_empty());
    }
}
