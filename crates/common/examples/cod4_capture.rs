//! Reads a CoD4 1.7 handshake capture (`tools/cod4_handshake_probe.py`):
//! prints the out-of-band lines, reassembles the server's netchan messages,
//! undoes the scramble and the Huffman block and walks the gamestate's
//! configstrings with vcod's `MsgReader` (docs/research/cod4-handshake.md).
//!
//!     cargo run -p vcod-common --example cod4_capture -- CAPTURE.txt [--strings]
//!     cargo run -p vcod-common --example cod4_capture -- --codes
//!
//! CoD4's Huffman tree is built here, not through `huffman::Huffman`: its
//! weights differ from CoD1's and CoD2's and the tree is a static build over
//! them, not the adaptive one (handshake doc, section 4).

use anyhow::{Context, bail};
use std::collections::BTreeMap;
use vcod_common::net::codec::MsgReader;
use vcod_common::net::netchan::{Netchan, NetchanParams};

/// `msg_hData` as `cod4_lnxded` 1.7 holds it (file offset 0x1e07a0).
const WEIGHTS: [u32; 256] = [
    274054, 68777, 40460, 40266, 48059, 39006, 48630, 27692, 17712, 15439, 12386, 10758, 9420,
    9979, 9346, 15256, 13184, 14319, 7750, 7221, 6095, 5666, 12606, 7263, 7322, 5807, 11628, 6199,
    7826, 6349, 7698, 9656, 28968, 5164, 13629, 6058, 4745, 4519, 5199, 4807, 5323, 3433, 3455,
    3563, 6979, 5229, 5002, 4423, 14108, 13631, 11908, 11801, 10261, 7635, 7215, 7218, 9353, 6161,
    5689, 4649, 5026, 5866, 8002, 10534, 15381, 8874, 11798, 7199, 12814, 6103, 4982, 5972, 6779,
    4929, 5333, 3503, 4345, 6098, 14117, 16440, 6446, 3062, 4695, 3085, 4198, 4013, 3878, 3414,
    5514, 4092, 3261, 4740, 4544, 3127, 3385, 7688, 11126, 6417, 5297, 4529, 6333, 4210, 7056,
    4658, 6190, 3512, 2843, 3479, 9369, 5203, 4980, 5881, 7509, 4292, 6097, 5492, 4648, 2996, 4988,
    4163, 6534, 4001, 4342, 4488, 6039, 4827, 7112, 8654, 26712, 8688, 9677, 9368, 7209, 3399,
    4473, 4677, 11087, 4094, 3404, 4176, 6733, 3702, 11420, 4867, 5968, 3475, 3722, 3560, 4571,
    2720, 3189, 3099, 4595, 4044, 4402, 3889, 4989, 3186, 3153, 5387, 8020, 3322, 3775, 2886, 4191,
    2879, 3110, 2576, 3693, 2436, 4935, 3017, 3538, 5688, 3444, 3410, 9170, 4708, 3425, 3273, 3684,
    4564, 6957, 4817, 5224, 3285, 3143, 4227, 5630, 6053, 5851, 6507, 13692, 8270, 8260, 5583,
    7568, 4082, 3984, 4574, 6440, 3533, 2992, 2708, 5190, 3889, 3799, 4582, 6020, 3464, 4431, 3495,
    2906, 2243, 3856, 3321, 8759, 3928, 2905, 3875, 4382, 3885, 5869, 6235, 10685, 4433, 4639,
    4305, 4683, 2849, 3379, 4683, 5477, 4127, 3853, 3515, 4913, 3601, 5237, 6617, 9019, 4857, 4112,
    5180, 5998, 4925, 4986, 6365, 7930, 5948, 8085, 7732, 8643, 8901, 9653, 32647,
];

const NYT: u16 = 256;
const INTERNAL: u16 = 257;

struct Node {
    weight: u64,
    symbol: u16,
    children: [usize; 2],
}

/// The static code tree: the NYT leaf merged with the lightest symbol, then
/// the two lightest of what is left, again and again. Ties sort leaves
/// before internal nodes, leaves by symbol with 155 and 205 swapped,
/// internal nodes oldest first (retail's qsort, INFERRED;
/// the capture's clean decode is the 1.7 check).
struct Tree {
    nodes: Vec<Node>,
    root: usize,
}

