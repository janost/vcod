//! What CoD2's and CoD4's servers do alike between a datagram and the
//! level, over their [`Conn`] slots: the netchan's packet acceptance, the
//! command sequence, the reliable ring, the message pacing and its
//! fragments, the snapshot's delta, the script's commands out to their
//! clients, the console queue, `getinfo`, `getstatus`, rcon, `connect` up
//! to `ClientConnect`, the console arms both share and the voice queue.
//! Both retail engines run these routines line for line alike at
//! their own addresses (cod2-protocol.md 2, 3.4, 17; cod4-handshake.md 3,
//! 15); what differs is a [`ConnGame`] hook. CoD1's server keeps its own.

use std::net::SocketAddr;
use std::time::Instant;

use vcod_common::net::codec::MsgWriter;
use vcod_common::net::netchan::ClientMessage;
use vcod_common::net::protocol::{SnapshotHead, WireGame};

use super::link;
use super::slot::{Conn, PACKET_BACKUP, State};
use super::{Engine, EngineCore, GameModule};
use crate::game::script::ScriptRuntime;
use crate::rcon::{Redirect, Verdict};

/// A game whose slots are [`Conn`]s: what the shared routines ask of it.
pub trait ConnGame: GameModule<Client = Conn<Self::Frame, Self::Extra>> {
    /// The frame a slot's ring files by message number.
    type Frame;
    /// The game's half of a slot.
    type Extra;

    /// An engine dvar's value.
    fn cvar<'a>(e: &'a Engine<Self>, name: &str) -> &'a str;
    /// `type 0` of `SV_AddServerCommand`, read off a command's letter.
    fn ignorable(text: &str) -> bool;
    /// Whether `SV_AddServerCommand` adds to this client's ring.
    fn takes_commands(_c: &Self::Client) -> bool {
        true
    }
    /// Whether a download is in flight: such a client is not idle.
    fn downloading(c: &Self::Client) -> bool;
    /// `SV_DropClient`.
    fn drop_client(e: &mut Engine<Self>, slot: usize, reason: &str);
    /// The running level's script, `None` on a bare server.
    fn script(e: &mut Engine<Self>) -> Option<&mut ScriptRuntime>;
    /// `SV_SetConfigstring` of a game write.
    fn write_configstring(e: &mut Engine<Self>, i: usize, value: &str);
    /// After [`Engine::flush_game_commands`] took the level's.
    fn flushed(_e: &mut Engine<Self>) {}
    /// One `svc_serverCommand`.
    fn write_server_command(w: &mut MsgWriter, seq: i32, text: &str);
    /// The message through the netchan: its packets, the first one first.
    fn netchan_out(e: &mut Engine<Self>, slot: usize, ops: Vec<u8>) -> Vec<Vec<u8>>;
    /// A pacing of the game's own ahead of the rate's, for the message
    /// that went out as `sequence`; true when it set `nextSnapshotTime`.
    fn back_off(_e: &mut Engine<Self>, _slot: usize, _sequence: i32) -> bool {
        false
    }
    /// One console line.
    fn exec(e: &mut Engine<Self>, line: &str, out: &mut Redirect);

    /// `PROTOCOL_VERSION`, and the version a refused `connect` names.
    const PROTOCOL: i32;
    const VERSION: &'static str;
    /// The dvars `getinfo`'s `ff` and `kc` read.
    const FF_KC: [&'static str; 2];
    /// The host name, map and gametype `getinfo` names.
    fn names(e: &Engine<Self>) -> [&str; 3];
    /// `sv_pure` on.
    fn pure(e: &Engine<Self>) -> bool;
    /// `getinfo`'s and `getstatus`' `mod`.
    fn modded(e: &Engine<Self>) -> bool;
    /// `getinfo`'s `sv_maxclients`.
    fn shown_slots(e: &Engine<Self>) -> usize {
        e.core.clients.len()
    }
    /// The score of `getstatus`' line for `slot`.
    fn status_score(_e: &mut Engine<Self>, _slot: usize) -> i32 {
        0
    }
    /// The name of `getstatus`' line.
    fn status_name(c: &Self::Client) -> String;
    /// Whether rcon's output goes back (CoD4's `killserver` ends it).
    fn answers(_e: &Engine<Self>) -> bool {
        true
    }
    /// Where a new client's slot search starts (`sv_privateClients`).
    fn first_slot(_e: &Engine<Self>, _password: &str) -> usize {
        0
    }
    /// `ClientConnect`'s `g_password` refusal.
    fn password_refused(e: &Engine<Self>, password: &str) -> bool;
    /// `SV_FreeClient`'s game half: `ClientDisconnect` for `slot`.
    fn free_level_slot(e: &mut Engine<Self>, slot: usize);
    /// `SV_GetPlayerByName` or `SV_GetPlayerByNum`; a miss prints why.
    fn find_player(e: &Engine<Self>, by_name: bool, who: &str, out: &mut Redirect)
    -> Option<usize>;
    /// `SV_BanClient`.
    fn ban_client(e: &mut Engine<Self>, slot: usize, out: &mut Redirect);
}

