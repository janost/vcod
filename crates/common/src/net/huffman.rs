//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! Port of RTCW-MP `qcommon/huffman.c`: the static message tree built from
//! `msg_hData` (`docs/protocol-1.1.md`, "Huffman coding") and the adaptive
//! coder the `connect` packet uses.
//!
//! Pointers into the C's two 768-entry pools are indices here, `NIL` for NULL,
//! so the two can be diffed side by side.

const HMAX: usize = 256;
/// Not-yet-transmitted; only the tree's initial node carries it.
const NYT: i32 = HMAX as i32;
const INTERNAL_NODE: i32 = HMAX as i32 + 1;
const POOL: usize = 768;
const NIL: i32 = -1;

/// `node_t`. `next`/`prev`/`head` are the rank list, used only during build.
#[derive(Clone, Copy)]
struct Node {
    left: i32,
    right: i32,
    parent: i32,
    next: i32,
    prev: i32,
    /// Index into `ptrs`: the C's `node_t **head`.
    head: i32,
    weight: i32,
    symbol: i32,
}

impl Node {
    const NULL: Node = Node {
        left: NIL,
        right: NIL,
        parent: NIL,
        next: NIL,
        prev: NIL,
        head: NIL,
        weight: 0,
        symbol: 0,
    };
}

/// The frozen static tree.
pub struct Huffman {
    nodes: Vec<Node>,
    root: i32,
    /// `huff_t.loc`: symbol -> node index.
    loc: [i32; HMAX + 1],
}

/// `huff_t`, the adaptive coder.
struct Builder {
    bloc_node: usize,
    bloc_ptrs: usize,
    tree: i32,
    lhead: i32,
    loc: [i32; HMAX + 1],
    /// Free chain through `ptrs`; a free slot holds the next free index, as
    /// the C's `freelist` aliasing does.
    freelist: i32,
    nodes: [Node; POOL],
    ptrs: [i32; POOL],
}

impl Builder {
    /// `Huff_Init`. `ltail` is dropped; huffman.c only ever assigns it.
    fn new() -> Self {
        let mut b = Builder {
            bloc_node: 0,
            bloc_ptrs: 0,
            tree: NIL,
            lhead: NIL,
            loc: [NIL; HMAX + 1],
            freelist: NIL,
            nodes: [Node::NULL; POOL],
            ptrs: [NIL; POOL],
        };
        let n = b.bloc_node as i32;
        b.bloc_node += 1;
        b.tree = n;
        b.lhead = n;
        b.loc[NYT as usize] = n;
        b.nodes[n as usize].symbol = NYT;
        b.nodes[n as usize].weight = 0;
        b
    }

    /// `get_ppnode`
    fn get_ppnode(&mut self) -> i32 {
        if self.freelist == NIL {
            let p = self.bloc_ptrs as i32;
            self.bloc_ptrs += 1;
            p
        } else {
            let tppnode = self.freelist;
            self.freelist = self.ptrs[tppnode as usize];
            tppnode
        }
    }

    /// `free_ppnode`
    fn free_ppnode(&mut self, ppnode: i32) {
        self.ptrs[ppnode as usize] = self.freelist;
        self.freelist = ppnode;
    }

    /// `swap`: exchange the two nodes' places in the tree.
    fn swap(&mut self, node1: i32, node2: i32) {
        let par1 = self.nodes[node1 as usize].parent;
        let par2 = self.nodes[node2 as usize].parent;

        if par1 != NIL {
            if self.nodes[par1 as usize].left == node1 {
                self.nodes[par1 as usize].left = node2;
            } else {
                self.nodes[par1 as usize].right = node2;
            }
        } else {
            self.tree = node2;
        }

        if par2 != NIL {
            if self.nodes[par2 as usize].left == node2 {
                self.nodes[par2 as usize].left = node1;
            } else {
                self.nodes[par2 as usize].right = node1;
            }
        } else {
            self.tree = node1;
        }

        self.nodes[node1 as usize].parent = par2;
        self.nodes[node2 as usize].parent = par1;
    }