impl Tree {
    fn build(weights: &[u32; 256]) -> Tree {
        let mut nodes: Vec<Node> = (0..256)
            .map(|s| Node {
                weight: weights[s] as u64,
                symbol: s as u16,
                children: [0; 2],
            })
            .collect();
        nodes.push(Node {
            weight: 0,
            symbol: NYT,
            children: [0; 2],
        });
        let rank = |n: &Node, id: usize| -> (u64, u8, usize) {
            match n.symbol {
                155 => (n.weight, 0, 205),
                205 => (n.weight, 0, 155),
                s if s < 256 => (n.weight, 0, s as usize),
                _ => (n.weight, 1, id),
            }
        };
        let mut heap: Vec<usize> = (0..256).collect();
        heap.sort_by_key(|&i| rank(&nodes[i], i));
        let first = heap[0];
        nodes.push(Node {
            weight: nodes[first].weight,
            symbol: INTERNAL,
            children: [256, first],
        });
        heap[0] = nodes.len() - 1;
        while heap.len() > 1 {
            heap.sort_by_key(|&i| rank(&nodes[i], i));
            let (a, b) = (heap[0], heap[1]);
            nodes.push(Node {
                weight: nodes[a].weight + nodes[b].weight,
                symbol: INTERNAL,
                children: [a, b],
            });
            heap.drain(..2);
            heap.insert(0, nodes.len() - 1);
        }
        Tree {
            root: heap[0],
            nodes,
        }
    }

    /// The code of `sym` as a 0/1 string, root first.
    fn code(&self, sym: u16) -> String {
        fn walk(t: &Tree, n: usize, sym: u16, path: &mut String) -> bool {
            if t.nodes[n].symbol != INTERNAL {
                return t.nodes[n].symbol == sym;
            }
            for (bit, &c) in t.nodes[n].children.iter().enumerate() {
                path.push(if bit == 1 { '1' } else { '0' });
                if walk(t, c, sym, path) {
                    return true;
                }
                path.pop();
            }
            false
        }
        let mut path = String::new();
        walk(self, self.root, sym, &mut path);
        path
    }

    /// `MSG_ReadBitsCompress`: LSB-first bits, decode until the input's
    /// bits run out.
    fn decompress(&self, src: &[u8]) -> Vec<u8> {
        let limit = src.len() * 8;
        let mut bit = 0;
        let mut out = Vec::new();
        while bit < limit {
            let mut n = self.root;
            while self.nodes[n].symbol == INTERNAL {
                let b = src.get(bit >> 3).map_or(0, |v| (v >> (bit & 7)) & 1);
                bit += 1;
                n = self.nodes[n].children[b as usize];
            }
            out.push(self.nodes[n].symbol as u8);
        }
        out
    }
}

struct Line {
    ms: u64,
    dir: String,
    data: Vec<u8>,
}

fn parse(text: &str) -> anyhow::Result<Vec<Line>> {
    let mut out = Vec::new();
    for l in text.lines().filter(|l| !l.trim().is_empty()) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() != 4 {
            bail!("bad line: {l}");
        }
        let hex = f[3];
        let data = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<_, _>>()?;
        out.push(Line {
            ms: f[0].parse()?,
            dir: f[1].to_string(),
            data,
        });
    }
    Ok(out)
}

fn le32(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(d[at..at + 4].try_into().unwrap())
}

fn printable(b: &[u8]) -> String {
    b.iter()
        .map(|&c| match c {
            b'\n' => "\\n".to_string(),
            0x20..=0x7e => (c as char).to_string(),
            _ => format!("<{c:02x}>"),
        })
        .collect()
}

/// Walks a decoded server message: server commands, then the gamestate's
/// configstring op, up to the first baseline (which needs the CoD4 delta
/// codec).
fn walk(plain: Vec<u8>, show_strings: bool) {
    let total = plain.len();
    let mut r = MsgReader::from_plain(plain);
    loop {
        let op = r.read_byte();
        if r.is_overflowed() {
            println!("    <end of message>");
            return;
        }
        match op {
            7 => {
                println!("    svc_EOF at byte {}", r.readcount() - 1);
                return;
            }
            4 => {
                let seq = r.read_long();
                let s = r.read_string();
                println!("    svc_serverCommand {seq} {s:?}");
            }
            1 => {
                let seq = r.read_long();
                println!("    svc_gamestate serverCommandSequence {seq}");
                gamestate(&mut r, show_strings, total);
                return;
            }
            o => {
                println!("    op {o} at byte {}: stop", r.readcount() - 1);
                return;
            }
        }
    }
}