/// How long a live slot must have been silent before a connect with
/// another challenge may take it ([`Engine::connect_slot`]).
const RECONNECT_SILENCE_MS: i32 = 3000;

/// A `connect` [`Engine::connect_slot`] let into a slot, for the game's
/// `ClientConnect`.
pub struct Admit {
    pub slot: usize,
    pub qport: u16,
    pub challenge: i32,
    /// The challenge slot, `taken` once the client is in.
    pub ci: usize,
}

/// `Cmd_Args` past the command as `SV_ConSay_f` and `SV_ConTell_f` read
/// it: the arguments joined by one space, a leading quote and the last
/// character cut (cod4-handshake.md 14).
pub fn console_args(args: &[String]) -> String {
    let text = args.join(" ");
    match text.strip_prefix('"') {
        Some(rest) => {
            let mut rest = rest.to_string();
            rest.pop();
            rest
        }
        None => text,
    }
}

/// What [`incoming`] made of a netchan packet.
pub enum Incoming {
    /// No slot holds the address and qport.
    NoSlot,
    /// Short, out of order, a fragment, or acknowledging what was never
    /// sent: dropped, nothing kept.
    Dropped,
    /// The slot's next message, not yet committed ([`commit`]).
    Message(usize, ClientMessage),
}

/// The slot holding `from`'s address and `qport`.
pub fn slot_of<F, X>(core: &EngineCore<Conn<F, X>>, from: SocketAddr, qport: u16) -> Option<usize> {
    core.clients.iter().position(|c| {
        c.as_ref()
            .is_some_and(|c| c.link.addr.ip() == from.ip() && c.link.netchan.qport == qport)
    })
}

/// The slot a netchan packet from `from` is for, by its qport.
pub fn sender_slot<F, X>(
    core: &EngineCore<Conn<F, X>>,
    from: SocketAddr,
    pkt: &[u8],
) -> Option<usize> {
    let q = pkt.get(4..6)?;
    slot_of(core, from, u16::from_le_bytes([q[0], q[1]]))
}

/// `SV_PacketEvent`'s slot by address and qport and `Netchan_Process`,
/// the acknowledges checked: a negative `messageAcknowledge` is dropped
/// (cod2-protocol.md 2.2). So is one of a message never sent, and a
/// `reliableAcknowledge` past what was sent or so far behind that its key
/// is gone, which only a forgery or garbage carries (vcod's, as CoD1's
/// `handle_client_packet`). Nothing is committed yet: a message of the
/// running level is read to its end first ([`commit`]), so a forged one
/// moves neither the sequence nor the address.
pub fn incoming<F, X>(core: &EngineCore<Conn<F, X>>, from: SocketAddr, pkt: &[u8]) -> Incoming {
    if pkt.len() < 6 {
        return Incoming::Dropped;
    }
    let qport = u16::from_le_bytes([pkt[4], pkt[5]]);
    let Some(slot) = slot_of(core, from, qport) else {
        return Incoming::NoSlot;
    };
    let c = core.clients[slot].as_ref().unwrap();
    let Some(m) = c.link.netchan.process_in(pkt) else {
        return Incoming::Dropped;
    };
    let sent = i64::from(c.link.netchan.outgoing_sequence);
    if m.message_ack < 0
        || i64::from(m.message_ack) >= sent
        || !c.link.reliable_ack_in_window(m.reliable_ack)
    {
        return Incoming::Dropped;
    }
    Incoming::Message(slot, m)
}

/// `Netchan_Process`'s commit: the sequence, the address the client now
/// sends from (NAT may move its port; the qport is its identity) and what
/// it has seen of the server's commands.
pub fn commit<F, X>(c: &mut Conn<F, X>, from: SocketAddr, m: &ClientMessage) {
    c.link.netchan.accept(m);
    c.link.addr = from;
    c.link.reliable_ack = m.reliable_ack;
}