    /// `swaplist`: exchange them in the rank list.
    fn swaplist(&mut self, node1: i32, node2: i32) {
        let par1 = self.nodes[node1 as usize].next;
        self.nodes[node1 as usize].next = self.nodes[node2 as usize].next;
        self.nodes[node2 as usize].next = par1;

        let par1 = self.nodes[node1 as usize].prev;
        self.nodes[node1 as usize].prev = self.nodes[node2 as usize].prev;
        self.nodes[node2 as usize].prev = par1;

        if self.nodes[node1 as usize].next == node1 {
            self.nodes[node1 as usize].next = node2;
        }
        if self.nodes[node2 as usize].next == node2 {
            self.nodes[node2 as usize].next = node1;
        }
        let n1next = self.nodes[node1 as usize].next;
        if n1next != NIL {
            self.nodes[n1next as usize].prev = node1;
        }
        let n2next = self.nodes[node2 as usize].next;
        if n2next != NIL {
            self.nodes[n2next as usize].prev = node2;
        }
        let n1prev = self.nodes[node1 as usize].prev;
        if n1prev != NIL {
            self.nodes[n1prev as usize].next = node1;
        }
        let n2prev = self.nodes[node2 as usize].prev;
        if n2prev != NIL {
            self.nodes[n2prev as usize].next = node2;
        }
    }

    /// `increment`: bump the weight and re-rank, recursing to the root.
    fn increment(&mut self, node: i32) {
        if node == NIL {
            return;
        }

        let next = self.nodes[node as usize].next;
        if next != NIL && self.nodes[next as usize].weight == self.nodes[node as usize].weight {
            let lnode = self.ptrs[self.nodes[node as usize].head as usize];
            if lnode != self.nodes[node as usize].parent {
                self.swap(lnode, node);
            }
            self.swaplist(lnode, node);
        }
        let prev = self.nodes[node as usize].prev;
        if prev != NIL && self.nodes[prev as usize].weight == self.nodes[node as usize].weight {
            let head = self.nodes[node as usize].head;
            self.ptrs[head as usize] = prev;
        } else {
            let head = self.nodes[node as usize].head;
            self.ptrs[head as usize] = NIL;
            self.free_ppnode(head);
        }
        self.nodes[node as usize].weight += 1;
        let next = self.nodes[node as usize].next;
        if next != NIL && self.nodes[next as usize].weight == self.nodes[node as usize].weight {
            self.nodes[node as usize].head = self.nodes[next as usize].head;
        } else {
            let head = self.get_ppnode();
            self.nodes[node as usize].head = head;
            self.ptrs[head as usize] = node;
        }
        let parent = self.nodes[node as usize].parent;
        if parent != NIL {
            self.increment(parent);
            if self.nodes[node as usize].prev == parent {
                self.swaplist(node, parent);
                if self.ptrs[self.nodes[node as usize].head as usize] == node {
                    let head = self.nodes[node as usize].head;
                    self.ptrs[head as usize] = parent;
                }
            }
        }
    }

