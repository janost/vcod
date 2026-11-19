//! Serving a file to a client, as the three games' `SV_WriteDownloadToClient`
//! do alike (cod_lnxded 0x8086290, cod2_lnxded 0x808fdc2, cod4_lnxded
//! 0x816f774): 0x800-byte blocks into an 8-block window resent after
//! 1000 ms without an ack, the `nextdl` and `retransdl` handlers, and the
//! `wwwdl` ladder after a www redirect (`SV_WWWDownload`, UO 0x808c298,
//! CoD2 0x808f97c, CoD4 0x8170ffa). What each game writes on its first
//! call for a name (refusals, redirect, their order) stays with the game:
//! `crate::download` (CoD1), `cod2/house.rs`, `cod4/download.rs`.

use std::io::{Read, Seek, SeekFrom};
use std::marker::PhantomData;
use std::path::Path;

use vcod_common::net::codec::MsgWriter;

/// All three servers read their blocks 0x800 bytes at a time.
pub const BLOCK: usize = 2048;
/// Blocks in flight before an ack (CoD4's `cmp 7`s at 0x816f8e7).
const WINDOW: u32 = 8;
/// Unacked blocks go again after this long (CoD2 `< 0x3e9`, CoD4 `cmp 0x3e8`
/// at 0x816fb5f).
const RETRANSMIT_MS: i32 = 1000;

/// What the engine asks of a game: its block's encoding and its `wwwdl`
/// drop reasons.
pub trait DownloadWire {
    /// The drop for a `wwwdl` before the redirect, before the ack, or unknown.
    const UNEXPECTED: &'static str;
    /// The drop for `wwwdl bbl8r`.
    const DISCONNECTED: &'static str;
    /// `wwwdl done`, `fail` and `chkfail` clear the redirect mark too (CoD4
    /// 0x817110a, 0x8171184); CoD1 and CoD2 keep it until blocks start.
    const END_CLEARS_REDIRECT: bool = false;

    /// One block, its `svc_download` op first: its number, block 0's file
    /// size, its bytes.
    fn write_block(w: &mut MsgWriter, block: u32, size: Option<i32>, data: Vec<u8>);
    /// The refusal, `<key>\x15<name>`.
    fn write_refusal(w: &mut MsgWriter, text: String);
    /// The www redirect: the URL, the file's size, the flags (bit 0
    /// `sv_wwwDlDisconnected`).
    fn write_redirect(w: &mut MsgWriter, url: String, size: i32, flags: i32);
}

trait Source: Read + Seek + Send {}
impl<T: Read + Seek + Send> Source for T {}

/// A file a transfer serves: its length and the open handle each 0x800-byte
/// block is read from as it goes out, so a transfer holds one handle and
/// never the file's bytes. Block `i` is bytes `i * 0x800 ..`, and the block
/// past the last byte is the empty one that ends a transfer.
pub struct DownloadFile {
    src: Box<dyn Source>,
    len: u64,
}

impl DownloadFile {
    /// The regular file at `path`; `None` when it does not open.
    pub fn open(path: &Path) -> Option<Self> {
        let file = std::fs::File::open(path).ok()?;
        let meta = file.metadata().ok()?;
        meta.is_file().then(|| DownloadFile {
            src: Box::new(file),
            len: meta.len(),
        })
    }

    #[cfg(test)]
    pub fn from_reader(src: impl Read + Seek + Send + 'static, len: u64) -> Self {
        DownloadFile {
            src: Box::new(src),
            len,
        }
    }

    #[cfg(test)]
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        let len = bytes.len() as u64;
        Self::from_reader(std::io::Cursor::new(bytes), len)
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The size block 0 and a redirect carry.
    pub fn size(&self) -> i32 {
        self.len as i32
    }

    /// Blocks in the transfer, the empty end block included.
    pub fn blocks(&self) -> u32 {
        self.len.div_ceil(BLOCK as u64) as u32 + 1
    }

    /// Block `i` is the empty one past the last byte.
    pub fn is_end(&self, i: u32) -> bool {
        u64::from(i) * BLOCK as u64 >= self.len
    }