/// Whether the last message sent a slot is its gamestate: a `donedl` that
/// asks for another before any message went out between is a flood, each
/// one a whole gamestate built (vcod's limit, retail sends one a
/// `donedl`).
pub fn gamestate_last<F, X>(c: &Conn<F, X>) -> bool {
    c.gamestate_message >= 0
        && i64::from(c.gamestate_message) == i64::from(c.link.netchan.outgoing_sequence) - 1
}

/// What [`Engine::take_client_command`] made of a `clc_clientCommand`'s
/// sequence.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CommandSeq {
    /// Already run: skipped.
    Old,
    /// One went missing: the client is dropped.
    Lost,
    /// The next one, recorded: run it.
    Next,
}

/// `sv_serverid` after `SV_SpawnServer` (cod2_lnxded 0x8093520, cod4_lnxded
/// 0x81748da): the high nibble steps, and steps again past a wrap to 0,
/// the low nibble kept. VERIFIED (CoD4's first spawn's 16 live).
pub fn spawn_server_id(id: i32) -> i32 {
    let mut b = (id as u8).wrapping_add(0x10);
    if b & 0xf0 == 0 {
        b = b.wrapping_add(0x10);
    }
    i32::from(b)
}

/// `sv_serverid` after a `fast_restart` (`SV_MapRestart`, cod2_lnxded
/// 0x808be3e, cod4_lnxded 0x816c466): the low nibble steps, the high one
/// kept. VERIFIED.
pub fn restart_server_id(id: i32) -> i32 {
    ((id + 1) & 0xf) + (id & 0xf0)
}

/// A frame older than this many messages is not deltaed from
/// (cod2-protocol.md 3.3).
pub const MAX_DELTA_AGE: i32 = PACKET_BACKUP as i32 - 3;

/// `SV_WriteSnapshotToClient` (cod2_lnxded 0x80985c8): `to` deltaed from
/// the frame the client last acknowledged, while that is past the
/// gamestate and young enough, else whole; the snapshot flags the
/// server's count bit, 1 while the rate holds messages back and 2 for a
/// zombie never sent one active; then `to` filed under its message.
pub fn write_snapshot<W: WireGame, X>(
    wire: &W,
    w: &mut MsgWriter,
    c: &mut Conn<(i32, W::Frame), X>,
    to: W::Frame,
    server_flags: u8,
    baselines: &W::Baselines,
) {
    let message = c.link.netchan.outgoing_sequence as i32;
    let from = (c.state == State::Active
        && c.delta_message > 0
        && c.delta_message > c.gamestate_message
        && message - c.delta_message < MAX_DELTA_AGE)
        .then(|| {
            c.link.frames[c.delta_message as usize % PACKET_BACKUP]
                .as_ref()
                .filter(|(n, _)| *n == c.delta_message)
                .map(|(_, f)| f)
        })
        .flatten();
    if c.state == State::Active {
        c.link.was_active = true;
    }
    let flags =
        server_flags | u8::from(c.link.rate_delayed) | if c.link.was_active { 0 } else { 2 };
    let head = SnapshotHead {
        delta_num: from.map_or(0, |_| (message - c.delta_message) as u8),
        snap_flags: u32::from(flags),
    };
    wire.write_snapshot(w, from, &to, head, baselines);
    c.link.frames[message as usize % PACKET_BACKUP] = Some((message, to));
}

/// The unacknowledged server commands, each `svc_serverCommand`; every
/// one of them now sent.
pub fn write_pending_commands<G: ConnGame>(w: &mut MsgWriter, c: &mut G::Client) {
    c.link.reliable_sent = c.link.netchan.reliable_sequence as i32;
    let mask = c.link.netchan.reliable.len() - 1;
    for seq in c.link.reliable_ack + 1..=c.link.netchan.reliable_sequence as i32 {
        G::write_server_command(w, seq, &c.link.netchan.reliable[seq as usize & mask]);
    }
}

impl<G: ConnGame> Engine<G> {
    /// An engine dvar as `atoi` reads it.
    pub(crate) fn int(&self, name: &str) -> i32 {
        crate::game::builtins::cvar::atoi(G::cvar(self, name))
    }