    /// `Huff_addRef`: split the NYT leaf on a new symbol, else increment.
    fn add_ref(&mut self, ch: u8) {
        let ch = ch as usize;
        if self.loc[ch] == NIL {
            let tnode = self.bloc_node as i32;
            self.bloc_node += 1;
            let tnode2 = self.bloc_node as i32;
            self.bloc_node += 1;

            self.nodes[tnode2 as usize].symbol = INTERNAL_NODE;
            self.nodes[tnode2 as usize].weight = 1;
            let lnext = self.nodes[self.lhead as usize].next;
            self.nodes[tnode2 as usize].next = lnext;
            if lnext != NIL {
                self.nodes[lnext as usize].prev = tnode2;
                if self.nodes[lnext as usize].weight == 1 {
                    self.nodes[tnode2 as usize].head = self.nodes[lnext as usize].head;
                } else {
                    let head = self.get_ppnode();
                    self.nodes[tnode2 as usize].head = head;
                    self.ptrs[head as usize] = tnode2;
                }
            } else {
                let head = self.get_ppnode();
                self.nodes[tnode2 as usize].head = head;
                self.ptrs[head as usize] = tnode2;
            }
            self.nodes[self.lhead as usize].next = tnode2;
            self.nodes[tnode2 as usize].prev = self.lhead;

            self.nodes[tnode as usize].symbol = ch as i32;
            self.nodes[tnode as usize].weight = 1;
            let lnext = self.nodes[self.lhead as usize].next;
            self.nodes[tnode as usize].next = lnext;
            if lnext != NIL {
                self.nodes[lnext as usize].prev = tnode;
                if self.nodes[lnext as usize].weight == 1 {
                    self.nodes[tnode as usize].head = self.nodes[lnext as usize].head;
                } else {
                    // this should never happen
                    let head = self.get_ppnode();
                    self.nodes[tnode as usize].head = head;
                    self.ptrs[head as usize] = tnode2;
                }
            } else {
                // this should never happen
                let head = self.get_ppnode();
                self.nodes[tnode as usize].head = head;
                self.ptrs[head as usize] = tnode;
            }
            self.nodes[self.lhead as usize].next = tnode;
            self.nodes[tnode as usize].prev = self.lhead;
            self.nodes[tnode as usize].left = NIL;
            self.nodes[tnode as usize].right = NIL;

            let lparent = self.nodes[self.lhead as usize].parent;
            if lparent != NIL {
                // lhead is guaranteed to be the NYT
                if self.nodes[lparent as usize].left == self.lhead {
                    self.nodes[lparent as usize].left = tnode2;
                } else {
                    self.nodes[lparent as usize].right = tnode2;
                }
            } else {
                self.tree = tnode2;
            }

            self.nodes[tnode2 as usize].right = tnode;
            self.nodes[tnode2 as usize].left = self.lhead;

            self.nodes[tnode2 as usize].parent = lparent;
            self.nodes[self.lhead as usize].parent = tnode2;
            self.nodes[tnode as usize].parent = tnode2;

            self.loc[ch] = tnode;

            self.increment(lparent);
        } else {
            self.increment(self.loc[ch]);
        }
    }

    /// `send`: the root-to-leaf path, collected upward and emitted reversed.
    fn send(&self, node: i32, out: &mut Vec<u8>, bloc: &mut usize) {
        let mut bits = [0u8; POOL];
        let mut n = 0;
        let mut cur = node;
        while self.nodes[cur as usize].parent != NIL {
            let parent = self.nodes[cur as usize].parent;
            bits[n] = u8::from(self.nodes[parent as usize].right == cur);
            n += 1;
            cur = parent;
        }
        for i in (0..n).rev() {
            add_bit(bits[i], out, bloc);
        }
    }

    /// `Huff_transmit`: an unseen symbol is the NYT code then the raw byte,
    /// MSB first.
    fn transmit(&self, ch: u8, out: &mut Vec<u8>, bloc: &mut usize) {
        if self.loc[ch as usize] == NIL {
            self.send(self.loc[NYT as usize], out, bloc);
            for i in (0..8).rev() {
                add_bit((ch >> i) & 1, out, bloc);
            }
        } else {
            self.send(self.loc[ch as usize], out, bloc);
        }
    }
}

