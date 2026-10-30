//! Downloads on the CoD2 wire (docs/research/cod2-protocol.md 11.5, 17.5):
//! the `svc_download` (5) shapes, `FS_iwIwd`'s stock test, `FS_CompareIwds`'
//! list of what to fetch, and the client's side of a UDP transfer
//! (`CL_BeginDownload`, `CL_ParseDownload`, `CL_NextDownload`; Mac 0x14d668,
//! 0x1d59c8, 0x14d784).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use vcod_common::net::codec::{MsgReader, MsgWriter};
use vcod_common::net::download::Download;

use super::ops::Svc;

/// `FS_iwIwd(name, base)` (0x8065324): `<base>/iw_00` .. `<base>/iw_24`
/// compared whole (case and slash folded, so a name with `.iwd` never
/// matches), or a name whose part up to `localized_` is
/// `<base>/localized_` and whose rest holds `_iw00` .. `_iw24`.
pub fn fs_iw_iwd(name: &str, base: &str) -> bool {
    let fold = |s: &str| s.to_ascii_lowercase().replace('\\', "/");
    let name = fold(name);
    let base = fold(base);
    if (0..25).any(|n| name == format!("{base}/iw_{n:02}")) {
        return true;
    }
    let Some(at) = name.find("localized_") else {
        return false;
    };
    let (head, rest) = name.split_at(at + "localized_".len());
    head == format!("{base}/localized_") && (0..25).any(|n| rest.contains(&format!("_iw{n:02}")))
}

/// `FS_svrIwd` (0x8065450): a name holding `_svr_`, any case.
pub fn fs_svr_iwd(name: &str) -> bool {
    name.to_ascii_lowercase().contains("_svr_")
}

/// One `svc_download`, as `CL_ParseDownload` reads it after the op.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadMsg {
    /// Block -1: `SV_WWWRedirectClient`'s URL, the file's size and the flags
    /// (bit 0 `sv_wwwDlDisconnected`).
    Redirect { url: String, size: i32, flags: i32 },
    /// Block 0 with size -1: the refusal, `EXE_...\x15<name>`.
    Refused(String),
    /// A block of the file; block 0 carries the file's size. The empty
    /// block past the last byte ends the transfer.
    Block {
        block: i32,
        size: Option<i32>,
        data: Vec<u8>,
    },
}

impl DownloadMsg {
    /// Reads one after its op byte.
    pub fn read(r: &mut MsgReader) -> DownloadMsg {
        let block = i32::from(r.read_short());
        if block == -1 {
            let url = super::latin1(&r.read_string_bytes());
            let size = r.read_long();
            let flags = r.read_long();
            return DownloadMsg::Redirect { url, size, flags };
        }
        let mut size = None;
        if block == 0 {
            let s = r.read_long();
            if s < 0 {
                return DownloadMsg::Refused(super::latin1(&r.read_string_bytes()));
            }
            size = Some(s);
        }
        let len = r.read_short().max(0) as usize;
        let data = (0..len).map(|_| r.read_byte()).collect();
        DownloadMsg::Block { block, size, data }
    }

    /// Writes it with its op: `SV_WriteDownloadToClient`'s blocks,
    /// `SV_WWWRedirectClient` and the refusal (0x808fd64).
    pub fn write(&self, w: &mut MsgWriter) {
        w.write_byte(Svc::Download as u8);
        match self {
            DownloadMsg::Redirect { url, size, flags } => {
                w.write_short(-1);
                w.write_string_bytes(&super::write::latin1_bytes(url));
                w.write_long(*size);
                w.write_long(*flags);
            }
            DownloadMsg::Refused(reason) => {
                w.write_short(0);
                w.write_long(-1);
                w.write_string_bytes(&super::write::latin1_bytes(reason));
            }
            DownloadMsg::Block { block, size, data } => {
                w.write_short(*block as i16);
                if let Some(s) = size {
                    w.write_long(*s);
                }
                w.write_short(data.len() as i16);
                for &b in data {
                    w.write_byte(b);
                }
            }
        }
    }
}