    /// Block `i`'s bytes off the handle; an error when the file came up
    /// short of its length.
    pub fn read_block(&mut self, i: u32) -> std::io::Result<Vec<u8>> {
        let start = (u64::from(i) * BLOCK as u64).min(self.len);
        let mut buf = vec![0; (self.len - start).min(BLOCK as u64) as usize];
        if !buf.is_empty() {
            self.src.seek(SeekFrom::Start(start))?;
            self.src.read_exact(&mut buf)?;
        }
        Ok(buf)
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

/// The window over an open file: `downloadClientBlock`,
/// `downloadCurrentBlock`, `downloadSendTime`.
#[derive(Default)]
pub struct Transfer {
    /// `None` until the game opens it.
    file: Option<DownloadFile>,
    /// The next block the client has to ack.
    acked: u32,
    /// The next block to put on the wire.
    current: u32,
    /// The server clock of the last send or ack.
    send_time: i32,
}

impl Transfer {
    pub fn file(&self) -> Option<&DownloadFile> {
        self.file.as_ref()
    }

    /// Blocks from 0 out of `file`.
    pub fn start(&mut self, file: DownloadFile) {
        self.file = Some(file);
        self.acked = 0;
        self.current = 0;
    }

    /// The file closed; the counters stay until the next [`Self::start`].
    pub fn close(&mut self) {
        self.file = None;
    }

    /// Up to `budget` blocks inside the window, the window again from the
    /// acked block once it sat a second unacked. Nothing without a file; an
    /// error when the file failed a read.
    pub fn write<G: DownloadWire>(
        &mut self,
        w: &mut MsgWriter,
        now_ms: i32,
        budget: u32,
    ) -> std::io::Result<()> {
        let Some(file) = self.file.as_mut() else {
            return Ok(());
        };
        let window = (self.acked + WINDOW).min(file.blocks());
        for _ in 0..budget {
            if self.acked == window {
                break;
            }
            if self.current == window {
                if now_ms.wrapping_sub(self.send_time) <= RETRANSMIT_MS {
                    break;
                }
                self.current = self.acked;
            }
            let block = file.read_block(self.current)?;
            let size = (self.current == 0).then(|| file.size());
            G::write_block(w, self.current, size, block);
            self.current += 1;
            self.send_time = now_ms;
        }
        Ok(())
    }

    /// `SV_NextDownload_f` for `nextdl <block>`. With no file open the due
    /// block still moves on, as CoD2's and CoD4's do.
    pub fn next_dl(&mut self, block: i32, now_ms: i32) -> NextDl {
        if block != self.acked as i32 {
            return NextDl::Broken;
        }
        if self.file.as_ref().is_some_and(|f| f.is_end(self.acked)) {
            return NextDl::Completed;
        }
        self.acked += 1;
        self.send_time = now_ms;
        NextDl::Acked
    }

    /// `SV_RetransmitDownload_f` for `retransdl <block>`: start over from
    /// the block due.
    pub fn retransmit(&mut self, block: i32) {
        if block == self.acked as i32 {
            self.current = self.acked;
        }
    }
}

/// One client's www state (UO client +0x10c08, CoD2 +0x20e0c, CoD4
/// +0x21448, each with the next two words). Retail keeps it on the client,
/// across transfers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WwwState {
    /// A redirect went out; `wwwdl` is refused before one.
    pub redirected: bool,
    /// `wwwdl ack` came back: nothing more is written for the transfer.
    pub acked: bool,
    /// The last www transfer failed (`wwwdl fail`, `chkfail`): the next one
    /// goes over UDP, or on UO waits a message.
    pub fallback: bool,
}

/// What a `wwwdl` subcommand does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WwwReply {
    /// Nothing more to do.
    None,
    /// The transfer is over; close it.
    Close,
    /// Close it and resend the gamestate (`fail`, `chkfail`).
    CloseAndGamestate,
    /// Drop the client with this reason.
    Drop(&'static str),
}

/// `SV_WWWDownload`: any subcommand before a redirect, an unknown one, or
/// anything but `ack` and `bbl8r` before the ack drops the client. `client`
/// and `download` name them in the log.
pub fn wwwdl<G: DownloadWire>(
    state: &mut WwwState,
    sub: &str,
    client: &str,
    download: &str,
) -> WwwReply {
    if !state.redirected {
        log::warn!("SV_WWWDownload: unexpected wwwdl '{sub}' for client '{client}'");
        return WwwReply::Drop(G::UNEXPECTED);
    }
    let is = |s: &str| sub.eq_ignore_ascii_case(s);
    if is("ack") {
        if state.acked {
            log::warn!("WARNING: dupe wwwdl ack from client '{client}'");
        }
        state.acked = true;
        return WwwReply::None;
    }
    if is("bbl8r") {
        return WwwReply::Drop(G::DISCONNECTED);
    }
    if !state.acked {
        log::warn!("SV_WWWDownload: unexpected wwwdl '{sub}' for client '{client}'");
        return WwwReply::Drop(G::UNEXPECTED);
    }
    let failed = if is("done") {
        false
    } else if is("chkfail") {
        log::warn!(
            "WARNING: client '{client}' reports that the redirect download for '{download}' \
             had wrong checksum.\n         you should check your download redirect configuration."
        );
        true
    } else if is("fail") {
        true
    } else {
        log::warn!("SV_WWWDownload: unknown wwwdl subcommand '{sub}' for client '{client}'");
        return WwwReply::Drop(G::UNEXPECTED);
    };
    state.acked = false;
    if G::END_CLEARS_REDIRECT {
        state.redirected = false;
    }
    if failed {
        state.fallback = true;
        WwwReply::CloseAndGamestate
    } else {
        WwwReply::Close
    }
}

/// What a CoD2 or CoD4 `wwwdl` asks of the server once [`Download::wwwdl`]
/// closed what it ends.
#[derive(Debug, PartialEq, Eq)]
pub enum Www {
    None,
    /// `fail` / `chkfail`: the gamestate again.
    Gamestate,
    Drop(&'static str),
}

/// One CoD2 or CoD4 client's transfer, kept on the client (CoD2 +0x20c68 ..
/// +0x20e14, CoD4 +0x212a0 .. +0x21450). Each game's first write is its
/// own `impl Download<G>`.
pub struct Download<G> {
    /// `downloadName`; empty when none.
    pub name: String,
    pub(crate) transfer: Transfer,
    pub(crate) www: WwwState,
    game: PhantomData<fn() -> G>,
}

impl<G> Default for Download<G> {
    fn default() -> Self {
        Download {
            name: String::new(),
            transfer: Transfer::default(),
            www: WwwState::default(),
            game: PhantomData,
        }
    }
}

impl<G: DownloadWire> Download<G> {
    pub fn active(&self) -> bool {
        !self.name.is_empty()
    }

