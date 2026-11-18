//! CoD2 server housekeeping (docs/research/cod2-protocol.md 17): the
//! timeouts and the UDP download with its www redirect. The rate, pacing,
//! ping and LAN test both servers share are `crate::engine::link`.

use vcod_cod2::net::download::{DownloadMsg, fs_iw_iwd};
use vcod_common::net::codec::MsgWriter;

use crate::download::DownloadFile;

/// `sv_timeout`, `sv_zombietime` and `sv_reconnectlimit` defaults, seconds
/// (`SV_Init` 0x8093adc).
pub const TIMEOUT_S: &str = "240";
pub const ZOMBIETIME_S: &str = "2";
pub const RECONNECT_LIMIT_S: &str = "3";
/// `SV_CheckTimeouts` drops a client on its sixth stale frame.
pub const TIMEOUT_FRAMES: u32 = 5;
/// `SV_WriteDownloadToClient`'s window; its 0x800-byte blocks are
/// [`DownloadFile`]'s.
const WINDOW: u32 = 8;
/// A window's blocks go again after this long without an ack (`< 0x3e9`).
const RETRANSMIT_MS: i32 = 1000;

/// The www dvars `SV_WriteDownloadToClient` reads.
#[derive(Clone, Debug, Default)]
pub struct DownloadCvars {
    pub allow: bool,
    pub pure: bool,
    pub www: bool,
    pub www_base_url: String,
    pub www_disconnected: bool,
}

/// One client's transfer state (client +0x20c68 .. +0x20e14).
#[derive(Default)]
pub struct Download {
    /// `downloadName`; empty when none.
    pub name: String,
    /// The open file; `None` until opened.
    file: Option<DownloadFile>,
    /// `downloadClientBlock`: the next block the client acks.
    acked: u32,
    /// `downloadCurrentBlock`: the next block written.
    current: u32,
    /// `downloadSendTime`.
    send_time: i32,
    /// A redirect went out (+0x20e0c).
    pub redirected: bool,
    /// `wwwdl ack` came (+0x20e10): nothing more is written.
    pub www_acked: bool,
    /// The last www transfer failed (+0x20e14): the next goes over UDP.
    pub www_fallback: bool,
}

/// What a `wwwdl` asks of the server (`SV_WWWDownload_f` 0x808f97c).
#[derive(Debug, PartialEq, Eq)]
pub enum Www {
    None,
    /// `fail` / `chkfail`: the gamestate again.
    Gamestate,
    Drop(&'static str),
}

impl Download {
    /// `SV_CloseDownload` (0x808f6f6).
    pub fn close(&mut self) {
        self.name.clear();
        self.file = None;
    }

    /// The `download <name>` command: a new name, the old transfer closed.
    pub fn begin(&mut self, name: &str) {
        self.close();
        self.name = name.chars().take(63).collect();
    }

    /// `SV_NextDownload_f` (0x808f83c). False for any block but the one
    /// due: the caller drops the client with `broken download`.
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

    /// `retransdl <n>` (0x808f7fc): start over from the block due.
    pub fn retransmit(&mut self, block: i32) {
        if block == self.acked as i32 {
            self.current = self.acked;
        }
    }

    /// `SV_WWWDownload_f` (0x808f97c).
    pub fn wwwdl(&mut self, sub: &str) -> Www {
        const UNEXPECTED: &str = "PC_PATCH_1_1_UNEXPECTEDDOWLOADMESSAGE";
        let is = |s: &str| sub.eq_ignore_ascii_case(s);
        if !self.redirected {
            return Www::Drop(UNEXPECTED);
        }
        if is("ack") {
            self.www_acked = true;
            return Www::None;
        }
        if is("bbl8r") {
            return Www::Drop("PC_PATCH_1_1_DOWNLOADDISCONNECTED");
        }
        if !self.www_acked {
            return Www::Drop(UNEXPECTED);
        }
        let failed = if is("done") {
            false
        } else if is("fail") || is("chkfail") {
            true
        } else {
            return Www::Drop(UNEXPECTED);
        };
        self.file = None;
        self.name.clear();
        self.www_acked = false;
        if failed {
            self.www_fallback = true;
            Www::Gamestate
        } else {
            Www::None
        }
    }

