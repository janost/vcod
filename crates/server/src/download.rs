//! Serving a pak to a client: `SV_WriteDownloadToClient` (cod_lnxded
//! 0x8086290) and the `nextdl` / `retransdl` handlers
//! (docs/research/cod11-server-handshake.md, "Serving a download"), and on
//! protocols 6 and 22 the www redirect (docs/research/cod15-protocol-diff.md
//! 14). The window, `nextdl`, `retransdl` and the `wwwdl` ladder are the
//! engine's (`crate::engine::download`); this is CoD1's first write.

use vcod_common::net::msg::MsgWriter;

use crate::engine::download::{BLOCK, DownloadWire, Transfer};
pub use crate::engine::download::{DownloadFile, NextDl, WwwReply, WwwState};

/// `svc_download`.
const SVC_DOWNLOAD: u8 = 6;

/// CoD1's `svc_download` shapes and its `wwwdl` drops (UO 0x808c298; 1.5b
/// has the same strings).
pub struct Cod1Download;

impl DownloadWire for Cod1Download {
    const UNEXPECTED: &'static str = "GMI_EXE_UNEXPECTEDWWWDOWLOADMESSAGE";
    const DISCONNECTED: &'static str = "GMI_EXE_DOWNLOADDISCONNECTED";

    /// The block number as a short, block 0's size, the length and bytes.
    fn write_block(w: &mut MsgWriter, block: u32, size: Option<i32>, data: Vec<u8>) {
        w.write_byte(SVC_DOWNLOAD);
        w.write_short(block as i16);
        if let Some(size) = size {
            w.write_long(size);
        }
        w.write_short(data.len() as i16);
        for b in data {
            w.write_byte(b);
        }
    }

    /// Block 0, size -1, the text.
    fn write_refusal(w: &mut MsgWriter, text: String) {
        w.write_byte(SVC_DOWNLOAD);
        w.write_short(0);
        w.write_long(-1);
        w.write_string(&text);
    }

    /// Block -1, `<base>/<name>`, the file's size and the flags, bit 0 the
    /// disconnected mode (UO 0x808c559).
    fn write_redirect(w: &mut MsgWriter, url: String, size: i32, flags: i32) {
        w.write_byte(SVC_DOWNLOAD);
        w.write_short(-1);
        w.write_string(&url);
        w.write_long(size);
        w.write_long(flags);
    }
}

/// Why a download is refused, the localized key the client shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// `EXE_AUTODL_SERVERDISABLED` (`_PURE` on a pure server).
    Disabled { pure: bool },
    /// `EXE_AUTODL_FILENOTONSERVER`.
    NotFound,
}

impl Refusal {
    fn key(self) -> &'static str {
        match self {
            Refusal::Disabled { pure: false } => "EXE_AUTODL_SERVERDISABLED",
            Refusal::Disabled { pure: true } => "EXE_AUTODL_SERVERDISABLED_PURE",
            Refusal::NotFound => "EXE_AUTODL_FILENOTONSERVER",
        }
    }
}

/// `sv_wwwDownload` on, for a protocol that has the redirect: where the
/// paks are (`sv_wwwBaseURL`) and whether the client fetches them off the
/// server (`sv_wwwDlDisconnected`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WwwConfig {
    pub base_url: String,
    pub disconnected: bool,
    /// The request after a failed www transfer goes over UDP (1.5b). UO's
    /// server clears the flag on the next message and redirects again on
    /// the one after (cod15-protocol-diff.md 14).
    pub udp_fallback: bool,
}

/// `SV_WWWDownload` (UO coduo_lnxded 0x808c298) on CoD1's strings.
pub fn wwwdl(state: &mut WwwState, sub: &str, client: &str, download: &str) -> WwwReply {
    crate::engine::download::wwwdl::<Cod1Download>(state, sub, client, download)
}

/// One client's transfer.
pub struct ServerDownload {
    /// As the client asked, e.g. `main/foo.pk3`.
    pub name: String,
    /// Its file opens on the first write.
    transfer: Transfer,
    /// Blocks have started going out over UDP; no redirect follows.
    streaming: bool,
}

impl ServerDownload {
    /// `SV_BeginDownload_f`: the name only; the file opens on the next write.
    pub fn new(name: &str) -> Self {
        ServerDownload {
            name: name.chars().take(63).collect(),
            transfer: Transfer::default(),
            streaming: false,
        }
    }