/// `sv_referencedIwds` and `sv_referencedIwdNames` paired up.
pub fn referenced_iwds(systeminfo: &str) -> Vec<(String, i32)> {
    let key = |k: &str| super::oob::info_value_for_key(systeminfo, k).unwrap_or("");
    let names = key("sv_referencedIwdNames").split_whitespace();
    let sums = key("sv_referencedIwds").split_whitespace();
    names
        .zip(sums)
        .filter_map(|(n, s)| Some((n.to_string(), s.parse().ok()?)))
        .collect()
}

/// `FS_CompareIwds` (0x80654a0) with its download flag: every referenced
/// iwd that is not a stock one (`FS_iwIwd` on the bare name, base `main`),
/// not a `_svr_` one, and whose checksum `have` does not hold, as the
/// `<name>.iwd` the client asks for.
pub fn missing_iwds(systeminfo: &str, have: impl Fn(i32) -> bool) -> Vec<String> {
    referenced_iwds(systeminfo)
        .into_iter()
        .filter(|(name, sum)| {
            !name.is_empty() && !fs_iw_iwd(name, "main") && !fs_svr_iwd(name) && !have(*sum)
        })
        .map(|(name, _)| format!("{name}.iwd"))
        .collect()
}

/// The iwd named `<game>/<file>.iwd` as a path under the homepath. Retail
/// writes whatever `<game>/<name>` the server references
/// (`FS_SV_FOpenFileWrite`, Mac 0x46304, checks no character), so a mod
/// directory and the `~`-padded names servers sort their iwds with pass;
/// vcod refuses only what would leave the homepath: more or fewer than
/// two components, `.` or `..`, a backslash, a drive colon or a control
/// character.
pub fn safe_iwd_path(remote: &str) -> Option<PathBuf> {
    remote.strip_suffix(".iwd")?;
    let (dir, file) = remote.split_once('/')?;
    let ok = |c: &str| {
        !c.is_empty()
            && c != "."
            && c != ".."
            && !c
                .bytes()
                .any(|b| b < b' ' || b == 0x7f || matches!(b, b'/' | b'\\' | b':'))
    };
    (ok(dir) && ok(file)).then(|| Path::new(dir).join(file))
}

/// `FS_CompareIwds`' local name (Mac 0x47b14): `<name>.iwd`, or
/// `<name>.<checksum %08x>.iwd` when a file of that name already sits in
/// the homepath (one with the wrong checksum, as the search path did not
/// hold the listed one).
pub fn local_iwd_name(root: &Path, remote: &str, sum: i32) -> String {
    match root.join(remote).exists() {
        false => remote.to_string(),
        true => {
            let stem = remote.strip_suffix(".iwd").unwrap_or(remote);
            format!("{stem}.{sum:08x}.iwd")
        }
    }
}

/// The checksums of every iwd under `root`'s `main/` and, when the
/// systeminfo names one, its `fs_game` directory: what `FS_CompareIwds`
/// looks for on the search path after `CL_SystemInfoChanged` set
/// `fs_game` and `FS_ConditionalRestart` reopened it.
pub fn held_checksums(root: &Path, fs_game: Option<&str>) -> Vec<i32> {
    let dirs: Vec<PathBuf> = std::iter::once(root.join("main"))
        .chain(fs_game.map(|g| root.join(g)))
        .filter(|d| d.is_dir())
        .collect();
    let refs: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
    vcod_common::pk3::search_archives(&refs, vcod_common::pk3::ArchiveKind::Iwd)
        .into_iter()
        .map(|p| p.checksum)
        .collect()
}

/// The systeminfo's `fs_game` as a directory beside `main`: `None` when
/// unset, `main` itself, or not one path component.
pub fn fs_game(systeminfo: &str) -> Option<String> {
    let g = super::oob::info_value_for_key(systeminfo, "fs_game")?;
    safe_iwd_path(&format!("{g}/x.iwd"))
        .filter(|_| !g.eq_ignore_ascii_case("main"))
        .map(|_| g.to_string())
}