/// Adaptive `Huff_Compress` over `buf[offset..]`, in place: a big-endian u16
/// length, then the code stream. Used for the `connect` packet from byte 12.
pub fn compress(buf: &mut Vec<u8>, offset: usize) {
    if buf.len() <= offset {
        return;
    }
    let size = buf.len() - offset;
    let mut b = Builder::new();
    let mut seq = vec![(size >> 8) as u8, (size & 0xff) as u8];
    let mut bloc = 16usize;
    for i in 0..size {
        let ch = buf[offset + i];
        b.transmit(ch, &mut seq, &mut bloc);
        b.add_ref(ch);
    }
    bloc += 8;
    seq.resize(bloc >> 3, 0);
    buf.truncate(offset);
    buf.extend_from_slice(&seq);
}

/// Inverse of [`compress`].
pub fn decompress(buf: &mut Vec<u8>, offset: usize) {
    if buf.len() <= offset + 1 {
        return;
    }
    let size = buf.len() - offset;
    let input = buf[offset..].to_vec();
    let mut b = Builder::new();
    let cch = (input[0] as usize) * 256 + input[1] as usize;
    let mut bloc = 16usize;
    let mut seq = Vec::with_capacity(cch);
    for _ in 0..cch {
        if (bloc >> 3) > size {
            seq.push(0);
            break;
        }
        let mut node = b.tree;
        while node != NIL && b.nodes[node as usize].symbol == INTERNAL_NODE {
            node = if get_bit(&input, &mut bloc) != 0 {
                b.nodes[node as usize].right
            } else {
                b.nodes[node as usize].left
            };
        }
        let mut ch = if node == NIL {
            0
        } else {
            b.nodes[node as usize].symbol
        };
        if ch == NYT {
            ch = 0;
            for _ in 0..8 {
                ch = (ch << 1) + i32::from(get_bit(&input, &mut bloc));
            }
        }
        seq.push(ch as u8);
        b.add_ref(ch as u8);
    }
    buf.truncate(offset);
    buf.extend_from_slice(&seq);
}

impl Huffman {
    /// `MSG_initHuffman`. One tree serves both directions, since neither side
    /// adapts afterwards.
    pub fn new() -> Self {
        let mut b = Builder::new();
        for (i, &freq) in MSG_HDATA.iter().enumerate() {
            for _ in 0..freq {
                b.add_ref(i as u8);
            }
        }
        Huffman {
            nodes: b.nodes[..b.bloc_node].to_vec(),
            root: b.tree,
            loc: b.loc,
        }
    }

    /// `Huff_offsetReceive`. Bits past the end of `data` read as 0, so the
    /// walk still ends at a leaf.
    pub fn offset_receive(&self, data: &[u8], bit_offset: &mut usize) -> u8 {
        let mut bloc = *bit_offset;
        let mut node = self.root;
        while node != NIL && self.nodes[node as usize].symbol == INTERNAL_NODE {
            node = if get_bit(data, &mut bloc) != 0 {
                self.nodes[node as usize].right
            } else {
                self.nodes[node as usize].left
            };
        }
        if node == NIL {
            // illegal tree; the C leaves *offset untouched
            return 0;
        }
        *bit_offset = bloc;
        // Only the unreachable NYT leaf would truncate here.
        self.nodes[node as usize].symbol as u8
    }

    /// `Huff_Decompress` (cod_lnxded 0x807f23c): no length prefix, decode
    /// until the input runs out.
    pub fn decompress_block(&self, src: &[u8]) -> Vec<u8> {
        let mut bit = 0usize;
        let limit = src.len() * 8;
        let mut out = Vec::with_capacity(src.len() * 2);
        while bit < limit {
            out.push(self.offset_receive(src, &mut bit));
        }
        out
    }

    /// `Huff_Compress` (cod_lnxded 0x807f03c), inverse of
    /// [`Self::decompress_block`].
    pub fn compress_block(&self, src: &[u8]) -> Vec<u8> {
        let mut bit = 0usize;
        let mut out = Vec::with_capacity(src.len());
        for &ch in src {
            self.offset_transmit(ch, &mut out, &mut bit);
        }
        out.resize(bit.div_ceil(8), 0);
        out
    }

