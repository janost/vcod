//! A raw packet log: every datagram of a session in order, both directions,
//! out of band ones included, so a replay rebuilds the challenge and the
//! netchan state from the bytes alone.
//!
//! Format: `VC2CAP1\n`, then per packet a direction byte (`<` from the
//! server, `>` to it), a little-endian u32 length and the bytes.

const MAGIC: &[u8] = b"VC2CAP1\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    FromServer,
    ToServer,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capture {
    pub packets: Vec<(Dir, Vec<u8>)>,
}

impl Capture {
    pub fn push(&mut self, dir: Dir, bytes: &[u8]) {
        self.packets.push((dir, bytes.to_vec()));
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut v = MAGIC.to_vec();
        for (dir, p) in &self.packets {
            v.push(match dir {
                Dir::FromServer => b'<',
                Dir::ToServer => b'>',
            });
            v.extend_from_slice(&(p.len() as u32).to_le_bytes());
            v.extend_from_slice(p);
        }
        v
    }

    pub fn decode(data: &[u8]) -> anyhow::Result<Self> {
        let mut rest = data
            .strip_prefix(MAGIC)
            .ok_or_else(|| anyhow::anyhow!("not a VC2CAP1 capture"))?;
        let mut packets = Vec::new();
        while !rest.is_empty() {
            anyhow::ensure!(rest.len() >= 5, "truncated record");
            let dir = match rest[0] {
                b'<' => Dir::FromServer,
                b'>' => Dir::ToServer,
                d => anyhow::bail!("bad direction {d}"),
            };
            let len = u32::from_le_bytes(rest[1..5].try_into()?) as usize;
            anyhow::ensure!(rest.len() >= 5 + len, "truncated packet");
            packets.push((dir, rest[5..5 + len].to_vec()));
            rest = &rest[5 + len..];
        }
        Ok(Capture { packets })
    }
}

/// What one client of a capture read: every server command in order,
/// every snapshot in order, and the parser as the last message left it.
pub struct Replayed {
    pub parser: super::Parser,
    pub commands: Vec<String>,
    pub snapshots: Vec<super::Snapshot2>,
    /// Every server message past the netchan, by sequence: the
    /// acknowledge, then the unscrambled, still compressed op block.
    pub messages: Vec<(i32, Vec<u8>)>,
}

/// Every server message of `cap` through a [`super::Parser`], the client
/// side rebuilt from the bytes: the challenge and qport key the netchan,
/// and the client's reliable commands (decoded from its own packets) key
/// the server's later messages (`SV_Netchan_Encode`, cod2-protocol.md 2.4).
pub fn replay(cap: &Capture) -> anyhow::Result<Replayed> {
    use super::oob::{self, Reply};
    use super::ops::{CLC_BITS, Clc, MAX_RELIABLE_COMMANDS};
    use vcod_common::net::codec::MsgReader;
    use vcod_common::net::huffman::Huffman;
    use vcod_common::net::netchan::{Netchan, NetchanParams, scramble_client_ops};

    let huff = Huffman::new();
    let mut challenge = None;
    let mut nc: Option<Netchan> = None;
    let mut out = Replayed {
        parser: super::Parser::new(),
        commands: Vec::new(),
        snapshots: Vec::new(),
        messages: Vec::new(),
    };
    let mut client_commands = vec![Vec::new(); MAX_RELIABLE_COMMANDS];
    for (dir, p) in &cap.packets {
        if let Some(pl) = oob::payload(p) {
            match (dir, oob::parse_reply(pl)) {
                (Dir::FromServer, Reply::Challenge(c)) => challenge = Some(c),
                (Dir::ToServer, _) => {
                    let text = String::from_utf8_lossy(pl);
                    let qport = text
                        .strip_prefix("connect \"")
                        .and_then(|ui| oob::info_value_for_key(ui.trim_end_matches('"'), "qport"));
                    if let (Some(q), Some(c)) = (qport, challenge) {
                        nc = Some(Netchan::with_params(q.parse()?, c, NetchanParams::COD2));
                    }
                }
                _ => {}
            }
            continue;
        }
        let Some(nc) = nc.as_mut() else { continue };
        match dir {
            Dir::ToServer => {
                anyhow::ensure!(p.len() >= 15, "short client packet");
                let msg_ack = i32::from_le_bytes(p[7..11].try_into()?);
                let rel_ack = i32::from_le_bytes(p[11..15].try_into()?);
                let key = out.parser.server_command(rel_ack).to_vec();
                let mut block = p[15..].to_vec();
                scramble_client_ops(
                    (nc.challenge ^ i32::from(p[6]) ^ msg_ack) as u8,
                    &key,
                    &mut block,
                );
                let mut r = MsgReader::from_plain(huff.decompress_block(&block));
                while r.read_bits(CLC_BITS) == Clc::ClientCommand as i32 {
                    let seq = r.read_long();
                    client_commands[seq as usize % MAX_RELIABLE_COMMANDS] = r.read_string_bytes();
                }
            }
            Dir::FromServer => {
                let Some((seq, mut body)) = nc.frame(p)? else {
                    continue;
                };
                let ack = i32::from_le_bytes(body[0..4].try_into()?);
                let key = client_commands[ack as usize % MAX_RELIABLE_COMMANDS].clone();
                nc.decode(seq, &key, &mut body);
                out.messages.push((seq as i32, body.clone()));
                let plain = huff.decompress_block(&body[4..]);
                let parsed = out.parser.parse(seq as i32, &plain)?;
                out.commands
                    .extend(parsed.commands.iter().map(|(_, t)| super::latin1(t)));
                if let Some(n) = parsed.snapshot
                    && let Some(s) = out.parser.frame(n)
                {
                    out.snapshots.push(s.clone());
                }
            }
        }
    }
    Ok(out)
}
