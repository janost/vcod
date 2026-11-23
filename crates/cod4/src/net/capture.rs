//! The datagram logs `tools/cod4_handshake_probe.py`,
//! `examples/cod4_probe.rs` and `tools/lo_capture.py` write: one `<ms>
//! <c2s|s2c|auth> <peer> <hex>` line per datagram, raw; `#` lines are the
//! recipe. [`Session`] walks one connection in both directions, each
//! message unscrambled with the key the other direction set up; on CoD4x
//! 21 the side channel's packets too.

use super::clmsg::{self, ClOp, ClientHeader, ClientMessage, UserCmd};
use super::parse::{Client, Parsed};
use super::reliable::{self, Inbound, Packet};
use super::wire::Dialect;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use vcod_common::net::netchan::{Netchan, NetchanParams, scramble_client_ops};

pub struct Line {
    pub ms: u64,
    pub dir: String,
    pub data: Vec<u8>,
}

pub fn parse(text: &str) -> Result<Vec<Line>> {
    let mut out = Vec::new();
    for l in text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
    {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() != 4 {
            bail!("bad line: {l}");
        }
        let hex = f[3].as_bytes();
        let data = hex
            .chunks(2)
            .map(|c| u8::from_str_radix(std::str::from_utf8(c)?, 16).map_err(Into::into))
            .collect::<Result<_>>()?;
        out.push(Line {
            ms: f[0].parse()?,
            dir: f[1].to_string(),
            data,
        });
    }
    Ok(out)
}

/// A server netchan message, reassembled and unscrambled.
pub struct Message {
    pub ms: u64,
    pub sequence: u32,
    pub reliable_ack: i32,
    /// The Huffman block past the plain `reliableAcknowledge`.
    pub block: Vec<u8>,
}

/// One decoded netchan message of a capture.
pub enum Item {
    Server {
        msg: Message,
        parsed: Parsed,
    },
    Client {
        msg: ClientMessage,
        /// The compressed op block, unscrambled.
        block: Vec<u8>,
        /// The usercmd key the moves were read with.
        key: i32,
    },
    /// A CoD4x side channel packet and the message bodies it completed.
    /// A server gamestate among them is already the client's.
    Side {
        from_server: bool,
        packet: Packet,
        messages: Vec<Vec<u8>>,
    },
}

/// Both directions of one connection: the server's messages through
/// [`Client::parse`], the client's through [`clmsg::read_ops`].
#[derive(Default)]
pub struct Session {
    pub client: Client,
    challenge: Option<i32>,
    chan: Option<Netchan>,
    /// The client's commands by sequence: what keys the server's messages.
    pub client_commands: BTreeMap<i32, Vec<u8>>,
    /// CoD4x: the side channel's streams from the server and to it.
    side_in: Inbound,
    side_out: Inbound,
}

impl Session {
    /// A 1.7 connection's.
    pub fn new() -> Session {
        Session::default()
    }

    pub fn with_dialect(dialect: Dialect) -> Session {
        Session {
            client: Client::with_dialect(dialect),
            ..Session::default()
        }
    }

    /// A side channel packet: its fragment into its direction's stream, a
    /// server gamestate into the client.
    fn side(&mut self, from_server: bool, d: &[u8]) -> Result<Item> {
        let packet = Packet::read(d)?;
        let stream = if from_server {
            &mut self.side_in
        } else {
            &mut self.side_out
        };
        let messages = stream.take(&packet)?;
        if from_server {
            for m in &messages {
                if m.get(..4) == Some(&reliable::CMD_GAMESTATE.to_le_bytes()[..]) {
                    self.client.take_side_gamestate(m)?;
                }
            }
        }
        Ok(Item::Side {
            from_server,
            packet,
            messages,
        })
    }

    /// Decodes one datagram; `None` for out-of-band traffic, a fragment
    /// short of its message, or a client packet with no ops.
    pub fn feed(&mut self, l: &Line) -> Result<Option<Item>> {
        let d = &l.data;
        if d.len() >= 4 && d[..4] == [0xff; 4] {
            if l.dir == "s2c"
                && let Some(rest) = d[4..].strip_prefix(b"challengeResponse ")
            {
                let s = std::str::from_utf8(rest)?;
                self.challenge = Some(s.split_whitespace().next().context("challenge")?.parse()?);
            }
            return Ok(None);
        }
        let dialect = self.client.dialect;
        if dialect == Dialect::X21 && Packet::is_side(d) && matches!(l.dir.as_str(), "s2c" | "c2s")
        {
            return self.side(l.dir == "s2c", d).map(Some);
        }
        let challenge = || -> Result<i32> {
            self.challenge
                .context("netchan message before challengeResponse")
        };
        match l.dir.as_str() {
            "s2c" => {
                let ch = challenge()?;
                let chan = self
                    .chan
                    .get_or_insert_with(|| Netchan::with_params(0, ch, NetchanParams::COD2));
                let commands = &self.client_commands;
                let key = |_: &Netchan, ack| commands.get(&ack).cloned().unwrap_or_default();
                let Some((sequence, msg)) = chan.process_keyed(d, key)? else {
                    return Ok(None);
                };
                if msg.len() < 4 {
                    return Ok(None);
                }
                let reliable_ack = i32::from_le_bytes(msg[..4].try_into().unwrap());
                let block = msg[4..].to_vec();
                let parsed = self
                    .client
                    .parse(sequence, &block)
                    .with_context(|| format!("server message {sequence}"))?;
                Ok(Some(Item::Server {
                    msg: Message {
                        ms: l.ms,
                        sequence,
                        reliable_ack,
                        block,
                    },
                    parsed,
                }))
            }
            "c2s" => {
                let ch = challenge()?;
                let Some((h, block)) = ClientHeader::read(d, dialect) else {
                    bail!("client packet of {} bytes", d.len());
                };
                if block.is_empty() {
                    return Ok(None);
                }
                let server_cmd = self.client.server_command(h.reliable_ack).to_vec();
                let mut block = block.to_vec();
                scramble_client_ops(h.seed(ch), &server_cmd, &mut block);
                let msg_head = ClientMessage {
                    sequence: h.sequence,
                    qport: h.qport,
                    server_id: h.server_id,
                    message_ack: h.message_ack,
                    reliable_ack: h.reliable_ack,
                    config_ack: h.config_ack,
                    ops: Vec::new(),
                };
                let feed = self
                    .client
                    .gamestate
                    .as_ref()
                    .map_or(0, |g| g.checksum_feed);
                let key = clmsg::usercmd_key(feed, msg_head.message_ack, &server_cmd);
                let plain = self.client.huff.decompress(&block);
                let ops = clmsg::read_ops(plain, key, &UserCmd::default())
                    .with_context(|| format!("client message {}", msg_head.sequence))?;
                for op in &ops {
                    if let ClOp::Command { sequence, text } = op {
                        self.client_commands.insert(*sequence, text.clone());
                    }
                }
                Ok(Some(Item::Client {
                    msg: ClientMessage { ops, ..msg_head },
                    block,
                    key,
                }))
            }
            _ => Ok(None),
        }
    }
}
