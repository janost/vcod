//! A 1.7 client's UDP download and the www redirect
//! (docs/research/cod4-handshake.md 11): `SV_WriteDownloadToClient`
//! (0x816f774) and the `download`, `nextdl`, `stopdl`, `donedl`,
//! `retransdl` and `wwwdl` commands.

use std::path::{Path, PathBuf};

use vcod_cod4::net::download::DownloadMsg;
use vcod_common::net::codec::MsgWriter;

use super::Cod4Game;
use crate::engine::download::{DownloadFile, DownloadWire};

pub use crate::engine::download::{DownloadCvars, Www};

/// One client's transfer (client +0x212a0 .. +0x21450).
pub type Download = crate::engine::download::Download<Cod4Game>;

/// 1.7's `svc_download` shapes (section 11: the block number a long) and
/// its `wwwdl` drops (0x8170ffa, as CoD2's `SV_WWWDownload_f` reads its
/// strings, 0x8230b14..0x8230cbc).
impl DownloadWire for Cod4Game {
    const UNEXPECTED: &'static str = "Unexpected www download message.";
    const DISCONNECTED: &'static str = "Client dropped to download files.";
    const END_CLEARS_REDIRECT: bool = true;

    fn write_block(w: &mut MsgWriter, block: u32, size: Option<i32>, data: Vec<u8>) {
        let block = block as i32;
        DownloadMsg::Block { block, size, data }.write(w);
    }

    fn write_refusal(w: &mut MsgWriter, text: String) {
        DownloadMsg::Refused(text).write(w);
    }

    fn write_redirect(w: &mut MsgWriter, url: String, size: i32, flags: i32) {
        DownloadMsg::Redirect { url, size, flags }.write(w);
    }
}

impl Download {
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
        if self.name.is_empty() || self.www.acked {
            return;
        }
        if self.transfer.file().is_none() {
            let Some(file) = self.open_checked(w, cvars, open) else {
                return;
            };
            if cvars.www && wants_www {
                if self.www.fallback {
                    // 0x816fc72: the fallback flag cleared and nothing
                    // written this message; the blocks start on the next.
                    self.www.fallback = false;
                    self.start(file);
                    return;
                }
                self.redirect(w, cvars, &file);
                return;
            }
            log::info!("clientDownload: beginning {:?}", self.name);
            self.start(file);
        }
        self.write_blocks(w, svs_ms, budget);
    }

    fn start(&mut self, file: DownloadFile) {
        self.www.redirected = false;
        self.transfer.start(file);
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
    vcod_common::game_dir::confined(root, name)
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
        assert_eq!(d.wwwdl("ack", "c"), Www::None);
        assert_eq!(d.wwwdl("fail", "c"), Www::Gamestate);
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
        let budget = crate::engine::download::blocks_per_message(5000, 50);
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
