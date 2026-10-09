//! Serving a pak to a client: `SV_WriteDownloadToClient` (cod_lnxded
//! 0x8086290) and the `nextdl` / `retransdl` handlers
//! (docs/research/cod11-server-handshake.md, "Serving a download").

use vcod_common::net::msg::MsgWriter;

/// `svc_download`.
const SVC_DOWNLOAD: u8 = 6;
/// lnxded 1.1d reads its blocks 0x800 bytes at a time.
pub const BLOCK: usize = 2048;
/// Blocks in flight before an ack.
const WINDOW: u32 = 8;
/// Unacked blocks go again after this long.
const RETRANSMIT_MS: i32 = 1000;

/// Why a download is refused, the localized key the client shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// `EXE_CANTAUTODLGAMEPAK`: a stock pak.
    GamePak,
    /// `EXE_AUTODL_SERVERDISABLED` (`_PURE` on a pure server).
    Disabled { pure: bool },
    /// `EXE_AUTODL_FILENOTONSERVER`.
    NotFound,
}

impl Refusal {
    fn key(self) -> &'static str {
        match self {
            Refusal::GamePak => "EXE_CANTAUTODLGAMEPAK",
            Refusal::Disabled { pure: false } => "EXE_AUTODL_SERVERDISABLED",
            Refusal::Disabled { pure: true } => "EXE_AUTODL_SERVERDISABLED_PURE",
            Refusal::NotFound => "EXE_AUTODL_FILENOTONSERVER",
        }
    }
}

/// What a `nextdl` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextDl {
    Acked,
    /// The empty end block was acked; the transfer is over.
    Completed,
    /// Any block but the one due, which retail drops as `broken download`.
    Broken,
}

/// One client's transfer. Block `i` is bytes `i * BLOCK ..` of the file, and
/// the block past the last byte is the empty one that ends it.
pub struct ServerDownload {
    /// As the client asked, e.g. `main/foo.pk3`.
    pub name: String,
    /// `None` until the first write opens it.
    data: Option<Vec<u8>>,
    /// `downloadClientBlock`: the next block the client has to ack.
    acked: u32,
    /// `downloadCurrentBlock`: the next block to put on the wire.
    current: u32,
    /// `downloadSendTime`, the server clock of the last send or ack.
    send_time: i32,
}

impl ServerDownload {
    /// `SV_BeginDownload_f`: the name only; the file opens on the next write.
    pub fn new(name: &str) -> Self {
        ServerDownload {
            name: name.chars().take(63).collect(),
            data: None,
            acked: 0,
            current: 0,
            send_time: 0,
        }
    }

    fn blocks(&self) -> u32 {
        self.data
            .as_ref()
            .map_or(0, |d| d.len().div_ceil(BLOCK) as u32 + 1)
    }

    fn block(&self, i: u32) -> &[u8] {
        let d = self.data.as_deref().unwrap_or_default();
        let start = (i as usize * BLOCK).min(d.len());
        &d[start..(start + BLOCK).min(d.len())]
    }

    /// `SV_WriteDownloadToClient`: opens the file on the first call (`open`
    /// answers with its bytes or the refusal), then writes up to `budget`
    /// blocks inside the send window. False when the transfer is over
    /// (refused), so the caller drops it.
    pub fn write(
        &mut self,
        w: &mut MsgWriter,
        now_ms: i32,
        budget: u32,
        open: impl FnOnce(&str) -> Result<Vec<u8>, Refusal>,
    ) -> bool {
        if self.data.is_none() {
            match open(&self.name) {
                Ok(bytes) => {
                    log::info!("clientDownload: beginning {:?}", self.name);
                    self.data = Some(bytes);
                    self.send_time = now_ms;
                }
                Err(why) => {
                    log::info!("clientDownload: {:?} refused: {why:?}", self.name);
                    w.write_byte(SVC_DOWNLOAD);
                    w.write_short(0);
                    w.write_long(-1);
                    w.write_string(&format!("{}\x15{}", why.key(), self.name));
                    return false;
                }
            }
        }
        let in_flight = (self.acked + WINDOW).min(self.blocks());
        for _ in 0..budget.max(1) {
            if self.acked == in_flight {
                break;
            }
            if self.current == in_flight {
                if now_ms.wrapping_sub(self.send_time) <= RETRANSMIT_MS {
                    break;
                }
                self.current = self.acked;
            }
            let block = self.block(self.current);
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(self.current as i16);
            if self.current == 0 {
                w.write_long(self.data.as_ref().map_or(0, |d| d.len() as i32));
            }
            w.write_short(block.len() as i16);
            for &b in block {
                w.write_byte(b);
            }
            self.current += 1;
            self.send_time = now_ms;
        }
        true
    }

