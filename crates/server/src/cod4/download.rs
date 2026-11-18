//! A 1.7 client's UDP download and the www redirect
//! (docs/research/cod4-handshake.md 11): `SV_WriteDownloadToClient`
//! (0x816f774) and the `download`, `nextdl`, `stopdl`, `donedl`,
//! `retransdl` and `wwwdl` commands.

use std::path::{Path, PathBuf};

use vcod_cod4::net::download::{DownloadMsg, fs_iw_iwd, safe_rel_path};
use vcod_common::net::codec::MsgWriter;

use crate::download::DownloadFile;

/// The window (`& 7`, the `cmp 7`s at 0x816f8e7); its 0x800-byte blocks
/// are [`DownloadFile`]'s.
const WINDOW: u32 = 8;
/// A window's blocks go again after this long without a `nextdl`
/// (`cmp 0x3e8` at 0x816fb5f).
const RETRANSMIT_MS: i32 = 1000;

/// The dvars `SV_WriteDownloadToClient` reads.
#[derive(Clone, Debug, Default)]
pub struct DownloadCvars {
    /// `sv_allowDownload` (0x13ed7c10).
    pub allow: bool,
    /// `sv_pure` (0x13ed7c50): the refusal's `_PURE` key.
    pub pure: bool,
    /// `sv_wwwDownload` (0x13ed7c88).
    pub www: bool,
    pub www_base_url: String,
    pub www_disconnected: bool,
}

/// One client's transfer (client +0x212a0 .. +0x21450).
#[derive(Default)]
pub struct Download {
    /// `downloadName` (+0x212a4); empty when none.
    pub name: String,
    /// The open file; `None` until opened.
    file: Option<DownloadFile>,
    /// `downloadClientBlock` (+0x212f0): the next block the client acks.
    acked: u32,
    /// `downloadCurrentBlock` (+0x212f8): the next block written.
    current: u32,
    /// `downloadSendTime` (+0x21340).
    send_time: i32,
    /// A redirect went out (+0x21448).
    redirected: bool,
    /// `wwwdl ack` came (+0x2144c): nothing more is written.
    www_acked: bool,
    /// The last www transfer failed (+0x21450): the next goes over UDP.
    www_fallback: bool,
}

/// What a `wwwdl` asks of the server.
#[derive(Debug, PartialEq, Eq)]
pub enum Www {
    None,
    /// `fail` / `chkfail`: the gamestate again.
    Gamestate,
    Drop(&'static str),
}

impl Download {
    pub fn active(&self) -> bool {
        !self.name.is_empty()
    }

    /// `SV_CloseDownload`.
    pub fn close(&mut self) {
        self.name.clear();
        self.file = None;
    }

    /// `download <name>` (0x8172068): the old transfer closed, the name
    /// kept to 63 characters.
    pub fn begin(&mut self, name: &str) {
        self.close();
        self.name = name.chars().take(63).collect();
    }

    /// `nextdl <n>` (0x81721c6). False for any block but the one due: the
    /// caller drops the client with `broken download`.
    pub fn next_dl(&mut self, block: i32, svs_ms: i32) -> bool {
        if block != self.acked as i32 {
            return false;
        }
        if self.file.as_ref().is_some_and(|f| f.is_end(self.acked)) {
            log::info!("clientDownload: file {:?} completed", self.name);
            self.close();
        } else {
            self.send_time = svs_ms;
            self.acked += 1;
        }
        true
    }

    /// `retransdl <n>` (0x81707aa): start over from the block due.
    pub fn retransmit(&mut self, block: i32) {
        if block == self.acked as i32 {
            self.current = self.acked;
        }
    }

    /// `wwwdl <sub>` (0x8170ffa), as CoD2's `SV_WWWDownload_f` reads its
    /// strings (0x8230b14..0x8230cbc).
    pub fn wwwdl(&mut self, sub: &str) -> Www {
        let is = |s: &str| sub.eq_ignore_ascii_case(s);
        if !self.redirected {
            return Www::Drop("Unexpected www download message.");
        }
        if is("ack") {
            self.www_acked = true;
            return Www::None;
        }
        if is("bbl8r") {
            return Www::Drop("Client dropped to download files.");
        }
        if !self.www_acked {
            return Www::Drop("Unexpected www download message.");
        }
        let failed = if is("done") {
            false
        } else if is("fail") || is("chkfail") {
            true
        } else {
            return Www::Drop("Unexpected www download message.");
        };
        self.close();
        self.www_acked = false;
        self.redirected = false;
        if failed {
            self.www_fallback = true;
            Www::Gamestate
        } else {
            Www::None
        }
    }