    /// `SV_WriteDownloadToClient` (0x808fdc2): opens the file on the first
    /// call, the www redirect first when the server and the client want it,
    /// then up to `budget` blocks inside the window. `open` opens a file
    /// as [`open_download`] does, `None` when there is none.
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
            if cvars.www && wants_www {
                if !self.www_fallback {
                    // `SV_WWWRedirectClient`: only when the file opens.
                    if let Some(file) = open(&self.name) {
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
                } else {
                    self.www_fallback = false;
                }
            }
            self.redirected = false;
            let refuse = |w: &mut MsgWriter, key: &str, name: &str| {
                DownloadMsg::Refused(format!("{key}\x15{name}")).write(w);
            };
            if !cvars.allow {
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
            match open(&self.name).filter(|f| !f.is_empty()) {
                Some(file) => {
                    log::info!("clientDownload: beginning {:?}", self.name);
                    self.file = Some(file);
                    self.acked = 0;
                    self.current = 0;
                }
                None => {
                    refuse(w, "EXE_AUTODL_FILENOTONSERVER", &self.name);
                    self.name.clear();
                    return;
                }
            }
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
}

/// `SV_WriteDownloadToClient`'s blocks a message: the rate over one
/// snapshot interval in 2 KiB units, rounded up past a whole one; 1 at
/// rate 0.
pub fn blocks_per_message(rate: i32, snapshot_msec: i32) -> u32 {
    if rate == 0 {
        return 1;
    }
    ((rate * snapshot_msec / 1000 + 0x800) >> 11).max(1) as u32
}

/// `FS_SV_FOpenFileRead` (0x8064558) for a download: `<fs_homepath>/<name>`,
/// then `<fs_basepath>/<name>` when the two differ, every run of `/` and
/// `\` folded to one `/` (`FS_BuildOSPath` 0x809eab0), so a leading `/`
/// stays under the root. vcod's own rules beyond retail's (cod2-protocol.md
/// 17.5): only an `.iwd` opens, where retail serves any file (a client asks
/// for referenced iwds alone); a name holding `..`, a colon (a Windows drive
/// prefix) or a control character opens nothing, where retail serves
/// `<root>/../<file>`; retail's third root, `fs_cdpath` (empty, so the
/// filesystem root) when the two paths match, is never tried; an empty path
/// is no root; a directory opens nothing.
pub fn open_download(homepath: &str, basepath: &str, name: &str) -> Option<DownloadFile> {
    DownloadFile::open(&download_path(homepath, basepath, name)?)
}

/// The file [`open_download`] opens.
fn download_path(homepath: &str, basepath: &str, name: &str) -> Option<std::path::PathBuf> {
    let parts: Vec<&str> = name
        .split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    let rel = vcod_common::game_dir::confined_rel_path(&parts.join("/"))?;
    if !rel
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("iwd"))
    {
        return None;
    }
    let base = (!basepath.eq_ignore_ascii_case(homepath)).then_some(basepath);
    [Some(homepath), base]
        .into_iter()
        .flatten()
        .filter(|root| !root.is_empty())
        .map(|root| std::path::Path::new(root).join(&rel))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_per_message_rounds_past_a_whole_block() {
        assert_eq!(blocks_per_message(25000, 50), 1);
        assert_eq!(blocks_per_message(90000, 33), 2);
        assert_eq!(blocks_per_message(0, 50), 1);
    }

    /// `download_path` against retail's live answers (cod2-protocol.md
    /// 17.5): a file under the home path, then the base path, separators
    /// folded; and vcod's own refusals of anything but an iwd, `..`,
    /// directories and the filesystem root.
    #[test]
    fn download_names_open_as_retail_bar_vcod_s_refusals() {
        let dir = std::env::temp_dir().join(format!("vcod-cod2-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (home, base) = (dir.join("home"), dir.join("base"));
        std::fs::create_dir_all(home.join("main")).unwrap();
        std::fs::create_dir_all(base.join("main")).unwrap();
        std::fs::write(home.join("main/zz_ok.iwd"), b"inside main").unwrap();
        std::fs::write(base.join("main/only_base.IWD"), b"base").unwrap();
        std::fs::write(dir.join("secret.iwd"), b"outside").unwrap();
        for other in ["zz_ok.txt", "server.cfg", "games_mp.log"] {
            std::fs::write(home.join("main").join(other), b"not an iwd").unwrap();
        }
        let (h, b) = (home.to_string_lossy(), base.to_string_lossy());
        let read = |p: std::path::PathBuf| std::fs::read(p).unwrap();
        let open = |name: &str| download_path(&h, &b, name).map(read);
        let served = Some(b"inside main".to_vec());
        // Retail's folding (live on `main/zz_ok.txt` and `main\zz_ok.txt`).
        assert_eq!(open("main/zz_ok.iwd"), served);
        assert_eq!(open("main\\zz_ok.iwd"), served);
        assert_eq!(open("/main//zz_ok.iwd"), served);
        assert_eq!(open("main/./zz_ok.iwd"), served);
        assert_eq!(open("main/only_base.IWD"), Some(b"base".to_vec()));
        assert_eq!(open("main/nothere.iwd"), None);
        // An empty home path is no root: the base serves.
        assert_eq!(
            download_path("", &b, "main/only_base.IWD").map(read),
            Some(b"base".to_vec())
        );
        // vcod's: retail serves any file, `../secret.iwd` and, with the two
        // paths equal, any file from the filesystem root.
        assert_eq!(open("main/zz_ok.txt"), None);
        assert_eq!(open("main/server.cfg"), None);
        assert_eq!(open("main/games_mp.log"), None);
        assert_eq!(open("../secret.iwd"), None);
        assert_eq!(open("main/../../secret.iwd"), None);
        assert_eq!(open("..\\secret.iwd"), None);
        let abs = dir.join("secret.iwd").to_string_lossy().into_owned();
        assert_eq!(open(&abs), None);
        assert_eq!(download_path(&h, &h, &abs), None);
        // A drive prefix would replace the root on Windows.
        assert_eq!(open("C:main/zz_ok.iwd"), None);
        assert_eq!(open("D:\\Users\\x.iwd"), None);
        assert_eq!(open("main/zz_ok.iwd:s"), None);
        assert_eq!(open("main"), None);
        assert_eq!(open(""), None);
        assert_eq!(open("."), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
