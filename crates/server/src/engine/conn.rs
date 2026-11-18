//! What CoD2's and CoD4's servers do alike between a datagram and the
//! level, over their [`Conn`] slots: the netchan's packet acceptance, the
//! command sequence, the reliable ring, the message pacing and its
//! fragments, the script's commands out to their clients and the console
//! queue. Both retail engines run these routines line for line alike at
//! their own addresses (cod2-protocol.md 2, 3.4, 17; cod4-handshake.md 3,
//! 15); what differs is a [`ConnGame`] hook. CoD1's server keeps its own.

use std::net::SocketAddr;

use vcod_common::net::codec::MsgWriter;
use vcod_common::net::netchan::ClientMessage;
use vcod_common::net::protocol::WireGame;

use super::link;
use super::slot::{Conn, State};
use super::{Engine, EngineCore, GameModule};
use crate::game::script::ScriptRuntime;
use crate::rcon::Redirect;

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

/// `SV_PacketEvent`'s slot by address and qport and `Netchan_Process`,
/// the acknowledges checked: a negative `messageAcknowledge` is dropped
/// (cod2-protocol.md 2.2), and so is a `reliableAcknowledge` past what was
/// sent or so far behind that its key is gone, which only a forgery or
/// garbage carries. Nothing is committed yet ([`commit`]).
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
    if m.message_ack < 0 || !c.link.reliable_ack_in_window(m.reliable_ack) {
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
        let Some(c) = self.core.clients[slot].as_mut() else {
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
            if to == crate::game::commands::ALL_CLIENTS {
                self.send_to_all(&cmd);
            } else {
                self.send_server_command(to, &cmd);
            }
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
    /// cod4-handshake.md 15).
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
}