    /// `Huff_offsetTransmit`; `out` grows as needed.
    pub fn offset_transmit(&self, ch: u8, out: &mut Vec<u8>, bit_offset: &mut usize) {
        let mut bloc = *bit_offset;
        let mut bits = [0u8; POOL];
        let mut n = 0;
        let mut node = self.loc[ch as usize];
        while self.nodes[node as usize].parent != NIL {
            let parent = self.nodes[node as usize].parent;
            bits[n] = u8::from(self.nodes[parent as usize].right == node);
            n += 1;
            node = parent;
        }
        for i in (0..n).rev() {
            add_bit(bits[i], out, &mut bloc);
        }
        *bit_offset = bloc;
    }
}

impl Default for Huffman {
    fn default() -> Self {
        Self::new()
    }
}

/// Append one bit at `bloc`, LSB first within a byte; `msg` shares the layout.
pub(crate) fn add_bit(bit: u8, out: &mut Vec<u8>, bloc: &mut usize) {
    let idx = *bloc >> 3;
    if idx >= out.len() {
        out.resize(idx + 1, 0);
    }
    if (*bloc & 7) == 0 {
        out[idx] = 0;
    }
    out[idx] |= bit << (*bloc & 7);
    *bloc += 1;
}

/// Read the bit at `bloc`; past the end reads as 0.
pub(crate) fn get_bit(fin: &[u8], bloc: &mut usize) -> u8 {
    let idx = *bloc >> 3;
    let t = if idx < fin.len() {
        (fin[idx] >> (*bloc & 7)) & 1
    } else {
        0
    };
    *bloc += 1;
    t
}