fn gamestate(r: &mut MsgReader, show: bool, total: usize) {
    let op = r.read_byte();
    if op != 2 {
        println!("    first gamestate op {op}, not svc_configstring");
        return;
    }
    let count = r.read_short() as u16;
    let mut last: i32 = -1;
    let mut strings = BTreeMap::new();
    let mut runs = 0;
    for _ in 0..count {
        let idx = if r.read_bits(1) == 1 {
            last + 1
        } else {
            runs += 1;
            r.read_bits(12)
        };
        let s = r.read_big_string();
        strings.insert(idx, s);
        last = idx;
    }
    println!(
        "    svc_configstring: {count} strings, {runs} explicit indices, last index {last}, bytes read {}",
        r.readcount()
    );
    for (i, s) in &strings {
        if show || *i < 30 {
            println!("      cs {i:4} {s:?}");
        }
    }
    let next = r.read_byte();
    println!(
        "    next op {next} at byte {} of {total} (3 = svc_baseline; the rest needs the delta codec)",
        r.readcount() - 1
    );
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().context("usage: cod4_capture <capture.txt>")?;
    let show = args.iter().any(|a| a == "--strings");
    let tree = Tree::build(&WEIGHTS);
    if path == "--codes" {
        // Each symbol's code, root first, for diffing against another build.
        for sym in 0..=256u16 {
            println!("{sym} {}", tree.code(sym));
        }
        return Ok(());
    }
    let lines = parse(&std::fs::read_to_string(path)?)?;

    let mut challenge: Option<i32> = None;
    let mut netchan: Option<Netchan> = None;
    let mut quiet = 0usize;
    for l in &lines {
        let d = &l.data;
        if d.len() >= 4 && d[..4] == [0xff; 4] {
            let body = &d[4..];
            let shown = if body.starts_with(b"stats\0") {
                format!(
                    "stats qport {} index {} ({} bytes)",
                    u16::from_le_bytes([body[6], body[7]]),
                    body[8],
                    d.len()
                )
            } else {
                printable(&body[..body.len().min(160)])
            };
            println!("{:6} {} oob {shown}", l.ms, l.dir);
            if let Some(rest) = body.strip_prefix(b"challengeResponse ") {
                challenge = Some(std::str::from_utf8(rest)?.trim().parse()?);
            }
            continue;
        }
        if l.dir == "c2s" {
            quiet += 1;
            continue;
        }
        if l.dir != "s2c" {
            continue;
        }
        let Some(ch) = challenge else {
            bail!("netchan message before challengeResponse");
        };
        // CoD2's netchan (long fragment start, 0x20000 cap, raw key) on CoD4.
        let chan = netchan.get_or_insert_with(|| Netchan::with_params(0, ch, NetchanParams::COD2));
        let word = le32(d, 0);
        if word & 0x8000_0000 != 0 {
            let start = le32(d, 4);
            let len = u16::from_le_bytes([d[8], d[9]]);
            println!(
                "{:6} s2c seq {} fragment start {start} len {len}",
                l.ms,
                word & 0x7fff_ffff
            );
        }
        let Some((seq, mut msg)) = chan.frame(d)? else {
            continue;
        };
        let ack = le32(&msg, 0);
        // No client command was sent, so the key walks an empty string.
        chan.decode(seq, b"", &mut msg);
        let block = &msg[4..];
        let plain = tree.decompress(block);
        println!(
            "{:6} s2c seq {seq} message {} bytes, reliableAck {ack}, block {} -> {} plain, first {:02x?}",
            l.ms,
            msg.len(),
            block.len(),
            plain.len(),
            &plain[..plain.len().min(8)]
        );
        walk(plain, show);
    }
    println!("{quiet} client netchan packets");
    Ok(())
}
