//! The datagram logs `tools/cod4_handshake_probe.py` and
//! `examples/cod4_probe.rs` write: one `<ms> <c2s|s2c|auth> <peer> <hex>` line
//! per datagram, raw; `#` lines are the recipe. [`Session`] walks one in both directions, each
//! message unscrambled with the key the other direction set up.

use super::clmsg::{self, ClOp, ClientMessage, UserCmd};
use super::parse::{Client, Parsed};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use vcod_common::net::netchan::{Netchan, NetchanParams, read_client_message};

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
}

impl Session {
    pub fn new() -> Session {
        Session::default()
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
                if d.len() < 15 {
                    bail!("client packet of {} bytes", d.len());
                }
                if d.len() == 15 {
                    return Ok(None);
                }
                let key = |ack| self.client.server_command(ack).to_vec();
                let m = read_client_message(d, ch, key).context("client header")?;
                let msg_head = ClientMessage {
                    sequence: m.sequence,
                    qport: u16::from_le_bytes(d[4..6].try_into().unwrap()),
                    server_id: m.server_id,
                    message_ack: m.message_ack,
                    reliable_ack: m.reliable_ack,
                    ops: Vec::new(),
                };
                let server_cmd = self.client.server_command(m.reliable_ack).to_vec();
                let block = m.ops;
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