    /// Whether `from`'s address holds its share of slots
    /// ([`link::per_ip_full`]); bots and zombies hold none. A refused
    /// address is told so.
    pub(crate) fn refuse_per_ip(&mut self, from: SocketAddr, exact: bool) -> bool {
        let held = self.core.clients.iter().flatten();
        let held = held.filter(|c| !c.bot && c.state > State::Zombie);
        let cap = self.int(link::PER_IP_CVAR);
        if !link::per_ip_full(cap, from, exact, held.map(|c| c.link.addr)) {
            return false;
        }
        log::info!("{from}: refused, its address holds {cap} slots");
        self.send_oob(from, link::PER_IP_REFUSAL);
        true
    }

    /// `NET_OutOfBandPrint`.
    pub(crate) fn send_oob(&mut self, to: SocketAddr, text: &str) {
        let pkt = G::wire(self).oob_packet(text);
        self.core.outbox.push((to, pkt));
    }

    /// The slots past `CS_ZOMBIE`.
    pub fn client_count(&self) -> usize {
        self.core
            .clients
            .iter()
            .flatten()
            .filter(|c| c.state >= State::Connected)
            .count()
    }

    /// `SV_ClientCommand`'s sequence: one already run is skipped, one past
    /// the next drops the client (`EXE_LOSTRELIABLECOMMANDS`), the next is
    /// recorded as the last, its text the netchan's key.
    pub(crate) fn take_client_command(&mut self, slot: usize, seq: i32, text: &str) -> CommandSeq {
        let Some(c) = self.core.clients[slot].as_mut() else {
            return CommandSeq::Lost;
        };
        if seq <= c.link.last_client_command {
            return CommandSeq::Old;
        }
        if seq > c.link.last_client_command.wrapping_add(1) {
            G::drop_client(self, slot, "EXE_LOSTRELIABLECOMMANDS");
            return CommandSeq::Lost;
        }
        c.link.last_client_command = seq;
        c.link.netchan.last_client_command_string = text.to_string();
        CommandSeq::Next
    }

    /// `SV_AddServerCommand` for one game or engine command, its type read
    /// off its letter ([`ConnGame::ignorable`]); a full ring drops the
    /// client.
    pub fn send_server_command(&mut self, slot: usize, text: &str) {
        let Some(c) = self.core.clients.get_mut(slot).and_then(Option::as_mut) else {
            return;
        };
        if !G::takes_commands(c) {
            return;
        }
        log::trace!("to client {slot}: {text:?}");
        let active = c.state == State::Active;
        if !super::commands::add(
            &mut c.link,
            &mut c.reliable_kept,
            active,
            text,
            !G::ignorable(text),
        ) {
            G::drop_client(self, slot, "EXE_SERVERCOMMANDOVERFLOW");
        }
    }

    /// `SV_GameSendServerCommand`: `cmd` to one client or to every one.
    pub(crate) fn send_to(&mut self, to: crate::game::commands::Recipient, cmd: &str) {
        match to {
            crate::game::commands::Recipient::All => self.send_to_all(cmd),
            crate::game::commands::Recipient::One(slot) => {
                self.send_server_command(slot.index(), cmd)
            }
        }
    }

    /// `SV_SendServerCommand(NULL, ..)`: every client past `CS_CONNECTED`,
    /// zombies left out.
    pub(crate) fn send_to_all(&mut self, cmd: &str) {
        for slot in 0..self.core.clients.len() {
            if self.core.clients[slot]
                .as_ref()
                .is_some_and(|c| c.state > State::Connected)
            {
                self.send_server_command(slot, cmd);
            }
        }
    }

    /// The configstrings the script wrote and the server commands and
    /// console lines it queued, out to their clients in the order they
    /// were written, as retail's `SV_SetConfigstring` and
    /// `SV_GameSendServerCommand` send each at once (cod2-map-cycle.md 3:
    /// the intermission's `f` lines lead its `d 5` / `d 6`). The dvar
    /// mirror's changes wait for the next `SV_Frame`'s head.
    pub(crate) fn flush_game_commands(&mut self) {
        let Some(rt) = G::script(self) else {
            return;
        };
        let writes = rt.host.configstrings.take_writes_stamped();
        let commands = rt.take_client_commands_stamped();
        let console = rt.take_console();
        let mut writes = writes.into_iter().peekable();
        for (seq, to, cmd) in commands {
            while let Some((_, i, v)) = writes.next_if(|w| w.0 < seq) {
                G::write_configstring(self, i, &v);
            }
            self.send_to(to, &cmd);
        }
        for (_, i, v) in writes {
            G::write_configstring(self, i, &v);
        }
        self.core.console.extend(console);
        G::flushed(self);
    }