// msg_hData, RTCW-MP/src/qcommon/msg.c:1815-2072. Keep 8 per line for diffing.
#[rustfmt::skip]
const MSG_HDATA: [u32; 256] = [
    250315, 41193, 6292, 7106, 3730, 3750, 6110, 23283, // 0..7
    33317, 6950, 7838, 9714, 9257, 17259, 3949, 1778, // 8..15
    8288, 1604, 1590, 1663, 1100, 1213, 1238, 1134, // 16..23
    1749, 1059, 1246, 1149, 1273, 4486, 2805, 3472, // 24..31
    21819, 1159, 1670, 1066, 1043, 1012, 1053, 1070, // 32..39
    1726, 888, 1180, 850, 960, 780, 1752, 3296, // 40..47
    10630, 4514, 5881, 2685, 4650, 3837, 2093, 1867, // 48..55
    2584, 1949, 1972, 940, 1134, 1788, 1670, 1206, // 56..63
    5719, 6128, 7222, 6654, 3710, 3795, 1492, 1524, // 64..71
    2215, 1140, 1355, 971, 2180, 1248, 1328, 1195, // 72..79
    1770, 1078, 1264, 1266, 1168, 965, 1155, 1186, // 80..87
    1347, 1228, 1529, 1600, 2617, 2048, 2546, 3275, // 88..95
    2410, 3585, 2504, 2800, 2675, 6146, 3663, 2840, // 96..103
    14253, 3164, 2221, 1687, 3208, 2739, 3512, 4796, // 104..111
    4091, 3515, 5288, 4016, 7937, 6031, 5360, 3924, // 112..119
    4892, 3743, 4566, 4807, 5852, 6400, 6225, 8291, // 120..127
    23243, 7838, 7073, 8935, 5437, 4483, 3641, 5256, // 128..135
    5312, 5328, 5370, 3492, 2458, 1694, 1821, 2121, // 136..143
    1916, 1149, 1516, 1367, 1236, 1029, 1258, 1104, // 144..151
    1245, 1006, 1149, 1025, 1241, 952, 1287, 997, // 152..159
    1713, 1009, 1187, 879, 1099, 929, 1078, 951, // 160..167
    1656, 930, 1153, 1030, 1262, 1062, 1214, 1060, // 168..175
    1621, 930, 1106, 912, 1034, 892, 1158, 990, // 176..183
    1175, 850, 1121, 903, 1087, 920, 1144, 1056, // 184..191
    3462, 2240, 4397, 12136, 7758, 1345, 1307, 3278, // 192..199
    1950, 886, 1023, 1112, 1077, 1042, 1061, 1071, // 200..207
    1484, 1001, 1096, 915, 1052, 995, 1070, 876, // 208..215
    1111, 851, 1059, 805, 1112, 923, 1103, 817, // 216..223
    1899, 1872, 976, 841, 1127, 956, 1159, 950, // 224..231
    7791, 954, 1289, 933, 1127, 3207, 1020, 927, // 232..239
    1355, 768, 1040, 745, 952, 805, 1073, 740, // 240..247
    1013, 805, 1008, 796, 996, 1057, 11457, 13504, // 248..255
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_byte_roundtrip_all_values() {
        let h = Huffman::new();
        for ch in 0u16..256 {
            let mut out = Vec::new();
            let mut wbit = 0usize;
            h.offset_transmit(ch as u8, &mut out, &mut wbit);
            let mut rbit = 0usize;
            assert_eq!(h.offset_receive(&out, &mut rbit), ch as u8, "byte {ch}");
            assert_eq!(rbit, wbit);
        }
    }

    #[test]
    fn stream_roundtrip() {
        let h = Huffman::new();
        let data: Vec<u8> = (0u32..4096)
            .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
            .collect();
        let mut out = Vec::new();
        let mut wbit = 0usize;
        for &b in &data {
            h.offset_transmit(b, &mut out, &mut wbit);
        }
        let mut rbit = 0usize;
        let decoded: Vec<u8> = (0..data.len())
            .map(|_| h.offset_receive(&out, &mut rbit))
            .collect();
        assert_eq!(decoded, data);
    }

    #[test]
    fn common_bytes_compress_shorter() {
        let h = Huffman::new();
        let mut out = Vec::new();
        let mut wbit = 0usize;
        h.offset_transmit(0, &mut out, &mut wbit);
        assert!(wbit < 8, "0x00 coded in {wbit} bits");
    }

    #[test]
    fn hdata_checksum() {
        // Sum of the C table's 256 counts:
        //   python3 -c "import re; \
        //     l=open('RTCW-MP/src/qcommon/msg.c').read().split('\n')[1815:2071]; \
        //     print(sum(int(re.match(r'\s*(-?\d+),',x).group(1)) for x in l))"
        const EXPECTED: u64 = 1053340;
        assert_eq!(MSG_HDATA.iter().map(|&v| v as u64).sum::<u64>(), EXPECTED);
    }

    #[test]
    fn adaptive_compress_roundtrip() {
        let plain = b"\xff\xff\xff\xffconnect \"\\protocol\\1\\qport\\7331\\challenge\\-1234567\\name\\vcod\"";
        let mut buf = plain.to_vec();
        compress(&mut buf, 12);
        assert_eq!(&buf[..12], &plain[..12], "header must stay in the clear");
        assert_ne!(buf.len(), plain.len());
        decompress(&mut buf, 12);
        assert_eq!(buf, plain.to_vec());
    }

    /// Exercises the NYT escape for every byte value.
    #[test]
    fn adaptive_roundtrip_all_bytes() {
        let mut plain: Vec<u8> = (0..=255u8).collect();
        plain.extend((0..=255u8).rev());
        let mut buf = plain.clone();
        compress(&mut buf, 0);
        decompress(&mut buf, 0);
        assert_eq!(buf, plain);
    }

    #[test]
    fn adaptive_compress_ignores_short_buffers() {
        let mut buf = vec![1u8, 2, 3];
        compress(&mut buf, 3);
        assert_eq!(buf, vec![1, 2, 3]);
    }
}
