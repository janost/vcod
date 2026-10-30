//! CoD2 server housekeeping (docs/research/cod2-protocol.md 17): the
//! timeouts and the UDP download with its www redirect. The rate, pacing,
//! ping and LAN test both servers share are `crate::engine::link`.

use vcod_cod2::net::download::{DownloadMsg, fs_iw_iwd};
use vcod_common::net::codec::MsgWriter;

/// `sv_timeout`, `sv_zombietime` and `sv_reconnectlimit` defaults, seconds
/// (`SV_Init` 0x8093adc).
pub const TIMEOUT_S: &str = "240";
pub const ZOMBIETIME_S: &str = "2";
pub const RECONNECT_LIMIT_S: &str = "3";
/// `SV_CheckTimeouts` drops a client on its sixth stale frame.
pub const TIMEOUT_FRAMES: u32 = 5;
/// `SV_WriteDownloadToClient`'s block size and window.
const BLOCK: usize = 0x800;
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
    /// The file, read whole on open; `None` until opened.
    data: Option<Vec<u8>>,
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
    /// Blocks in the file, the empty one that ends it included.
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

    /// `SV_CloseDownload` (0x808f6f6).
    pub fn close(&mut self) {
        self.name.clear();
        self.data = None;
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
        if self.data.is_some() && self.block(self.acked).is_empty() {
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
        self.data = None;
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
    /// then up to `budget` blocks inside the window. `open` reads a file
    /// under the server's base path, `None` when there is none.
    pub fn write(
        &mut self,
        w: &mut MsgWriter,
        svs_ms: i32,
        budget: u32,
        cvars: &DownloadCvars,
        wants_www: bool,
        open: &dyn Fn(&str) -> Option<Vec<u8>>,
    ) {
        if self.name.is_empty() || self.www_acked {
            return;
        }
        if self.data.is_none() {
            if cvars.www && wants_www {
                if !self.www_fallback {
                    // `SV_WWWRedirectClient`: only when the file opens.
                    if let Some(bytes) = open(&self.name) {
                        let url = format!("{}/{}", cvars.www_base_url, self.name);
                        log::info!("Redirecting client to {url}");
                        self.redirected = true;
                        DownloadMsg::Redirect {
                            url,
                            size: bytes.len() as i32,
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
            match open(&self.name).filter(|d| !d.is_empty()) {
                Some(bytes) => {
                    log::info!("clientDownload: beginning {:?}", self.name);
                    self.data = Some(bytes);
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
        let window = (self.acked + WINDOW).min(self.blocks());
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
            DownloadMsg::Block {
                block: block as i32,
                size: (block == 0).then(|| self.data.as_ref().map_or(0, |d| d.len() as i32)),
                data: self.block(block).to_vec(),
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

/// A file a client may download: `<dir>/<file>.iwd`, any directory beside
/// `main` (a mod's) and any name that stays inside it (the `~`-padded ones
/// too). Retail's `FS_SV_FOpenFileRead` serves any file under the base
/// path; vcod serves iwds alone.
pub fn servable(name: &str) -> Option<std::path::PathBuf> {
    vcod_cod2::net::download::safe_iwd_path(name)
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
}