    /// `SV_SendMessageToClient` (cod2_lnxded 0x809aba2, cod4_lnxded
    /// 0x8178b3e): the message through the netchan, its first packet now
    /// and any other one a turn ([`super::send_messages`]), then
    /// `nextSnapshotTime`: the next frame for a LAN client, else what the
    /// rate needs and at least the snapshot interval, a second on for one
    /// neither in the world nor downloading (cod2-protocol.md 17.2,
    /// cod4-handshake.md 15). CoD4's loopback walker was sent its 359-byte
    /// spawn frame and the next snapshot 50 ms apart at `sv_maxRate` 5000,
    /// and the gamestate's fragments 350 ms apart (VERIFIED live,
    /// cod4-pmove.md 15, cod4-handshake.md 3).
    pub(crate) fn transmit(&mut self, slot: usize, ops: Vec<u8>) {
        let svs = self.core.sv_time_ms;
        let max_rate = self.int("sv_maxRate");
        let Some(c) = self.core.clients[slot].as_ref() else {
            return;
        };
        let sequence = c.link.netchan.outgoing_sequence as i32;
        let mut pkts = G::netchan_out(self, slot, ops);
        // The compressed message with its acknowledge: one packet less its
        // sequence, or each fragment less its 10-byte header.
        let bytes = match pkts.as_slice() {
            [one] => one.len() - 4,
            many => many.iter().map(|p| p.len() - 10).sum(),
        };
        if !G::back_off(self, slot, sequence) {
            let c = self.core.clients[slot].as_mut().unwrap();
            let idle = c.state != State::Active && !G::downloading(c);
            match link::pace(
                svs,
                bytes,
                c.lan,
                c.rates.send_rate(max_rate),
                c.rates.snapshot_msec,
                idle,
            ) {
                link::Pace::Lan => c.link.next_message_ms = svs - 1,
                link::Pace::At { next, delayed } => {
                    c.link.next_message_ms = next;
                    c.link.rate_delayed = delayed;
                }
            }
        }
        let c = self.core.clients[slot].as_mut().unwrap();
        let to = c.link.addr;
        if !pkts.is_empty() {
            self.core.outbox.push((to, pkts.remove(0)));
        }
        c.link.unsent = pkts.into();
    }

    /// `SV_SendClientGameState`'s tail: the message the gamestate's `ops`
    /// go in marked, the client `CS_PRIMED` with no delta base and no pure
    /// verdict, the message sent.
    pub(crate) fn transmit_gamestate(&mut self, slot: usize, ops: Vec<u8>) {
        let c = self.core.clients[slot].as_mut().unwrap();
        c.gamestate_message = c.link.netchan.outgoing_sequence as i32;
        c.state = State::Primed;
        c.delta_message = -1;
        c.pure = None;
        log::info!("client {slot} gamestate, message {}", c.gamestate_message);
        self.transmit(slot, ops);
    }

    /// Whether `slot` takes a turn in `SV_SendClientMessages`
    /// ([`super::send_messages`]): a client with a socket whose
    /// `nextSnapshotTime` came.
    pub(crate) fn message_due(&self, slot: usize) -> bool {
        let svs = self.core.sv_time_ms;
        self.core.clients[slot]
            .as_ref()
            .is_some_and(|c| !c.bot && c.link.next_message_ms <= svs)
    }

    /// The turn's next fragment, paced on the bytes left.
    pub(crate) fn next_fragment(&mut self, slot: usize) -> Option<(SocketAddr, Vec<u8>)> {
        let svs = self.core.sv_time_ms;
        let max_rate = self.int("sv_maxRate");
        let c = self.core.clients[slot].as_mut()?;
        let pkt = c.link.unsent.pop_front()?;
        let left = pkt.len() - 10 + c.link.unsent.iter().map(|p| p.len() - 10).sum::<usize>();
        c.link.next_message_ms = svs + link::rate_msec(left, c.rates.send_rate(max_rate));
        Some((c.link.addr, pkt))
    }

    /// `SV_PreGameUserVoice` (cod2_lnxded 0x809c3c6, cod4_lnxded
    /// 0x817bc58): every other client past `CS_ZOMBIE` that has not muted
    /// the talker and wants voice, no game rule asked.
    pub(crate) fn pre_game_voice(&mut self, talker: usize, frame: &[u8]) {
        for (n, c) in self.core.clients.iter_mut().enumerate() {
            let Some(c) = c.as_mut() else { continue };
            if n != talker && c.state > State::Zombie && !c.voice.muted[talker] && c.voice.send {
                c.voice.queue(talker, frame);
            }
        }
    }