    /// `SV_NextDownload_f` (0x8086168) for `nextdl <block>`.
    pub fn next_dl(&mut self, block: i32, now_ms: i32) -> NextDl {
        if block != self.acked as i32 || self.data.is_none() {
            return NextDl::Broken;
        }
        if self.block(self.acked).is_empty() {
            log::info!("clientDownload: {:?} completed", self.name);
            return NextDl::Completed;
        }
        self.acked += 1;
        self.send_time = now_ms;
        NextDl::Acked
    }

    /// `SV_RetransmitDownload_f` (0x8087a2c): start over from the block due.
    pub fn retransmit(&mut self, block: i32) {
        if block == self.acked as i32 {
            self.current = self.acked;
        }
    }
}

/// `SV_WriteDownloadToClient`'s blocks per message: the whole 2 KiB units
/// the client's rate allows over one snapshot interval, plus one.
pub fn blocks_per_message(rate: i32, snapshot_ms: i32) -> u32 {
    (rate.saturating_mul(snapshot_ms) / 1000 / BLOCK as i32).max(0) as u32 + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::huffman::Huffman;
    use vcod_common::net::msg::MsgReader;

    /// The blocks one write put out, as `(block, size, bytes)`.
    fn blocks(ops: &[u8], huff: &Huffman) -> Vec<(i16, i32, Vec<u8>)> {
        let mut r = MsgReader::new(ops, huff);
        let mut out = Vec::new();
        while r.read_byte() == SVC_DOWNLOAD && !r.is_overflowed() {
            let n = r.read_short();
            let size = if n == 0 { r.read_long() } else { 0 };
            if size < 0 {
                out.push((n, size, r.read_string().into_bytes()));
                break;
            }
            let len = r.read_short();
            out.push((n, size, (0..len).map(|_| r.read_byte()).collect()));
        }
        out
    }

    #[test]
    fn a_file_goes_out_in_blocks_then_an_empty_one() {
        let huff = Huffman::new();
        let file: Vec<u8> = (0..5000).map(|i| i as u8).collect();
        let mut dl = ServerDownload::new("main/foo.pk3");
        let mut w = MsgWriter::new(&huff);
        assert!(dl.write(&mut w, 0, 8, |_| Ok(file.clone())));
        let got = blocks(&w.finish(), &huff);
        assert_eq!(got.len(), 4);
        assert_eq!((got[0].0, got[0].1, got[0].2.len()), (0, 5000, BLOCK));
        assert_eq!(got[2].2.len(), 5000 - 2 * BLOCK);
        assert!(got[3].2.is_empty(), "the end block");
        let joined: Vec<u8> = got.iter().flat_map(|b| b.2.clone()).collect();
        assert_eq!(joined, file);
        for n in 0..3 {
            assert_eq!(dl.next_dl(n, 10), NextDl::Acked);
        }
        assert_eq!(dl.next_dl(3, 10), NextDl::Completed);
    }

    #[test]
    fn the_window_waits_for_acks_and_retransmits_after_a_second() {
        let huff = Huffman::new();
        let mut dl = ServerDownload::new("main/big.pk3");
        let mut w = MsgWriter::new(&huff);
        dl.write(&mut w, 0, 20, |_| Ok(vec![0; 20 * BLOCK]));
        assert_eq!(blocks(&w.finish(), &huff).len(), 8, "one window");
        let mut w = MsgWriter::new(&huff);
        dl.write(&mut w, 500, 20, |_| unreachable!());
        assert!(blocks(&w.finish(), &huff).is_empty(), "nothing acked yet");
        let mut w = MsgWriter::new(&huff);
        dl.write(&mut w, 1600, 2, |_| unreachable!());
        let again = blocks(&w.finish(), &huff);
        assert_eq!(again.iter().map(|b| b.0).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(dl.next_dl(5, 1600), NextDl::Broken, "out of order");
    }

    #[test]
    fn a_refusal_names_the_file() {
        let huff = Huffman::new();
        let mut dl = ServerDownload::new("main/pak0.pk3");
        let mut w = MsgWriter::new(&huff);
        assert!(!dl.write(&mut w, 0, 1, |_| Err(Refusal::GamePak)));
        let got = blocks(&w.finish(), &huff);
        assert_eq!(got[0].1, -1);
        assert_eq!(got[0].2, b"EXE_CANTAUTODLGAMEPAK\x15main/pak0.pk3");
    }

    #[test]
    fn the_rate_sets_blocks_per_message() {
        assert_eq!(blocks_per_message(25000, 50), 1);
        assert_eq!(blocks_per_message(99999, 50), 3);
        assert_eq!(blocks_per_message(0, 50), 1);
    }
}