/// What a download step asks of the caller.
#[derive(Debug, PartialEq, Eq)]
pub enum DownloadEvent {
    /// A client command to queue (`download`, `nextdl`, `retransdl`,
    /// `stopdl`, `donedl`, `wwwdl ...`).
    Command(String),
    /// A file landed at this path; the caller adds it to its search path.
    Finished(PathBuf),
    /// A file could not be written and was passed over, as `CL_ParseDownload`
    /// does with `Could not create %s` (Mac 0x1d59c8): the join goes on
    /// without it.
    Skipped(String),
    /// The server refused or the transfer failed; the message says why.
    /// Retail ends the connection with it (`Com_Error`).
    Failed(String),
}

/// Bytes landed of the file in flight, for the connect screen
/// (`UI_DisplayDownloadInfo`, Mac 0x156aa6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub remote: String,
    pub got: u64,
    /// 0 until block 0 or the redirect named it.
    pub size: u64,
}

/// One file in flight.
struct Current {
    remote: String,
    spool: Download,
    /// `clc.downloadBlock`: the block expected next.
    block: i32,
    want: Option<i32>,
    size: u64,
    /// The HTTP transfer a www redirect started.
    www: Option<vcod_common::net::http::Fetch>,
}

/// Largest file a redirect may announce.
const MAX_WWW_SIZE: u64 = 1 << 31;

/// The client's download queue after a gamestate (`CL_InitDownloads`):
/// one file at a time, `donedl` once the queue is empty, which has the
/// server send the gamestate again. A www redirect is fetched over HTTP
/// when [`Self::with_www`] is on (`CL_ParseWWWDownload`, `CL_WWWDownload`):
/// `wwwdl ack`, then `wwwdl done` and the next file, or `wwwdl fail`, which
/// retail's server answers by resending the gamestate and serving the next
/// `download` over UDP; build a new queue on that gamestate.
pub struct Downloads {
    root: PathBuf,
    queue: VecDeque<String>,
    current: Option<Current>,
    systeminfo: String,
    /// Whether anything was asked for: `donedl` is owed at the end.
    started: bool,
    www: bool,
}

impl Downloads {
    /// The queue for `missing` (`<game>/<file>.iwd`), files landing under
    /// `root` (the homepath, `main/` and the mod directories inside it).
    pub fn new(root: &Path, systeminfo: &str, missing: Vec<String>) -> Self {
        Downloads {
            root: root.to_path_buf(),
            queue: missing.into(),
            current: None,
            systeminfo: systeminfo.to_string(),
            started: false,
            www: false,
        }
    }

    /// Fetch a www redirect over HTTP (`cl_wwwDownload`).
    pub fn with_www(mut self, www: bool) -> Self {
        self.www = www;
        self
    }

    pub fn is_done(&self) -> bool {
        self.queue.is_empty() && self.current.is_none()
    }

    /// The file in flight and how much of it landed.
    pub fn progress(&self) -> Option<Progress> {
        let cur = self.current.as_ref()?;
        let got = match &cur.www {
            Some(f) => f.received(),
            None => cur.spool.received,
        };
        Some(Progress {
            remote: cur.remote.clone(),
            got,
            size: cur.size,
        })
    }