    /// `G_BroadcastVoice`'s loop: `frame` queued for each client the
    /// level's rules let hear `talker` (`voicers` by slot).
    pub(crate) fn queue_voice(
        &mut self,
        talker: usize,
        frame: &[u8],
        rules: &crate::voice::Rules,
        voicers: &[Option<crate::voice::Voicer>],
    ) {
        let Some(t) = voicers.get(talker).copied().flatten() else {
            return;
        };
        for (i, l) in voicers.iter().enumerate() {
            if let Some(l) = l
                && crate::voice::hears(rules, &t, l, i == talker)
            {
                self.core.clients[i]
                    .as_mut()
                    .unwrap()
                    .voice
                    .queue(talker, frame);
            }
        }
    }

    /// `Cbuf_Execute` over the queued console lines (`quit` from Ctrl-C),
    /// each split on `;` as `Cbuf_ExecuteText` does (a passed `g_gametype`
    /// vote queues `g_gametype tdm; map_restart`), their prints logged.
    pub(crate) fn run_console(&mut self) {
        let lines: Vec<String> = std::mem::take(&mut self.core.console)
            .iter()
            .flat_map(|l| crate::console::split_commands(l))
            .collect();
        for line in lines {
            let mut out = Redirect::default();
            G::exec(self, &line, &mut out);
            for p in out.finish() {
                let text = p.trim_start_matches("print\n");
                if !text.is_empty() {
                    log::info!("{}", text.trim_end());
                }
            }
        }
    }

    /// `SVC_Info`, its keys in their order (cod2-protocol.md 1.4,
    /// cod4-handshake.md 2; the two engines' are one routine).
    pub(crate) fn svc_info(&mut self, challenge: &str) -> String {
        let mut i = vcod_common::net::connectionless::Info::new();
        i.set("challenge", challenge);
        i.set("protocol", G::PROTOCOL);
        let [hostname, mapname, gametype] = G::names(self);
        i.set("hostname", hostname);
        i.set("mapname", mapname);
        let clients = self.client_count();
        if clients > 0 {
            i.set("clients", clients);
        }
        let shown = G::shown_slots(self);
        if shown > 0 {
            i.set("sv_maxclients", shown);
        }
        i.set("gametype", gametype);
        if G::pure(self) {
            i.set("pure", 1);
        }
        let set_on = |i: &mut vcod_common::net::connectionless::Info, key: &str, v: i32| {
            if v != 0 {
                i.set(key, v);
            }
        };
        set_on(&mut i, "minPing", self.int("sv_minPing"));
        set_on(&mut i, "maxPing", self.int("sv_maxPing"));
        i.set("game", G::cvar(self, "fs_game"));
        set_on(&mut i, "sv_allowAnonymous", self.int("sv_allowAnonymous"));
        set_on(&mut i, "con_disabled", self.int("sv_disableClientConsole"));
        if !G::cvar(self, "g_password").is_empty() {
            i.set("pswrd", 1);
        }
        set_on(&mut i, "ff", self.int(G::FF_KC[0]));
        set_on(&mut i, "kc", self.int(G::FF_KC[1]));
        i.set("hw", 1);
        i.set("mod", u8::from(G::modded(self)));
        i.set("voice", self.int("sv_voice"));
        i.set("pb", self.int("sv_punkbuster"));
        format!("infoResponse\n{i}")
    }

    /// `SVC_Status`: the serverinfo with `challenge`, `pswrd` and `mod`
    /// appended, then a `score ping "name"` line for each client past
    /// `CS_ZOMBIE` (cod2-protocol.md 1.5, cod4-handshake.md 14).
    pub(crate) fn svc_status(&mut self, challenge: &str) -> String {
        let mut info = self.core.configstrings[0].clone();
        // `Info_SetValueForKey` drops a value's `\`, `;` and `"`, so a
        // challenge cannot add keys (cod2-protocol.md 1.5).
        let mut add = |k: &str, v: String| {
            let v = vcod_common::net::connectionless::info_value(v);
            if !v.is_empty() {
                info.push_str(&format!("\\{k}\\{v}"));
            }
        };
        add("challenge", challenge.to_string());
        add(
            "pswrd",
            u8::from(!G::cvar(self, "g_password").is_empty()).to_string(),
        );
        add("mod", u8::from(G::modded(self)).to_string());
        let mut text = format!("statusResponse\n{info}\n");
        for slot in 0..self.core.clients.len() {
            let score = G::status_score(self, slot);
            let Some(c) = self.core.clients[slot].as_ref() else {
                continue;
            };
            if c.state >= State::Connected {
                let name = G::status_name(c);
                text.push_str(&format!("{score} {} \"{name}\"\n", c.link.ping));
            }
        }
        text
    }