    /// `SV_WriteDownloadToClient` (0x816f774): on the first call the
    /// refusals (`sv_allowDownload`, a stock iwd, no file), then the www
    /// redirect when the server and the client want one, then up to `budget`
    /// blocks inside the window. `open` opens a file, `None` when there is
    /// none.
    pub fn write(
        &mut self,
        w: &mut MsgWriter,
        svs_ms: i32,
        budget: u32,
        cvars: &DownloadCvars,
        wants_www: bool,
        open: &dyn Fn(&str) -> Option<DownloadFile>,
    ) {
        if self.name.is_empty() || self.www_acked {
            return;
        }
        if self.file.is_none() {
            let refuse = |w: &mut MsgWriter, key: &str, name: &str| {
                DownloadMsg::Refused(format!("{key}\x15{name}")).write(w);
            };
            if !cvars.allow {
                log::info!("clientDownload: {:?} download disabled", self.name);
                let key = if cvars.pure {
                    "EXE_AUTODL_SERVERDISABLED_PURE"
                } else {
                    "EXE_AUTODL_SERVERDISABLED"
                };
                refuse(w, key, &self.name);
                self.name.clear();
                return;
            }
            if fs_iw_iwd(&self.name, "main") {
                log::info!(
                    "clientDownload: {:?} cannot download IW iwd files",
                    self.name
                );
                refuse(w, "EXE_CANTAUTODLGAMEPAK", &self.name);
                self.name.clear();
                return;
            }
            let Some(file) = open(&self.name).filter(|f| !f.is_empty()) else {
                log::info!("clientDownload: {:?} file not found on server", self.name);
                refuse(w, "EXE_AUTODL_FILENOTONSERVER", &self.name);
                self.name.clear();
                return;
            };
            if cvars.www && wants_www {
                if self.www_fallback {
                    // 0x816fc72: the fallback flag cleared and nothing
                    // written this message; the blocks start on the next.
                    self.www_fallback = false;
                    self.start(file);
                    return;
                }
                // `SV_WWWRedirectClient` (0x816f674).
                let url = format!("{}/{}", cvars.www_base_url, self.name);
                log::info!("Redirecting client to {url}");
                self.redirected = true;
                DownloadMsg::Redirect {
                    url,
                    size: file.size(),
                    flags: i32::from(cvars.www_disconnected),
                }
                .write(w);
                self.name.clear();
                return;
            }
            log::info!("clientDownload: beginning {:?}", self.name);
            self.start(file);
        }
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let window = (self.acked + WINDOW).min(file.blocks());
        for _ in 0..budget {
            if self.acked == window {
                break;
            }
            if self.current == window {
                if svs_ms - self.send_time <= RETRANSMIT_MS {
                    return;
                }
                self.current = self.acked;
            }
            let block = self.current;
            let data = match file.read_block(block) {
                Ok(data) => data,
                Err(e) => {
                    log::warn!("clientDownload: {:?} read failed: {e}", self.name);
                    self.close();
                    return;
                }
            };
            DownloadMsg::Block {
                block: block as i32,
                size: (block == 0).then(|| file.size()),
                data,
            }
            .write(w);
            self.current += 1;
            self.send_time = svs_ms;
        }
    }