    /// `SV_CloseDownload`.
    pub fn close(&mut self) {
        self.name.clear();
        self.transfer.close();
    }

    /// `download <name>`: the old transfer closed, the name kept to 63
    /// characters.
    pub fn begin(&mut self, name: &str) {
        self.close();
        self.name = name.chars().take(63).collect();
    }

    /// `nextdl <n>`. False for any block but the one due: the caller drops
    /// the client with `broken download`.
    pub fn next_dl(&mut self, block: i32, svs_ms: i32) -> bool {
        match self.transfer.next_dl(block, svs_ms) {
            NextDl::Broken => return false,
            NextDl::Completed => {
                log::info!("clientDownload: file {:?} completed", self.name);
                self.close();
            }
            NextDl::Acked => {}
        }
        true
    }

    /// `retransdl <n>`: start over from the block due.
    pub fn retransmit(&mut self, block: i32) {
        self.transfer.retransmit(block);
    }

    /// `wwwdl <sub>` from the client named `client`.
    pub fn wwwdl(&mut self, sub: &str, client: &str) -> Www {
        match wwwdl::<G>(&mut self.www, sub, client, &self.name) {
            WwwReply::None => Www::None,
            WwwReply::Close => {
                self.close();
                Www::None
            }
            WwwReply::CloseAndGamestate => {
                self.close();
                Www::Gamestate
            }
            WwwReply::Drop(why) => Www::Drop(why),
        }
    }

    /// The blocks after the game's first write; a failed read closes the
    /// transfer.
    pub(crate) fn write_blocks(&mut self, w: &mut MsgWriter, svs_ms: i32, budget: u32) {
        if let Err(e) = self.transfer.write::<G>(w, svs_ms, budget) {
            log::warn!("clientDownload: {:?} read failed: {e}", self.name);
            self.close();
        }
    }