    /// `SV_WriteDownloadToClient`: opens the file on the first call (`open`
    /// answers with it or the refusal), then writes up to `budget` blocks
    /// inside the send window. False when the transfer is over (refused, or
    /// the file failed a read), so the caller drops it.
    ///
    /// With `www` set and a client that sent `cl_wwwDownload 1`, the
    /// redirect goes out instead, on every message until `wwwdl ack`, and
    /// nothing after it. A client without `cl_wwwDownload` gets the blocks:
    /// 1.5b's order, which opens the pak before the www branch (UO's
    /// stalls; cod15-protocol-diff.md 14). One whose last www transfer
    /// failed gets the blocks on 1.5b and the redirect again on UO.
    pub fn write(
        &mut self,
        w: &mut MsgWriter,
        now_ms: i32,
        budget: u32,
        open: impl FnOnce(&str) -> Result<DownloadFile, Refusal>,
        www: Option<&WwwConfig>,
        wants_www: bool,
        state: &mut WwwState,
    ) -> bool {
        if state.acked {
            return true;
        }
        if self.transfer.file().is_none() {
            match open(&self.name) {
                Ok(file) => self.transfer.start(file),
                Err(why) => {
                    log::info!("clientDownload: {:?} refused: {why:?}", self.name);
                    Cod1Download::write_refusal(w, format!("{}\x15{}", why.key(), self.name));
                    return false;
                }
            }
        }
        if !self.streaming {
            if let Some(www) = www.filter(|_| wants_www) {
                if !state.fallback {
                    let url = format!("{}/{}", www.base_url, self.name);
                    if !state.redirected {
                        log::info!("Redirecting client to {url}");
                    }
                    let size = self.transfer.file().map_or(0, DownloadFile::size);
                    Cod1Download::write_redirect(w, url, size, i32::from(www.disconnected));
                    state.redirected = true;
                    return true;
                }
                state.fallback = false;
                if !www.udp_fallback {
                    return true;
                }
                log::info!(
                    "falling back to regular downloading for failed file {}",
                    self.name
                );
            }
            log::info!("clientDownload: beginning {:?}", self.name);
            state.redirected = false;
            self.streaming = true;
        }
        match self
            .transfer
            .write::<Cod1Download>(w, now_ms, budget.max(1))
        {
            Ok(()) => true,
            Err(e) => {
                log::warn!("clientDownload: {:?} read failed: {e}", self.name);
                false
            }
        }
    }

    /// `SV_NextDownload_f` (0x8086168) for `nextdl <block>`; any block
    /// before the file opened is broken too.
    pub fn next_dl(&mut self, block: i32, now_ms: i32) -> NextDl {
        if self.transfer.file().is_none() {
            return NextDl::Broken;
        }
        let next = self.transfer.next_dl(block, now_ms);
        if next == NextDl::Completed {
            log::info!("clientDownload: {:?} completed", self.name);
        }
        next
    }

    /// `SV_RetransmitDownload_f` (0x8087a2c): start over from the block due.
    pub fn retransmit(&mut self, block: i32) {
        self.transfer.retransmit(block);
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
    use std::io::{Read, Seek, SeekFrom};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use vcod_common::net::huffman::Huffman;
    use vcod_common::net::msg::MsgReader;

    /// A write with no www redirect.
    fn udp(
        dl: &mut ServerDownload,
        w: &mut MsgWriter,
        now: i32,
        budget: u32,
        open: impl FnOnce(&str) -> Result<DownloadFile, Refusal>,
    ) -> bool {
        dl.write(w, now, budget, open, None, false, &mut WwwState::default())
    }

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
        assert!(udp(&mut dl, &mut w, 0, 8, |_| Ok(
            DownloadFile::from_bytes(file.clone())
        )));
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

    /// A source that counts the bytes read off it.
    struct Counting(
        std::io::Cursor<std::sync::Arc<[u8]>>,
        std::sync::Arc<AtomicUsize>,
    );

    impl Read for Counting {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.0.read(buf)?;
            self.1.fetch_add(n, Ordering::Relaxed);
            Ok(n)
        }
    }