    /// `SVC_RemoteCommand`: `rcon_password`, 1.1's 500 ms window, the
    /// `Rcon from` or `Bad rcon from` log line with the third token, 1.7's
    /// refusals, the output back as `print` packets (cod2-protocol.md 1.6,
    /// cod4-handshake.md 14.2).
    pub(crate) fn remote_command(&mut self, from: SocketAddr, args: &[String], now: Instant) {
        let password = G::cvar(self, "rcon_password").to_string();
        let mut out = Redirect::default();
        let verdict = self.core.rcon.check(&password, args, from.ip(), now);
        if let Some(line) = crate::rcon::log_line(&verdict, from, args) {
            log::info!("{line}");
        }
        match verdict {
            Verdict::Limited => return,
            Verdict::NoPassword | Verdict::BadPassword => {
                out.print(verdict.login_refusal(args).unwrap_or_default())
            }
            Verdict::Run(None) => {}
            Verdict::Run(Some(line)) => G::exec(self, &line, &mut out),
        }
        if !G::answers(self) {
            return;
        }
        for text in out.finish() {
            self.send_oob(from, &text);
        }
    }

    /// `SV_DirectConnect` on a plain-text `connect "<userinfo>"` up to the
    /// game's `ClientConnect` (cod2-protocol.md 17.1, cod4-handshake.md 3):
    /// the protocol, the reconnect limit, the challenge and its ping off
    /// the LAN, the slot (a new one within vcod's per-address cap,
    /// [`link::PER_IP_CVAR`]), `g_password`. The same client again takes its old
    /// slot, freed first when live (cod4-handshake.md 9, "Reconnects"); a
    /// refused password leaves that slot free.
    pub(crate) fn connect_slot(&mut self, from: SocketAddr, userinfo: &str) -> Option<Admit> {
        let svs = self.core.sv_time_ms;
        let key = |k: &str| vcod_common::net::info_value_for_key(userinfo, k).unwrap_or("");
        let atoi = crate::game::builtins::cvar::atoi;
        if atoi(key("protocol")) != G::PROTOCOL {
            let refusal = format!("error\nEXE_SERVER_IS_DIFFERENT_VER\u{15}{}\n", G::VERSION);
            self.send_oob(from, &refusal);
            return None;
        }
        let challenge = atoi(key("challenge"));
        let qport = atoi(key("qport")) as u16;
        let same = |c: &G::Client| {
            c.link.addr.ip() == from.ip()
                && (c.link.netchan.qport == qport || c.link.addr.port() == from.port())
        };
        let limit = self.int("sv_reconnectlimit") * 1000;
        if self
            .core
            .clients
            .iter()
            .flatten()
            .find(|c| same(c))
            .is_some_and(|c| svs - c.last_connect_ms < limit)
        {
            log::debug!("{from}: reconnect rejected: too soon");
            return None;
        }
        let Some((ci, ping)) = self.core.challenges.connect(from, challenge, svs) else {
            self.send_oob(from, "error\nEXE_BAD_CHALLENGE");
            return None;
        };
        log::info!("Client {ci} connecting with {ping} challenge ping from {from}");
        if !link::Lan::Cod2.is_lan(from.ip()) {
            let (min, max) = (self.int("sv_minPing"), self.int("sv_maxPing"));
            if min != 0 && ping < min {
                self.send_oob(from, "error\nEXE_ERR_HIGH_PING_ONLY");
                return None;
            }
            if max != 0 && ping > max {
                self.send_oob(from, "error\nEXE_ERR_LOW_PING_ONLY");
                return None;
            }
        }
        let slot = match self
            .core
            .clients
            .iter()
            .position(|c| c.as_ref().is_some_and(same))
        {
            Some(slot) => {
                // vcod's guard, as CoD1's `direct_connect`: retail hands a
                // live slot to any challenge issued to the ip, so a host
                // behind the same NAT could take over a player by his
                // qport. Only the slot's own challenge replaces a client
                // heard from in the last 3 s.
                if self.core.clients[slot].as_ref().is_some_and(|c| {
                    c.state > State::Zombie
                        && c.link.netchan.challenge != challenge
                        && svs.wrapping_sub(c.link.last_packet_ms) < RECONNECT_SILENCE_MS
                }) {
                    log::info!(
                        "{from}: connect with a foreign challenge refused, client {slot} is live"
                    );
                    return None;
                }
                if self.core.clients[slot]
                    .as_ref()
                    .is_some_and(|c| c.state > State::Zombie)
                {
                    G::free_level_slot(self, slot);
                }
                slot
            }
            None => {
                if self.refuse_per_ip(from, false) {
                    return None;
                }
                let start = G::first_slot(self, key("password")).min(self.core.clients.len());
                let Some(slot) =
                    (start..self.core.clients.len()).find(|&i| self.core.clients[i].is_none())
                else {
                    self.send_oob(from, "error\nEXE_SERVERISFULL");
                    return None;
                };
                slot
            }
        };
        if G::password_refused(self, key("password")) {
            self.core.clients[slot] = None;
            self.send_oob(from, "error\nGAME_INVALIDPASSWORD");
            return None;
        }
        Some(Admit {
            slot,
            qport,
            challenge,
            ci,
        })
    }