    /// `CL_NextDownload`: the next file's `download`, or `donedl` once
    /// every file came (only when one was asked for).
    pub fn next_download(&mut self) -> Vec<DownloadEvent> {
        let mut out = Vec::new();
        while let Some(remote) = self.queue.pop_front() {
            let stem = remote.strip_suffix(".iwd").unwrap_or(&remote);
            let want = referenced_iwds(&self.systeminfo)
                .into_iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(stem))
                .map(|(_, s)| s);
            let local = local_iwd_name(&self.root, &remote, want.unwrap_or(0));
            let Some(rel) = safe_iwd_path(&local) else {
                out.push(DownloadEvent::Skipped(format!("Could not create {local}")));
                continue;
            };
            match Download::create(&remote, &self.root.join(rel)) {
                Ok(spool) => {
                    self.current = Some(Current {
                        remote: remote.clone(),
                        spool,
                        block: 0,
                        want,
                        size: 0,
                        www: None,
                    });
                    self.started = true;
                    out.push(DownloadEvent::Command(format!("download {remote}")));
                    return out;
                }
                Err(e) => out.push(DownloadEvent::Skipped(format!(
                    "Could not create {local}: {e:#}"
                ))),
            }
        }
        if self.started {
            self.started = false;
            out.push(DownloadEvent::Command("donedl".into()));
        }
        out
    }

    /// `CL_WWWDownload` once a frame: an HTTP transfer that ended lands
    /// its file, says `wwwdl done` and asks for the next one, or says
    /// `wwwdl fail` and waits for the gamestate the fallback brings.
    pub fn poll(&mut self) -> Vec<DownloadEvent> {
        let Some(res) = self
            .current
            .as_mut()
            .and_then(|c| c.www.as_mut())
            .and_then(|f| f.poll())
        else {
            return Vec::new();
        };
        let mut cur = self.current.take().unwrap();
        cur.www = None;
        let dest = cur.spool.dest().to_path_buf();
        let landed = res.and_then(|()| cur.spool.finish(cur.want));
        match landed {
            Ok(()) => {
                let mut out = vec![
                    DownloadEvent::Finished(dest),
                    DownloadEvent::Command("wwwdl done".into()),
                ];
                out.extend(self.next_download());
                out
            }
            Err(e) => {
                self.queue.clear();
                self.started = false;
                vec![
                    DownloadEvent::Skipped(format!(
                        "Download failure while getting '{}': {e:#}",
                        cur.remote
                    )),
                    DownloadEvent::Command("wwwdl fail".into()),
                ]
            }
        }
    }

    /// `CL_ParseWWWDownload` (Mac 0x1d57c8): `wwwdl ack` and the HTTP
    /// transfer into the file's spool, or `wwwdl fail` when it cannot
    /// start (vcod without `cl_wwwDownload` too). Flag 2 opens the URL in
    /// a browser and quits on retail; vcod ends the connection instead.
    fn redirect(&mut self, url: &str, size: i32, flags: i32) -> Vec<DownloadEvent> {
        if flags & 2 != 0 {
            return vec![
                DownloadEvent::Command("wwwdl bbl8r".into()),
                DownloadEvent::Failed(format!("the server sends {url} to be opened in a browser")),
            ];
        }
        let mut out = vec![DownloadEvent::Command("wwwdl ack".into())];
        let fetch = match (self.www, self.current.as_ref()) {
            (true, Some(cur)) if (0..MAX_WWW_SIZE as i64).contains(&i64::from(size)) => cur
                .spool
                .spool()
                .map_err(anyhow::Error::from)
                .and_then(|f| vcod_common::net::http::Fetch::spawn(url, f, MAX_WWW_SIZE)),
            (true, _) => Err(anyhow::anyhow!("{url} announced {size} bytes")),
            (false, _) => Err(anyhow::anyhow!("cl_wwwDownload is off")),
        };
        match fetch {
            Ok(f) => {
                log::info!("cod2: server redirected download: {url} ({size} bytes)");
                let cur = self.current.as_mut().unwrap();
                cur.size = size as u64;
                cur.www = Some(f);
            }
            Err(e) => {
                if let Some(cur) = self.current.take() {
                    cur.spool.abort();
                }
                self.queue.clear();
                self.started = false;
                out.push(DownloadEvent::Command("wwwdl fail".into()));
                out.push(DownloadEvent::Skipped(format!(
                    "Failed to initialize download for '{url}': {e:#}"
                )));
            }
        }
        out
    }

    /// `CL_ParseDownload` for one message.
    pub fn on_message(&mut self, msg: &DownloadMsg) -> Vec<DownloadEvent> {
        let Some(cur) = self.current.as_mut() else {
            // Mac 0x1d59c8: blocks nobody asked for are stopped.
            return match msg {
                DownloadMsg::Block { .. } => vec![DownloadEvent::Command("stopdl".into())],
                _ => Vec::new(),
            };
        };
        let block_due = cur.block;
        match msg {
            // A redirect while one runs is read and dropped (Mac 0x1d59c8).
            DownloadMsg::Redirect { .. } if cur.www.is_some() => Vec::new(),
            DownloadMsg::Redirect { url, size, flags } => self.redirect(url, *size, *flags),
            DownloadMsg::Refused(reason) => {
                let cur = self.current.take().unwrap();
                cur.spool.abort();
                vec![DownloadEvent::Failed(reason.clone())]
            }
            DownloadMsg::Block { block, .. } if *block != block_due => {
                // A block past the one due asks for it again; an older one is
                // a retransmit already written.
                if *block > block_due {
                    vec![DownloadEvent::Command(format!("retransdl {block_due}"))]
                } else {
                    Vec::new()
                }
            }
            DownloadMsg::Block { data, size, .. } => {
                if let Some(s) = size {
                    cur.size = (*s).max(0) as u64;
                }
                if let Err(e) = cur.spool.accept_block(data) {
                    let cur = self.current.take().unwrap();
                    cur.spool.abort();
                    let mut out = vec![
                        DownloadEvent::Command("stopdl".into()),
                        DownloadEvent::Skipped(format!("{e:#}")),
                    ];
                    out.extend(self.next_download());
                    return out;
                }
                cur.block += 1;
                let mut out = vec![DownloadEvent::Command(format!("nextdl {block_due}"))];
                if data.is_empty() {
                    let cur = self.current.take().unwrap();
                    let dest = cur.spool.dest().to_path_buf();
                    match cur.spool.finish(cur.want) {
                        Ok(()) => out.push(DownloadEvent::Finished(dest)),
                        Err(e) => out.push(DownloadEvent::Skipped(format!("{e:#}"))),
                    }
                    out.extend(self.next_download());
                }
                out
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::huffman::Huffman;

    #[test]
    fn fs_iw_iwd_takes_the_stock_names_only() {
        assert!(fs_iw_iwd("main/iw_00", "main"));
        assert!(fs_iw_iwd("MAIN\\IW_24", "main"));
        assert!(!fs_iw_iwd("main/iw_25", "main"));
        // The whole-name compare: with `.iwd` it is not stock.
        assert!(!fs_iw_iwd("main/iw_00.iwd", "main"));
        assert!(fs_iw_iwd("main/localized_english_iw07", "main"));
        assert!(fs_iw_iwd("main/localized_english_iw07.iwd", "main"));
        assert!(!fs_iw_iwd("main/localized_english_x", "main"));
        assert!(!fs_iw_iwd("mod/iw_00", "main"));
        assert!(!fs_iw_iwd("main/zzz_test", "main"));
    }

    #[test]
    fn missing_iwds_skips_stock_svr_and_held_ones() {
        let si = "\\sv_referencedIwds\\1 2 3 4 \\sv_referencedIwdNames\\main/iw_15 main/zzz_a main/x_svr_y main/zzz_b";
        assert_eq!(missing_iwds(si, |s| s == 4), vec!["main/zzz_a.iwd"]);
    }

    #[test]
    fn iwd_paths_take_mod_dirs_and_tilde_names_but_never_leave_home() {
        let tilde = format!("main/{}.iwd", "~".repeat(54));
        assert_eq!(
            safe_iwd_path(&tilde),
            Some(Path::new("main").join(format!("{}.iwd", "~".repeat(54))))
        );
        assert_eq!(
            safe_iwd_path("veteran/rt.iwd"),
            Some(Path::new("veteran").join("rt.iwd"))
        );
        for bad in [
            "main/../x.iwd",
            "../main/x.iwd",
            "main/a/b.iwd",
            "main\\x.iwd",
            "c:/x.iwd",
            "main/x.pk3",
            "/x.iwd",
            "main/.iwd/..",
        ] {
            assert_eq!(safe_iwd_path(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_name_already_on_disk_gets_the_checksum_suffix() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(local_iwd_name(dir.path(), "main/a.iwd", -2), "main/a.iwd");
        std::fs::create_dir(dir.path().join("main")).unwrap();
        std::fs::write(dir.path().join("main/a.iwd"), b"old").unwrap();
        assert_eq!(
            local_iwd_name(dir.path(), "main/a.iwd", -2),
            "main/a.fffffffe.iwd"
        );
    }

    #[test]
    fn fs_game_names_one_directory_beside_main() {
        assert_eq!(fs_game("\\fs_game\\veteran"), Some("veteran".into()));
        assert_eq!(fs_game("\\fs_game\\MAIN"), None);
        assert_eq!(fs_game("\\fs_game\\"), None);
        assert_eq!(fs_game("\\fs_game\\../x"), None);
        assert_eq!(fs_game("\\sv_pure\\1"), None);
    }

    /// The public server's lists: a mod iwd and a tilde-padded one in
    /// `main/`, both asked for.
    #[test]
    fn missing_iwds_keeps_mod_and_tilde_names() {
        let t = "~".repeat(54);
        let si = format!(
            "\\sv_referencedIwds\\7 8 9 \\sv_referencedIwdNames\\veteran/rt main/{t} main/iw_07"
        );
        assert_eq!(
            missing_iwds(&si, |_| false),
            vec!["veteran/rt.iwd".to_string(), format!("main/{t}.iwd")]
        );
    }

    fn iwd_sum(iwd: &[u8]) -> i32 {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("probe.iwd");
        std::fs::write(&p, iwd).unwrap();
        vcod_common::pak_checksum::pak_checksums(&p, 0)
            .unwrap()
            .checksum
    }

    fn block(block: i32, size: Option<i32>, data: &[u8]) -> DownloadMsg {
        DownloadMsg::Block {
            block,
            size,
            data: data.to_vec(),
        }
    }

    fn commands(evs: &[DownloadEvent]) -> Vec<&str> {
        evs.iter()
            .filter_map(|e| match e {
                DownloadEvent::Command(c) => Some(c.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_udp_transfer_lands_the_file_in_its_mod_dir_then_says_donedl() {
        let dir = tempfile::tempdir().unwrap();
        let iwd = vcod_common::testing::pk3_with(&[("maps/mp/mp_x.d3dbsp", b"IBSP")]);
        let sum = iwd_sum(&iwd);
        let si = format!("\\sv_referencedIwds\\{sum} \\sv_referencedIwdNames\\veteran/rt");
        let mut dls = Downloads::new(dir.path(), &si, missing_iwds(&si, |_| false));
        assert_eq!(commands(&dls.next_download()), ["download veteran/rt.iwd"]);
        let half = iwd.len() / 2;
        let evs = dls.on_message(&block(0, Some(iwd.len() as i32), &iwd[..half]));
        assert_eq!(commands(&evs), ["nextdl 0"]);
        let p = dls.progress().unwrap();
        assert_eq!((p.got, p.size), (half as u64, iwd.len() as u64));
        // A block past the one due asks for it again.
        assert_eq!(
            commands(&dls.on_message(&block(2, None, &[]))),
            ["retransdl 1"]
        );
        dls.on_message(&block(1, None, &iwd[half..]));
        let evs = dls.on_message(&block(2, None, &[]));
        assert_eq!(commands(&evs), ["nextdl 2", "donedl"]);
        let dest = dir.path().join("veteran/rt.iwd");
        assert!(evs.contains(&DownloadEvent::Finished(dest.clone())));
        assert_eq!(std::fs::read(dest).unwrap(), iwd);
        assert!(dls.is_done());
    }

    #[test]
    fn a_refusal_fails_and_an_unwritable_name_is_passed_over() {
        let dir = tempfile::tempdir().unwrap();
        let si = "\\sv_referencedIwds\\1 2 \\sv_referencedIwdNames\\main/a/b main/z";
        let mut dls = Downloads::new(dir.path(), si, missing_iwds(si, |_| false));
        let evs = dls.next_download();
        assert!(matches!(&evs[0], DownloadEvent::Skipped(m) if m.contains("main/a/b.iwd")));
        assert_eq!(commands(&evs), ["download main/z.iwd"]);
        let why = "EXE_AUTODL_FILENOTONSERVER\x15main/z.iwd";
        assert_eq!(
            dls.on_message(&DownloadMsg::Refused(why.into())),
            vec![DownloadEvent::Failed(why.into())]
        );
        assert!(!dir.path().join("main/z.iwd").exists());
    }

    #[test]
    fn a_redirect_without_www_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let si = "\\sv_referencedIwds\\1 \\sv_referencedIwdNames\\main/z";
        let mut dls = Downloads::new(dir.path(), si, missing_iwds(si, |_| false));
        dls.next_download();
        let evs = dls.on_message(&DownloadMsg::Redirect {
            url: "http://127.0.0.1:1/main/z.iwd".into(),
            size: 10,
            flags: 0,
        });
        assert_eq!(commands(&evs), ["wwwdl ack", "wwwdl fail"]);
        assert!(dls.is_done());
    }

    /// `CL_WWWDownload`: the HTTP body lands the file, `wwwdl done`, then
    /// `donedl` for the empty queue.
    #[test]
    fn a_www_redirect_fetches_over_http() {
        use std::io::{Read, Write};
        let dir = tempfile::tempdir().unwrap();
        let iwd = vcod_common::testing::pk3_with(&[("maps/mp/mp_x.d3dbsp", b"IBSP")]);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = iwd.clone();
        let server = std::thread::spawn(move || {
            let (mut c, _) = listener.accept().unwrap();
            let mut req = [0u8; 1024];
            let n = c.read(&mut req).unwrap();
            let line = String::from_utf8_lossy(&req[..n])
                .lines()
                .next()
                .unwrap()
                .to_string();
            write!(
                c,
                "HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .unwrap();
            c.write_all(&body).unwrap();
            line
        });
        let sum = iwd_sum(&iwd);
        let si = format!("\\sv_referencedIwds\\{sum} \\sv_referencedIwdNames\\main/z");
        let mut dls = Downloads::new(dir.path(), &si, missing_iwds(&si, |_| false)).with_www(true);
        dls.next_download();
        let evs = dls.on_message(&DownloadMsg::Redirect {
            url: format!("http://127.0.0.1:{port}/main/z.iwd"),
            size: iwd.len() as i32,
            flags: 0,
        });
        assert_eq!(commands(&evs), ["wwwdl ack"]);
        assert_eq!(server.join().unwrap(), "GET /main/z.iwd HTTP/1.0");
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let evs = loop {
            let evs = dls.poll();
            if !evs.is_empty() || std::time::Instant::now() > until {
                break evs;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(commands(&evs), ["wwwdl done", "donedl"]);
        let dest = dir.path().join("main/z.iwd");
        assert!(evs.contains(&DownloadEvent::Finished(dest.clone())));
        assert_eq!(std::fs::read(dest).unwrap(), iwd);
    }

    #[test]
    fn download_messages_read_back_as_written() {
        let huff = Huffman::new();
        for m in [
            DownloadMsg::Redirect {
                url: "http://x/main/a.iwd".into(),
                size: 77,
                flags: 1,
            },
            DownloadMsg::Refused("EXE_AUTODL_FILENOTONSERVER\x15main/a.iwd".into()),
            DownloadMsg::Block {
                block: 0,
                size: Some(3),
                data: vec![1, 2, 3],
            },
            DownloadMsg::Block {
                block: 4,
                size: None,
                data: Vec::new(),
            },
        ] {
            let mut w = MsgWriter::new(&huff);
            m.write(&mut w);
            let mut r = MsgReader::from_plain(w.into_ops());
            assert_eq!(r.read_byte(), Svc::Download as u8);
            assert_eq!(DownloadMsg::read(&mut r), m);
        }
    }
}