    /// The refusals both games test on a first write, in this order:
    /// `sv_allowDownload` off (`_PURE` with `sv_pure`), a stock iwd
    /// (`FS_iwIwd(name, "main")`), a file that does not open or is empty.
    /// The file, or `None` with the refusal written and the name cleared.
    pub(crate) fn open_checked(
        &mut self,
        w: &mut MsgWriter,
        cvars: &DownloadCvars,
        open: &dyn Fn(&str) -> Option<DownloadFile>,
    ) -> Option<DownloadFile> {
        let key = if !cvars.allow {
            log::info!("clientDownload: {:?} download disabled", self.name);
            if cvars.pure {
                "EXE_AUTODL_SERVERDISABLED_PURE"
            } else {
                "EXE_AUTODL_SERVERDISABLED"
            }
        } else if vcod_common::pak_checksum::fs_iw_iwd(&self.name, "main") {
            log::info!(
                "clientDownload: {:?} cannot download IW iwd files",
                self.name
            );
            "EXE_CANTAUTODLGAMEPAK"
        } else if let Some(file) = open(&self.name).filter(|f| !f.is_empty()) {
            return Some(file);
        } else {
            log::info!("clientDownload: {:?} file not found on server", self.name);
            "EXE_AUTODL_FILENOTONSERVER"
        };
        G::write_refusal(w, format!("{key}\x15{}", self.name));
        self.name.clear();
        None
    }