    fn start(&mut self, file: DownloadFile) {
        self.redirected = false;
        self.file = Some(file);
        self.acked = 0;
        self.current = 0;
    }
}

/// A file a client may fetch: an `.iwd` or `.ff` that sits under `root` at
/// its name. Retail's `FS_SV_FOpenFileRead` serves any file under the base
/// and home paths; vcod serves those two kinds alone.
pub fn servable(root: &Path, name: &str) -> Option<PathBuf> {
    let lower = name.to_ascii_lowercase();
    if !lower.ends_with(".iwd") && !lower.ends_with(".ff") {
        return None;
    }
    Some(root.join(safe_rel_path(name)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_cod4::net::parse::UNUSED_HUFF;
    use vcod_common::net::codec::MsgReader;

    fn msgs(w: MsgWriter) -> Vec<DownloadMsg> {
        let mut r = MsgReader::from_plain(w.into_ops());
        let mut out = Vec::new();
        while r.read_byte() == 5 && !r.is_overflowed() {
            out.push(DownloadMsg::read(&mut r));
        }
        out
    }

    fn cvars(allow: bool) -> DownloadCvars {
        DownloadCvars {
            allow,
            pure: true,
            ..DownloadCvars::default()
        }
    }

    #[test]
    fn refusals_come_in_retail_order() {
        let open = |n: &str| (n == "mods/m/a.iwd").then(|| DownloadFile::from_bytes(vec![1u8; 10]));
        for (allow, name, key) in [
            (false, "mods/m/a.iwd", "EXE_AUTODL_SERVERDISABLED_PURE"),
            (true, "main/iw_00", "EXE_CANTAUTODLGAMEPAK"),
            (true, "mods/m/b.iwd", "EXE_AUTODL_FILENOTONSERVER"),
        ] {
            let mut d = Download::default();
            d.begin(name);
            let mut w = MsgWriter::new(&UNUSED_HUFF);
            d.write(&mut w, 0, 1, &cvars(allow), false, &open);
            assert_eq!(msgs(w), [DownloadMsg::Refused(format!("{key}\x15{name}"))]);
            assert!(!d.active());
        }
    }

    #[test]
    fn a_file_goes_in_blocks_acked_one_by_one() {
        let bytes: Vec<u8> = (0..5000u32).map(|i| i as u8).collect();
        let file = bytes.clone();
        let open = move |_: &str| Some(DownloadFile::from_bytes(file.clone()));
        let mut d = Download::default();
        d.begin("mods/m/a.iwd");
        let mut got = Vec::new();
        let mut t = 0;
        while d.active() {
            let mut w = MsgWriter::new(&UNUSED_HUFF);
            d.write(&mut w, t, 1, &cvars(true), false, &open);
            for m in msgs(w) {
                let DownloadMsg::Block { block, data, size } = m else {
                    panic!("{m:?}");
                };
                assert_eq!(size.is_some(), block == 0);
                got.extend(data);
                assert!(d.next_dl(block, t));
            }
            t += 50;
        }
        assert_eq!(got, bytes);
        assert!(!d.next_dl(9, t) || d.name.is_empty());
    }

    #[test]
    fn www_redirects_only_a_file_it_has() {
        let open = |_: &str| Some(DownloadFile::from_bytes(vec![0u8; 3]));
        let cv = DownloadCvars {
            www: true,
            www_base_url: "http://h/x".into(),
            ..cvars(true)
        };
        let mut d = Download::default();
        d.begin("mods/m/a.iwd");
        let mut w = MsgWriter::new(&UNUSED_HUFF);
        d.write(&mut w, 0, 1, &cv, true, &open);
        assert_eq!(
            msgs(w),
            [DownloadMsg::Redirect {
                url: "http://h/x/mods/m/a.iwd".into(),
                size: 3,
                flags: 0
            }]
        );
        assert_eq!(d.wwwdl("ack"), Www::None);
        assert_eq!(d.wwwdl("fail"), Www::Gamestate);
        d.begin("mods/m/a.iwd");
        let mut w = MsgWriter::new(&UNUSED_HUFF);
        d.write(&mut w, 0, 1, &cv, true, &open);
        assert!(msgs(w).is_empty());
        let mut w = MsgWriter::new(&UNUSED_HUFF);
        d.write(&mut w, 50, 1, &cv, true, &open);
        assert!(matches!(msgs(w)[..], [DownloadMsg::Block { block: 0, .. }]));
    }

    /// Retail's blocks for ModWarfare's iwd and mod.ff, one a message at
    /// `rate 25000` under `sv_maxRate 5000` (the decoded capture,
    /// cod4-handshake.md 11), against ours for files of those sizes.
    #[test]
    fn blocks_match_the_retail_capture() {
        let text = include_str!("../../../cod4/tests/fixtures/net/download-1.7.txt");
        let section = text.split("# allow list").next().unwrap();
        let retail: Vec<String> = section
            .lines()
            .filter_map(|l| l.split_once(" download block "))
            .map(|(_, rest)| format!("block {rest}"))
            .collect();
        let budget = vcod_cod4::net::download::blocks_per_message(5000, 50);
        assert_eq!(budget, 1);
        let mut ours = Vec::new();
        for (name, size) in [("mods/m/z.iwd", 178745), ("mods/m/mod.ff", 34310)] {
            let open = move |_: &str| Some(DownloadFile::from_bytes(vec![0u8; size]));
            let mut d = Download::default();
            d.begin(name);
            let mut t = 0;
            while d.active() {
                let mut w = MsgWriter::new(&UNUSED_HUFF);
                d.write(&mut w, t, budget, &cvars(true), false, &open);
                for m in msgs(w) {
                    let DownloadMsg::Block { block, size, data } = m else {
                        panic!("{m:?}");
                    };
                    ours.push(format!("block {block} size {size:?} len {}", data.len()));
                    assert!(d.next_dl(block, t));
                }
                t += 250;
            }
        }
        assert_eq!(ours, retail);
    }

    #[test]
    fn only_iwds_and_ffs_under_the_root() {
        let root = Path::new("r");
        assert!(servable(root, "mods/m/a.iwd").is_some());
        assert!(servable(root, "usermaps/mp_x/mp_x.ff").is_some());
        assert!(servable(root, "mods/m/server.cfg").is_none());
        assert!(servable(root, "../a.iwd").is_none());
    }
}