    /// The console commands both engines run alike: `serverinfo` and
    /// `systeminfo` (`Info_Print`, the key padded to 20), `dumpuser`,
    /// `tell`, `banUser`, `banClient` and `unbanUser`. False for any other.
    pub(crate) fn console_common(&mut self, args: &[String], out: &mut Redirect) -> bool {
        let arg = |i: usize| args.get(i).map_or("", String::as_str);
        let print_info = |out: &mut Redirect, info: &str| {
            for (k, v) in vcod_common::net::connectionless::info_pairs(info) {
                out.print(&format!("{k:<20}{v}\n"));
            }
        };
        match args[0].to_ascii_lowercase().as_str() {
            "serverinfo" => {
                out.print("Server info settings:\n");
                print_info(out, &self.core.configstrings[0]);
            }
            "systeminfo" => {
                out.print("System info settings:\n");
                print_info(out, &self.core.configstrings[1]);
            }
            // `SV_DumpUser_f`: `userinfo`, a rule, then the pairs.
            "dumpuser" => {
                if args.len() != 2 {
                    out.print("Usage: info <userid>\n");
                } else if let Some(slot) = G::find_player(self, true, arg(1), out) {
                    out.print("userinfo\n--------\n");
                    let c = self.core.clients[slot].as_ref().unwrap();
                    print_info(out, &c.link.userinfo);
                }
            }
            // `SV_ConTell_f`: the chat line to one client in the world;
            // anything else is silent.
            "tell" => {
                let to = crate::game::builtins::cvar::atoi(arg(1));
                if args.len() > 2
                    && let Ok(slot) = usize::try_from(to)
                    && self
                        .core
                        .clients
                        .get(slot)
                        .and_then(Option::as_ref)
                        .is_some_and(|c| c.state == State::Active)
                {
                    let text = format!("h \"\u{15}console: {}\"", console_args(&args[2..]));
                    self.send_server_command(slot, &text);
                }
            }
            // `banUser` and `banClient` into `SV_BanClient`.
            "banuser" | "banclient" => {
                let by_name = args[0].eq_ignore_ascii_case("banuser");
                if args.len() != 2 {
                    out.print(if by_name {
                        "Usage: banUser <player name>\n"
                    } else {
                        "Usage: banClient <client number>\n"
                    });
                } else if let Some(slot) = G::find_player(self, by_name, arg(1), out) {
                    G::ban_client(self, slot, out);
                }
            }
            // `unbanUser` into `SV_UnbanClient`.
            "unbanuser" => {
                if args.len() != 2 {
                    out.print("Usage: unban <client name>\n");
                } else {
                    let name = crate::game::say::clean_name(arg(1));
                    match self.core.bans.unban(&name) {
                        None => {}
                        Some(0) => out.print(&format!("no banned user has name {name}\n")),
                        Some(n) => out.print(&format!("unbanned {n} user(s) named {name}\n")),
                    }
                }
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_id_steps_as_retail_s() {
        assert_eq!(spawn_server_id(0), 16);
        assert_eq!(spawn_server_id(17), 33);
        assert_eq!(spawn_server_id(0x1f), 0x2f);
        assert_eq!(spawn_server_id(0xf3), 0x13);
        assert_eq!(restart_server_id(16), 17);
        assert_eq!(restart_server_id(0x1f), 0x10);
    }
}