    impl Seek for Counting {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.0.seek(pos)
        }
    }

    /// A transfer reads the blocks it sends off the handle: a `download`
    /// sent again and again costs a block a message, never the whole file,
    /// and a full window waits without reading.
    #[test]
    fn a_transfer_reads_only_the_blocks_it_sends() {
        let huff = Huffman::new();
        let bytes: std::sync::Arc<[u8]> = vec![7u8; 1 << 20].into();
        let read = std::sync::Arc::new(AtomicUsize::new(0));
        let open = |_: &str| {
            let src = Counting(std::io::Cursor::new(bytes.clone()), read.clone());
            Ok(DownloadFile::from_reader(src, bytes.len() as u64))
        };
        for n in 0..100 {
            let mut dl = ServerDownload::new("main/big.pk3");
            let mut w = MsgWriter::new(&huff);
            assert!(udp(&mut dl, &mut w, n * 50, 1, open));
            assert_eq!(blocks(&w.finish(), &huff).len(), 1);
        }
        assert_eq!(read.swap(0, Ordering::Relaxed), 100 * BLOCK);
        let mut dl = ServerDownload::new("main/big.pk3");
        for now in [0, 50, 100] {
            let mut w = MsgWriter::new(&huff);
            assert!(udp(&mut dl, &mut w, now, 20, open));
        }
        assert_eq!(read.load(Ordering::Relaxed), 8 * BLOCK, "one window");
    }

    #[test]
    fn a_refusal_names_the_file() {
        let huff = Huffman::new();
        let mut dl = ServerDownload::new("main/zzz_map.pk3");
        let mut w = MsgWriter::new(&huff);
        assert!(!udp(&mut dl, &mut w, 0, 1, |_| Err(Refusal::NotFound)));
        let got = blocks(&w.finish(), &huff);
        assert_eq!(got[0].1, -1);
        assert_eq!(got[0].2, b"EXE_AUTODL_FILENOTONSERVER\x15main/zzz_map.pk3");
    }

    #[test]
    fn the_rate_sets_blocks_per_message() {
        assert_eq!(blocks_per_message(25000, 50), 1);
        assert_eq!(blocks_per_message(99999, 50), 3);
        assert_eq!(blocks_per_message(0, 50), 1);
    }

    fn www(disconnected: bool) -> WwwConfig {
        WwwConfig {
            base_url: "http://127.0.0.1:31072".into(),
            disconnected,
            udp_fallback: true,
        }
    }

    /// Retail's redirect messages (cod15-protocol-diff.md 14): the 1.5b and
    /// UO servers redirecting `main/nsf_mp_j5outpost.pk3` (953499 bytes),
    /// each fixture the message's reliable acknowledge then its Huffman
    /// block. Ours writes the same block.
    #[test]
    fn the_redirect_is_retails_byte_for_byte() {
        let huff = Huffman::new();
        for (file, disconnected) in [
            ("net/www-redirect-uo.bin", false),
            ("net/www-redirect-uo-discon.bin", true),
            ("net/www-redirect-1.5.bin", false),
        ] {
            let retail = vcod_common::testing::fixture(file);
            let mut dl = ServerDownload::new("main/nsf_mp_j5outpost.pk3");
            let mut state = WwwState::default();
            let mut w = MsgWriter::new(&huff);
            let open = |_: &str| Ok(DownloadFile::from_bytes(vec![0; 953499]));
            assert!(dl.write(
                &mut w,
                0,
                4,
                open,
                Some(&www(disconnected)),
                true,
                &mut state
            ));
            w.write_byte(vcod_common::net::msg::SVC_EOF);
            assert_eq!(w.finish(), retail[4..], "{file}");
            assert!(state.redirected && !state.acked);
        }
    }

    #[test]
    fn the_redirect_repeats_until_the_ack_and_then_nothing_goes_out() {
        let huff = Huffman::new();
        let cfg = www(false);
        let mut dl = ServerDownload::new("main/foo.pk3");
        let mut state = WwwState::default();
        for _ in 0..2 {
            let mut w = MsgWriter::new(&huff);
            dl.write(
                &mut w,
                0,
                4,
                |_| Ok(DownloadFile::from_bytes(vec![1; 10])),
                Some(&cfg),
                true,
                &mut state,
            );
            let mut r = MsgReader::new(&w.finish(), &huff);
            assert_eq!((r.read_byte(), r.read_short()), (SVC_DOWNLOAD, -1));
            assert_eq!(r.read_string(), "http://127.0.0.1:31072/main/foo.pk3");
            assert_eq!((r.read_long(), r.read_long()), (10, 0));
        }
        assert_eq!(
            wwwdl(&mut state, "ACK", "vcod", "main/foo.pk3"),
            WwwReply::None
        );
        let mut w = MsgWriter::new(&huff);
        assert!(dl.write(
            &mut w,
            0,
            4,
            |_| unreachable!(),
            Some(&cfg),
            true,
            &mut state
        ));
        assert_eq!(w.bits_written(), 0);
        assert_eq!(
            wwwdl(&mut state, "done", "vcod", "main/foo.pk3"),
            WwwReply::Close
        );
        assert!(!state.acked && !state.fallback);
    }

    #[test]
    fn a_client_without_cl_wwwdownload_or_after_a_failure_gets_blocks() {
        let huff = Huffman::new();
        let cfg = www(false);
        let mut dl = ServerDownload::new("main/foo.pk3");
        let mut state = WwwState::default();
        let mut w = MsgWriter::new(&huff);
        dl.write(
            &mut w,
            0,
            4,
            |_| Ok(DownloadFile::from_bytes(vec![1; 10])),
            Some(&cfg),
            false,
            &mut state,
        );
        assert_eq!(blocks(&w.finish(), &huff)[0].1, 10, "block 0 with the size");

        // `fail` after the ack: the gamestate again, and the next request
        // streams.
        let mut state = WwwState {
            redirected: true,
            acked: true,
            fallback: false,
        };
        assert_eq!(
            wwwdl(&mut state, "chkfail", "vcod", "main/foo.pk3"),
            WwwReply::CloseAndGamestate
        );
        assert!(state.fallback);
        let mut dl = ServerDownload::new("main/foo.pk3");
        let mut w = MsgWriter::new(&huff);
        dl.write(
            &mut w,
            0,
            4,
            |_| Ok(DownloadFile::from_bytes(vec![1; 10])),
            Some(&cfg),
            true,
            &mut state,
        );
        assert_eq!(blocks(&w.finish(), &huff)[0].1, 10);
        assert_eq!(state, WwwState::default());
    }

    /// UO after a failed www transfer, live on coduo_lnxded 1.51b
    /// (2026-10-20, `crates/server/tests/fixtures/handshake/www-refuse-uo.txt`): the
    /// re-request's first message carries nothing and the next one
    /// redirects again; no block goes over UDP.
    #[test]
    fn uo_redirects_again_after_a_failed_www_transfer() {
        let huff = Huffman::new();
        let cfg = WwwConfig {
            udp_fallback: false,
            ..www(false)
        };
        let mut state = WwwState {
            redirected: true,
            acked: true,
            fallback: false,
        };
        assert_eq!(
            wwwdl(&mut state, "chkfail", "vcod", "main/foo.pk3"),
            WwwReply::CloseAndGamestate
        );
        let mut dl = ServerDownload::new("main/foo.pk3");
        let first_op = |w: MsgWriter| {
            let ops = w.finish();
            let mut r = MsgReader::new(&ops, &huff);
            (r.read_byte(), r.read_short())
        };
        let mut w = MsgWriter::new(&huff);
        let open = |_: &str| Ok(DownloadFile::from_bytes(vec![1; 10]));
        assert!(dl.write(&mut w, 0, 4, open, Some(&cfg), true, &mut state));
        assert_ne!(first_op(w).0, SVC_DOWNLOAD, "the first message is empty");
        let mut w = MsgWriter::new(&huff);
        assert!(dl.write(&mut w, 50, 4, open, Some(&cfg), true, &mut state));
        assert_eq!(first_op(w), (SVC_DOWNLOAD, -1), "the redirect again");
        assert!(state.redirected && !state.fallback && !state.acked);
    }

    #[test]
    fn wwwdl_out_of_turn_drops_the_client() {
        let unexpected = WwwReply::Drop("GMI_EXE_UNEXPECTEDWWWDOWLOADMESSAGE");
        let mut state = WwwState::default();
        assert_eq!(
            wwwdl(&mut state, "ack", "c", "f"),
            unexpected,
            "no redirect yet"
        );
        state.redirected = true;
        assert_eq!(
            wwwdl(&mut state, "done", "c", "f"),
            unexpected,
            "before the ack"
        );
        assert_eq!(
            wwwdl(&mut state, "bbl8r", "c", "f"),
            WwwReply::Drop("GMI_EXE_DOWNLOADDISCONNECTED")
        );
        state.acked = true;
        assert_eq!(wwwdl(&mut state, "what", "c", "f"), unexpected, "unknown");
    }
}
