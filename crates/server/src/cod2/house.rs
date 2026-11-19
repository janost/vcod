//! CoD2 server housekeeping (docs/research/cod2-protocol.md 17): the
//! timeouts and the UDP download with its www redirect. The rate, pacing,
//! ping and LAN test both servers share are `crate::engine::link`.

use vcod_cod2::net::download::DownloadMsg;
use vcod_common::net::codec::MsgWriter;

use super::Cod2Game;
use crate::engine::download::{DownloadFile, DownloadWire};

/// `sv_timeout`, `sv_zombietime` and `sv_reconnectlimit` defaults, seconds
/// (`SV_Init` 0x8093adc).
pub const TIMEOUT_S: &str = "240";
pub const ZOMBIETIME_S: &str = "2";
pub const RECONNECT_LIMIT_S: &str = "3";
/// `SV_CheckTimeouts` drops a client on its sixth stale frame.
pub const TIMEOUT_FRAMES: u32 = 5;
pub use crate::engine::download::{DownloadCvars, Www};

/// One client's transfer state (client +0x20c68 .. +0x20e14).
pub type Download = crate::engine::download::Download<Cod2Game>;

/// CoD2's `svc_download` shapes (0x808fd64) and its `wwwdl` drops
/// (`SV_WWWDownload_f` 0x808f97c).
impl DownloadWire for Cod2Game {
    const UNEXPECTED: &'static str = "PC_PATCH_1_1_UNEXPECTEDDOWLOADMESSAGE";
    const DISCONNECTED: &'static str = "PC_PATCH_1_1_DOWNLOADDISCONNECTED";

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
        if self.name.is_empty() || self.www.acked {
            return;
        }
        if self.transfer.file().is_none() {
            if cvars.www && wants_www {
                if !self.www.fallback {
                    // `SV_WWWRedirectClient`: only when the file opens.
                    if let Some(file) = open(&self.name) {
                        self.redirect(w, cvars, &file);
                        return;
                    }
                } else {
                    self.www.fallback = false;
                }
            }
            self.www.redirected = false;
            let Some(file) = self.open_checked(w, cvars, open) else {
                return;
            };
            log::info!("clientDownload: beginning {:?}", self.name);
            self.transfer.start(file);
        }
        self.write_blocks(w, svs_ms, budget);
    }
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

    /// `download_path` against retail's live answers (cod2-protocol.md
    /// 17.5): a file under the home path, then the base path, separators
    /// folded; and vcod's own refusals of anything but an iwd, `..`,
    /// directories and the filesystem root.
    #[test]
    fn download_names_open_as_retail_bar_vcod_s_refusals() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
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
    }
}