    /// `SV_WWWRedirectClient` (CoD2 0x808fc0c, CoD4 0x816f674):
    /// `<sv_wwwBaseURL>/<name>`, `file`'s size and the disconnected flag,
    /// then the name cleared.
    pub(crate) fn redirect(
        &mut self,
        w: &mut MsgWriter,
        cvars: &DownloadCvars,
        file: &DownloadFile,
    ) {
        let url = format!("{}/{}", cvars.www_base_url, self.name);
        log::info!("Redirecting client to {url}");
        self.www.redirected = true;
        G::write_redirect(w, url, file.size(), i32::from(cvars.www_disconnected));
        self.name.clear();
    }
}

/// The dvars CoD2's and CoD4's `SV_WriteDownloadToClient` read (CoD4
/// 0x8173319, 0x81738c9..0x817393a).
#[derive(Clone, Debug, Default)]
pub struct DownloadCvars {
    /// `sv_allowDownload`.
    pub allow: bool,
    /// `sv_pure`: the refusal's `_PURE` key.
    pub pure: bool,
    /// `sv_wwwDownload`.
    pub www: bool,
    pub www_base_url: String,
    pub www_disconnected: bool,
}

/// CoD2's and CoD4's blocks a message: the rate over one snapshot interval
/// in 2 KiB units, rounded up past a whole one; 1 at rate 0 (CoD2
/// 0x808fdc2, CoD4 0x816fa21..0x816fa93).
pub fn blocks_per_message(rate: i32, snapshot_msec: i32) -> u32 {
    if rate == 0 {
        return 1;
    }
    ((rate * snapshot_msec / 1000 + 0x800) >> 11).max(1) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::huffman::Huffman;
    use vcod_common::net::msg::MsgReader;

    /// A wire whose blocks read back as `(block, size, len)` triples.
    struct Plain;

    impl DownloadWire for Plain {
        const UNEXPECTED: &'static str = "unexpected";
        const DISCONNECTED: &'static str = "disconnected";
        fn write_block(w: &mut MsgWriter, block: u32, size: Option<i32>, data: Vec<u8>) {
            w.write_long(block as i32);
            w.write_long(size.unwrap_or(-1));
            w.write_short(data.len() as i16);
            for b in data {
                w.write_byte(b);
            }
        }
        fn write_refusal(_: &mut MsgWriter, _: String) {}
        fn write_redirect(_: &mut MsgWriter, _: String, _: i32, _: i32) {}
    }

    fn blocks(w: MsgWriter) -> Vec<(i32, i32, usize)> {
        let ops = w.into_ops();
        let mut r = MsgReader::from_plain(ops);
        let mut out = Vec::new();
        loop {
            let block = r.read_long();
            if r.is_overflowed() {
                return out;
            }
            let size = r.read_long();
            let len = r.read_short() as usize;
            (0..len).for_each(|_| {
                r.read_byte();
            });
            out.push((block, size, len));
        }
    }

    fn started(len: usize) -> Transfer {
        let mut t = Transfer::default();
        t.start(DownloadFile::from_bytes(vec![0; len]));
        t
    }

    #[test]
    fn the_window_waits_for_acks_and_retransmits_after_a_second() {
        let huff = Huffman::new();
        let mut t = started(20 * BLOCK);
        let mut w = MsgWriter::new(&huff);
        t.write::<Plain>(&mut w, 0, 20).unwrap();
        assert_eq!(blocks(w).len(), 8, "one window");
        let mut w = MsgWriter::new(&huff);
        t.write::<Plain>(&mut w, 1000, 20).unwrap();
        assert!(blocks(w).is_empty(), "nothing acked yet");
        let mut w = MsgWriter::new(&huff);
        t.write::<Plain>(&mut w, 1001, 2).unwrap();
        let again: Vec<i32> = blocks(w).iter().map(|b| b.0).collect();
        assert_eq!(again, [0, 1]);
        assert_eq!(t.next_dl(5, 1001), NextDl::Broken, "out of order");
        t.retransmit(0);
        let mut w = MsgWriter::new(&huff);
        t.write::<Plain>(&mut w, 1002, 1).unwrap();
        assert_eq!(blocks(w)[0].0, 0, "retransdl rewinds to the due block");
    }

    /// A window sent just before the server clock wraps goes again a
    /// second later, past the wrap.
    #[test]
    fn the_resend_clock_wraps() {
        let huff = Huffman::new();
        let mut t = started(20 * BLOCK);
        let t0 = i32::MAX - 100;
        t.write::<Plain>(&mut MsgWriter::new(&huff), t0, 20)
            .unwrap();
        let mut w = MsgWriter::new(&huff);
        t.write::<Plain>(&mut w, t0.wrapping_add(1500), 2).unwrap();
        let again: Vec<i32> = blocks(w).iter().map(|b| b.0).collect();
        assert_eq!(again, [0, 1]);
    }

    #[test]
    fn a_file_opens_by_length_and_a_short_one_fails_its_read() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let path = dir.join("f.pk3");
        let bytes: Vec<u8> = (0..5000u32).map(|i| i as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        assert!(DownloadFile::open(&dir).is_none(), "a directory");
        let mut f = DownloadFile::open(&path).unwrap();
        assert_eq!((f.size(), f.blocks()), (5000, 4));
        assert!(!f.is_end(2) && f.is_end(3));
        assert_eq!(f.read_block(2).unwrap(), bytes[2 * BLOCK..]);
        assert!(f.read_block(3).unwrap().is_empty());
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(3000)
            .unwrap();
        assert_eq!(f.read_block(0).unwrap(), bytes[..BLOCK]);
        assert!(f.read_block(1).is_err(), "cut short after the open");
    }

    #[test]
    fn wwwdl_out_of_turn_drops_the_client() {
        let unexpected = WwwReply::Drop(Plain::UNEXPECTED);
        let mut state = WwwState::default();
        let www = |state: &mut WwwState, sub| wwwdl::<Plain>(state, sub, "c", "f");
        assert_eq!(www(&mut state, "ack"), unexpected, "no redirect yet");
        state.redirected = true;
        assert_eq!(www(&mut state, "done"), unexpected, "before the ack");
        assert_eq!(
            www(&mut state, "bbl8r"),
            WwwReply::Drop(Plain::DISCONNECTED)
        );
        state.acked = true;
        assert_eq!(www(&mut state, "what"), unexpected, "unknown");
        assert_eq!(www(&mut state, "ACK"), WwwReply::None);
        assert_eq!(www(&mut state, "chkfail"), WwwReply::CloseAndGamestate);
        assert!(state.fallback && !state.acked && state.redirected);
    }

    #[test]
    fn blocks_per_message_rounds_past_a_whole_block() {
        assert_eq!(blocks_per_message(25000, 50), 1);
        assert_eq!(blocks_per_message(90000, 33), 2);
        assert_eq!(blocks_per_message(0, 50), 1);
    }
}
