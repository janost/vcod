//! Downloads on the CoD4 1.7 wire (docs/research/cod4-handshake.md 11): the
//! `svc_download` (5) shapes `SV_WriteDownloadToClient` (0x816f774) writes
//! and `CL_ParseDownload` (iw3mp.exe 0x474440) reads, `FS_iwIwd`'s stock
//! test, and which referenced files a client lacks.

use std::path::{Path, PathBuf};

use vcod_common::net::codec::{MsgReader, MsgWriter};
use vcod_common::net::info_value_for_key;

pub const SVC_DOWNLOAD: u8 = 5;

/// What the client reads as the block number of the server's refusal: the
/// refusal writes a short 0 and a long -1 (0x816f5f2), and the client reads
/// a long first, so it sees the short and the low half of the -1.
const REFUSAL_HEAD: i32 = 0xffff_0000_u32 as i32;

/// One `svc_download`, after its op byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadMsg {
    /// Block -1 (`SV_WWWRedirectClient`, 0x816f674): the URL, the file's
    /// size and `sv_wwwDlDisconnected`.
    Redirect { url: String, size: i32, flags: i32 },
    /// The refusal, `EXE_...\x15<name>`.
    Refused(String),
    /// A block of the file; block 0 carries the file's size. The empty block
    /// past the last byte ends the transfer.
    Block {
        block: i32,
        size: Option<i32>,
        data: Vec<u8>,
    },
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

fn latin1_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

impl DownloadMsg {
    /// Reads one as `CL_ParseDownload` does: the block as a long, -1 a
    /// redirect, 0 the file's size as a long (negative: a refusal string),
    /// then the block's size as a short and its bytes. The server's own
    /// refusal shape (a short 0 before the long -1) is read whole here;
    /// retail's client misreads it (section 11).
    pub fn read(r: &mut MsgReader) -> DownloadMsg {
        let block = r.read_long();
        if block == -1 {
            let url = latin1(&r.read_string_bytes());
            let size = r.read_long();
            let flags = r.read_long();
            return DownloadMsg::Redirect { url, size, flags };
        }
        if block == REFUSAL_HEAD {
            r.read_short();
            return DownloadMsg::Refused(latin1(&r.read_string_bytes()));
        }
        let mut size = None;
        if block == 0 {
            let s = r.read_long();
            if s < 0 {
                return DownloadMsg::Refused(latin1(&r.read_string_bytes()));
            }
            size = Some(s);
        }
        let len = r.read_short().max(0) as usize;
        let data = (0..len).map(|_| r.read_byte()).collect();
        DownloadMsg::Block { block, size, data }
    }

    /// Writes it with its op, as the 1.7 server does.
    pub fn write(&self, w: &mut MsgWriter) {
        w.write_byte(SVC_DOWNLOAD);
        match self {
            DownloadMsg::Redirect { url, size, flags } => {
                w.write_long(-1);
                w.write_string_bytes(&latin1_bytes(url));
                w.write_long(*size);
                w.write_long(*flags);
            }
            DownloadMsg::Refused(reason) => {
                w.write_short(0);
                w.write_long(-1);
                w.write_string_bytes(&latin1_bytes(reason));
            }
            DownloadMsg::Block { block, size, data } => {
                w.write_long(*block);
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

/// `FS_iwIwd(name, base)` (0x81286c2): `<base>/iw_00` .. `<base>/iw_24`
/// compared whole, or a name whose part up to `localized_` is
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

/// A file a 1.7 client fetches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    /// As requested: `<dir>/<name>.iwd` or `.ff`.
    pub remote: String,
    /// The iwd's unkeyed checksum (`sv_referencedIwds`); `None` for a
    /// fastfile, which the server lists by size.
    pub checksum: Option<i32>,
    /// A fastfile's size (`sv_referencedFFCheckSums`).
    pub size: Option<i64>,
}

fn pairs(systeminfo: &str, names: &str, sums: &str) -> Vec<(String, i64)> {
    let key = |k: &str| info_value_for_key(systeminfo, k).unwrap_or("");
    key(names)
        .split_whitespace()
        .zip(key(sums).split_whitespace())
        .filter_map(|(n, s)| Some((n.to_string(), s.parse().ok()?)))
        .collect()
}

/// The directory a server's own files sit in: `fs_game` when set, else
/// `None` (stock `main`).
pub fn fs_game(systeminfo: &str) -> Option<String> {
    info_value_for_key(systeminfo, "fs_game")
        .filter(|g| !g.is_empty() && !g.eq_ignore_ascii_case("main"))
        .map(str::to_string)
}

/// What `FS_CompareWithServerFiles` puts on the download list: every
/// referenced iwd under the server's `fs_game` (or a `usermaps/` one) whose
/// checksum `have_iwd` lacks, and every referenced fastfile there whose
/// size `ff_size` does not match. A stock `main` iwd is never fetched.
/// INFERRED (`server_file_compare`).
pub fn missing_files(
    systeminfo: &str,
    have_iwd: impl Fn(i32) -> bool,
    ff_size: impl Fn(&str) -> Option<u64>,
) -> Vec<Wanted> {
    let game = fs_game(systeminfo);
    let ours = |name: &str| {
        let lower = name.to_ascii_lowercase().replace('\\', "/");
        let in_game = game.as_ref().is_some_and(|g| {
            lower.starts_with(&format!("{}/", g.to_ascii_lowercase().replace('\\', "/")))
        });
        (in_game || lower.starts_with("usermaps/")) && safe_rel_path(name).is_some()
    };
    let mut out = Vec::new();
    for (name, sum) in pairs(systeminfo, "sv_referencedIwdNames", "sv_referencedIwds") {
        let sum = sum as i32;
        if ours(&name) && !fs_iw_iwd(&name, "main") && !have_iwd(sum) {
            out.push(Wanted {
                remote: format!("{name}.iwd"),
                checksum: Some(sum),
                size: None,
            });
        }
    }
    for (name, size) in pairs(
        systeminfo,
        "sv_referencedFFNames",
        "sv_referencedFFCheckSums",
    ) {
        if ours(&name) && ff_size(&name).is_none_or(|s| s as i64 != size) {
            out.push(Wanted {
                remote: format!("{name}.ff"),
                checksum: None,
                size: Some(size),
            });
        }
    }
    out
}

/// [`missing_files`] against an install at `root`: the iwds it holds in
/// `main/`, the `fs_game` directory and `usermaps/<map>/`, and its
/// fastfiles' sizes at their referenced paths, each matched case-blind.
pub fn missing_in(root: &Path, systeminfo: &str, map: Option<&str>) -> Vec<Wanted> {
    let ci = |rel: &str| crate::install::resolve_ci(root, Path::new(rel));
    let mut dirs = vec![ci("main")];
    dirs.extend(fs_game(systeminfo).map(|g| ci(&g)));
    dirs.extend(map.map(|m| ci(&format!("usermaps/{m}"))));
    let refs: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
    let held = iwd_checksums(&refs);
    missing_files(
        systeminfo,
        |sum| held.contains(&sum),
        |name| {
            let rel = safe_rel_path(&format!("{name}.ff"))?;
            std::fs::metadata(crate::install::resolve_ci(root, &rel))
                .ok()
                .map(|m| m.len())
        },
    )
}

/// A relative path under the homepath for a referenced name: one or more
/// components, none empty, `.`, `..`, or holding a backslash, a colon or a
/// control character.
pub fn safe_rel_path(name: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = name.split('/').collect();
    let ok = |c: &&str| {
        !c.is_empty()
            && *c != "."
            && *c != ".."
            && !c
                .bytes()
                .any(|b| b < b' ' || b == 0x7f || matches!(b, b'\\' | b':'))
    };
    (parts.len() >= 2 && parts.iter().all(ok)).then(|| parts.iter().collect())
}

/// The iwds' unkeyed checksums under `dirs` (as `sv_iwds` lists them).
pub fn iwd_checksums(dirs: &[&Path]) -> Vec<i32> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("iwd")) {
                continue;
            }
            if let Some(sum) = iwd_file_checksum(&p) {
                out.push(sum);
            }
        }
    }
    out
}

/// One iwd's unkeyed checksum; `None` when it does not read as a zip.
pub fn iwd_file_checksum(path: &Path) -> Option<i32> {
    let f = std::fs::File::open(path).ok()?;
    let crcs = vcod_common::pak_checksum::entry_crcs(std::io::BufReader::new(f)).ok()?;
    Some(super::pure::iwd_checksum(&crcs, 0))
}

/// `SV_WriteDownloadToClient`'s blocks a message: the client's rate (capped
/// by `sv_maxRate` when that is 1000 or more) over its snapshot interval in
/// 2 KiB units, rounded up past a whole one (0x816fa21..0x816fa93); 1 at
/// rate 0.
pub fn blocks_per_message(rate: i32, snapshot_msec: i32) -> u32 {
    if rate == 0 {
        return 1;
    }
    ((rate * snapshot_msec / 1000 + 0x800) >> 11).max(1) as u32
}

/// The file in flight for the connect screen: its referenced name, the
/// bytes landed and its size (0 until the server names it: block 0 over
/// UDP, the redirect over HTTP).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DownloadProgress {
    pub file: String,
    pub bytes: u64,
    pub total: u64,
}

/// `sv_wwwDlDisconnected`'s bit in a redirect's flags (`test 0x1` at
/// iw3mp.exe 0x474413): fetch with the server left behind, then reconnect.
const DL_FLAG_DISCON: i32 = 1;
/// The redirect's other bit (`test 0x2` at 0x474308): the URL is for a
/// browser, and the client quits.
const DL_FLAG_URL: i32 = 2;

/// What a download step asks of the caller.
#[derive(Debug, PartialEq, Eq)]
pub enum DownloadEvent {
    /// A reliable command to queue (`download`, `nextdl`, `retransdl`,
    /// `stopdl`, `donedl`, `wwwdl ...`).
    Command(String),
    /// A file, by its referenced name, landed at this path; its size as
    /// the server gave it, for the connect screen's last look.
    Finished(String, PathBuf, u64),
    /// A file was passed over: it could not be written. The next
    /// gamestate's compare lists it again.
    Skipped(String),
    /// The server refused a file: `EXE_...\x15<name>`, or a disconnected
    /// HTTP fetch failed. Retail ends the connection on it.
    Failed(String),
    /// A disconnected HTTP fetch landed (`reconnect`, iw3mp.exe 0x472aa8):
    /// join the server again, which lists what is still missing.
    Reconnect,
}

/// One file in flight.
struct Current {
    wanted: Wanted,
    dest: PathBuf,
    tmp: PathBuf,
    file: std::fs::File,
    /// `clc.downloadBlock` (0xc5b28c): the block expected next.
    block: i32,
    size: u64,
    got: u64,
    /// The HTTP fetch after a redirect (the client's www download flag, 0xc5b6a4).
    www: Option<super::http::HttpFetch>,
}

/// The client's queue after a gamestate (`CL_InitDownloads`, `CL_NextDownload`,
/// `CL_ParseDownload` at iw3mp.exe 0x474440): one file at a time into
/// `root` at its referenced path, `donedl` once the queue is empty, which
/// has the server send the gamestate again (VERIFIED live, section 11).
/// A www redirect is fetched over HTTP ([`super::http::HttpFetch`]),
/// driven by [`Downloads::poll`]; `wwwdl fail` falls back to UDP on the
/// gamestate it brings ([`Downloads::on_gamestate`]).
pub struct Downloads {
    root: PathBuf,
    queue: std::collections::VecDeque<Wanted>,
    current: Option<Current>,
    /// Whether anything was asked for: `donedl` is owed at the end.
    started: bool,
    /// `cl_wwwDownload`; off, a redirect is answered `wwwdl fail` at once.
    www: bool,
    /// The www disconnect flag (0xc5b6a0): `wwwdl bbl8r` went out and the
    /// fetch runs with the server gone.
    disconnected: bool,
    /// `wwwdl fail` went out: the file is asked for again over UDP once
    /// the gamestate the server answers with comes.
    refetch: bool,
}

impl Downloads {
    pub fn new(root: &Path, wanted: Vec<Wanted>) -> Self {
        Downloads {
            root: root.to_path_buf(),
            queue: wanted.into(),
            current: None,
            started: false,
            www: true,
            disconnected: false,
            refetch: false,
        }
    }

    /// `cl_wwwDownload` (on by default, iw3mp.exe 0x46e2b8).
    pub fn with_www(mut self, www: bool) -> Self {
        self.www = www;
        self
    }

    /// Whether `wwwdl bbl8r` went out: the server drops us and the fetch
    /// ends in [`DownloadEvent::Reconnect`] or a failure.
    pub fn is_disconnected(&self) -> bool {
        self.disconnected
    }

    pub fn is_done(&self) -> bool {
        self.queue.is_empty() && self.current.is_none()
    }

    /// The file in flight, for the connect screen.
    pub fn progress(&self) -> Option<DownloadProgress> {
        let c = self.current.as_ref()?;
        Some(DownloadProgress {
            file: c.wanted.remote.clone(),
            bytes: c.www.as_ref().map_or(c.got, |f| f.bytes()),
            total: c.size,
        })
    }

    /// A gamestate came: after `wwwdl fail` the file is asked for again,
    /// and the server, its www transfer marked failed, sends it over UDP
    /// (0x816fc72).
    pub fn on_gamestate(&mut self) -> Vec<DownloadEvent> {
        if !std::mem::take(&mut self.refetch) {
            return Vec::new();
        }
        self.next_download()
    }

    /// `CL_WWWDownload` (iw3mp.exe 0x4729c0), once a frame while a fetch
    /// runs: on its end the spool checked and renamed, `wwwdl done` and
    /// the next file (connected), or [`DownloadEvent::Reconnect`]
    /// (disconnected); on a failure `wwwdl fail` (connected) or the end of
    /// the join with `Download failure while getting '%s'` (0x6d252c).
    pub fn poll(&mut self) -> Vec<DownloadEvent> {
        let Some(cur) = self.current.as_mut() else {
            return Vec::new();
        };
        let Some(done) = cur.www.as_mut().and_then(|f| f.poll()) else {
            return Vec::new();
        };
        let mut cur = self.current.take().unwrap();
        cur.www = None;
        match done {
            Ok(got) => {
                cur.got = got;
                let mut out = Vec::new();
                if !self.disconnected {
                    out.push(DownloadEvent::Command("wwwdl done".into()));
                }
                out.push(cur.finish());
                if self.disconnected {
                    self.queue.clear();
                    self.started = false;
                    out.push(DownloadEvent::Reconnect);
                } else {
                    out.extend(self.next_download());
                }
                out
            }
            Err(e) => {
                log::warn!(
                    "cod4: Download failure while getting '{}': {e}",
                    cur.wanted.remote
                );
                drop(cur.file);
                let _ = std::fs::remove_file(&cur.tmp);
                if self.disconnected {
                    self.queue.clear();
                    self.started = false;
                    return vec![DownloadEvent::Failed(format!(
                        "Download failure while getting '{}'",
                        cur.wanted.remote
                    ))];
                }
                self.fall_back(cur.wanted)
            }
        }
    }

    /// `wwwdl fail`: the file goes back on the queue for the gamestate the
    /// server answers with.
    fn fall_back(&mut self, wanted: Wanted) -> Vec<DownloadEvent> {
        self.queue.push_front(wanted);
        self.refetch = true;
        vec![DownloadEvent::Command("wwwdl fail".into())]
    }

    /// `CL_NextDownload`: the next file's `download`, or `donedl` once
    /// every file came (only when one was asked for).
    pub fn next_download(&mut self) -> Vec<DownloadEvent> {
        use std::io::Write as _;
        let mut out = Vec::new();
        while let Some(wanted) = self.queue.pop_front() {
            let Some(rel) = safe_rel_path(&wanted.remote) else {
                out.push(DownloadEvent::Skipped(format!(
                    "Could not create {}",
                    wanted.remote
                )));
                continue;
            };
            let mut dest = crate::install::resolve_ci(&self.root, &rel);
            if dest.exists()
                && let Some(sum) = wanted.checksum
            {
                // A held iwd of that name with another checksum stays; the
                // listed one lands beside it.
                dest.set_extension(format!("{:08x}.iwd", sum as u32));
            }
            let tmp = dest.with_extension("vcod-dl");
            let file = dest
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::File::create(&tmp))
                .and_then(|mut f| f.flush().map(|()| f));
            match file {
                Ok(file) => {
                    out.push(DownloadEvent::Command(format!(
                        "download {}",
                        wanted.remote
                    )));
                    self.current = Some(Current {
                        wanted,
                        dest,
                        tmp,
                        file,
                        block: 0,
                        size: 0,
                        got: 0,
                        www: None,
                    });
                    self.started = true;
                    return out;
                }
                Err(e) => out.push(DownloadEvent::Skipped(format!(
                    "Could not create {}: {e}",
                    dest.display()
                ))),
            }
        }
        if self.started {
            self.started = false;
            out.push(DownloadEvent::Command("donedl".into()));
        }
        out
    }

    /// `CL_ParseDownload` for one message.
    pub fn on_message(&mut self, msg: &DownloadMsg) -> Vec<DownloadEvent> {
        use std::io::Write as _;
        let Some(cur) = self.current.as_mut() else {
            // 0x474545: blocks nobody asked for are stopped.
            return match msg {
                DownloadMsg::Block { .. } => vec![DownloadEvent::Command("stopdl".into())],
                _ => Vec::new(),
            };
        };
        match msg {
            DownloadMsg::Redirect { url, size, flags } => self.redirect(url, *size, *flags),
            DownloadMsg::Refused(reason) => {
                let cur = self.current.take().unwrap();
                drop(cur.file);
                let _ = std::fs::remove_file(&cur.tmp);
                self.queue.clear();
                self.started = false;
                vec![DownloadEvent::Failed(reason.clone())]
            }
            DownloadMsg::Block { block, .. } if *block != cur.block => {
                // 0x4744fb: a block past the one due asks for it again; an
                // older one is a resend already written.
                if *block > cur.block {
                    vec![DownloadEvent::Command(format!("retransdl {}", cur.block))]
                } else {
                    Vec::new()
                }
            }
            DownloadMsg::Block { block, size, data } => {
                if let Some(s) = size {
                    cur.size = (*s).max(0) as u64;
                }
                if let Err(e) = cur.file.write_all(data) {
                    let cur = self.current.take().unwrap();
                    let _ = std::fs::remove_file(&cur.tmp);
                    let mut out = vec![
                        DownloadEvent::Command("stopdl".into()),
                        DownloadEvent::Skipped(format!("cannot write {}: {e}", cur.tmp.display())),
                    ];
                    out.extend(self.next_download());
                    return out;
                }
                cur.got += data.len() as u64;
                cur.block += 1;
                let mut out = vec![DownloadEvent::Command(format!("nextdl {block}"))];
                if data.is_empty() {
                    let cur = self.current.take().unwrap();
                    out.push(cur.finish());
                    out.extend(self.next_download());
                }
                out
            }
        }
    }
}

impl Downloads {
    /// `CL_ParseDownload`'s block -1 (iw3mp.exe 0x474270..0x474439).
    fn redirect(&mut self, url: &str, size: i32, flags: i32) -> Vec<DownloadEvent> {
        let mut cur = self.current.take().unwrap();
        if flags & DL_FLAG_URL != 0 {
            // 0x474311: the URL opened in a browser, `wwwdl bbl8r` and
            // `quit`.
            drop(cur.file);
            let _ = std::fs::remove_file(&cur.tmp);
            self.queue.clear();
            self.started = false;
            return vec![
                DownloadEvent::Command("wwwdl bbl8r".into()),
                DownloadEvent::Failed(format!("The server sends its files from {url}")),
            ];
        }
        cur.size = size.max(0) as u64;
        log::info!("cod4: Server redirected download: {url}");
        let mut out = vec![DownloadEvent::Command("wwwdl ack".into())];
        let fetch = if self.www {
            super::http::HttpFetch::start(url, &cur.tmp)
        } else {
            Err("cl_wwwDownload is off".into())
        };
        match fetch {
            Ok(f) => {
                cur.www = Some(f);
                self.current = Some(cur);
            }
            Err(e) => {
                // 0x4743e7: `wwwdl fail` and `Failed to initialize
                // download for '%s'`.
                log::warn!("cod4: Failed to initialize download for '{url}': {e}");
                drop(cur.file);
                let _ = std::fs::remove_file(&cur.tmp);
                out.extend(self.fall_back(cur.wanted));
                return out;
            }
        }
        if flags & DL_FLAG_DISCON != 0 {
            out.push(DownloadEvent::Command("wwwdl bbl8r".into()));
            self.disconnected = true;
        }
        out
    }
}

impl Current {
    /// The spool renamed into place, unchecked, as both of 1.7's ends of a
    /// file do (`CL_WWWDownload` 0x472a4b, `CL_ParseDownload` 0x474614):
    /// the next gamestate's `CL_InitDownloads` compares it again.
    fn finish(self) -> DownloadEvent {
        drop(self.file);
        match std::fs::rename(&self.tmp, &self.dest) {
            Ok(()) => DownloadEvent::Finished(self.wanted.remote, self.dest, self.size),
            Err(e) => {
                let _ = std::fs::remove_file(&self.tmp);
                DownloadEvent::Skipped(format!("cannot rename into {}: {e}", self.dest.display()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::parse::UNUSED_HUFF;

    fn round_trip(m: &DownloadMsg) -> DownloadMsg {
        let mut w = MsgWriter::new(&UNUSED_HUFF);
        m.write(&mut w);
        let bytes = w.into_ops();
        assert_eq!(bytes[0], SVC_DOWNLOAD);
        let mut r = MsgReader::from_plain(bytes[1..].to_vec());
        let got = DownloadMsg::read(&mut r);
        assert!(!r.is_overflowed());
        got
    }

    #[test]
    fn every_shape_reads_back() {
        for m in [
            DownloadMsg::Redirect {
                url: "http://x/mods/m/z.iwd".into(),
                size: 1234,
                flags: 1,
            },
            DownloadMsg::Refused("EXE_AUTODL_SERVERDISABLED\x15mods/m/z.iwd".into()),
            DownloadMsg::Block {
                block: 0,
                size: Some(3),
                data: vec![1, 2, 3],
            },
            DownloadMsg::Block {
                block: 70000,
                size: None,
                data: vec![],
            },
        ] {
            assert_eq!(round_trip(&m), m);
        }
    }

    #[test]
    fn the_refusal_is_a_short_then_a_long() {
        let mut w = MsgWriter::new(&UNUSED_HUFF);
        DownloadMsg::Refused("E".into()).write(&mut w);
        assert_eq!(w.into_ops(), [5, 0, 0, 0xff, 0xff, 0xff, 0xff, b'E', 0]);
    }

    #[test]
    fn missing_files_keeps_the_mods_own() {
        let si = "\\fs_game\\mods/mw\\sv_referencedIwds\\1 2 3 \\sv_referencedIwdNames\\main/iw_13 mods/mw/z_mw mods/mw/held\
                  \\sv_referencedFFCheckSums\\100 200 \\sv_referencedFFNames\\mods/mw/mod mp_crash";
        let got = missing_files(si, |s| s == 3, |_| None);
        let names: Vec<&str> = got.iter().map(|w| w.remote.as_str()).collect();
        assert_eq!(names, ["mods/mw/z_mw.iwd", "mods/mw/mod.ff"]);
        assert_eq!(got[0].checksum, Some(2));
        assert_eq!(got[1].size, Some(100));
        let got = missing_files(si, |_| true, |n| (n == "mods/mw/mod").then_some(100));
        assert!(got.is_empty());
    }

    #[test]
    fn paths_stay_home() {
        assert!(safe_rel_path("mods/mw/z.iwd").is_some());
        assert!(safe_rel_path("usermaps/mp_x/mp_x.ff").is_some());
        assert!(safe_rel_path("../x.iwd").is_none());
        assert!(safe_rel_path("x.iwd").is_none());
        assert!(safe_rel_path("mods//x.iwd").is_none());
        assert!(safe_rel_path("c:/x.iwd").is_none());
    }

    /// A redirect to a name holding a colour code (a CoD4x update
    /// backend's `mods/cod4i/^7mod.ff`): the GET carries the `^` raw, as
    /// libwww sends the path, and the file lands under that name with no
    /// size check, as 1.7 renames its spool (cod4-handshake.md 11, 13).
    #[test]
    fn a_colour_coded_name_is_fetched_raw_and_kept() {
        use std::io::{BufRead, Write};
        let root = std::env::temp_dir().join(format!("vcod-dl-caret-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let body: Vec<u8> = (0..5000u32).map(|i| (i * 7) as u8).collect();
        let served = body.clone();
        let server = std::thread::spawn(move || {
            let (sock, _) = l.accept().unwrap();
            let mut r = std::io::BufReader::new(sock);
            let mut first = String::new();
            r.read_line(&mut first).unwrap();
            let mut line = String::new();
            while r.read_line(&mut line).unwrap() > 2 {
                line.clear();
            }
            let mut w = r.into_inner();
            write!(
                w,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                served.len()
            )
            .unwrap();
            w.write_all(&served).unwrap();
            first
        });
        let wanted = Wanted {
            remote: "mods/cod4i/^7mod.ff".into(),
            checksum: None,
            // Not the served size: the file is kept all the same.
            size: Some(body.len() as i64 + 1),
        };
        let mut dls = Downloads::new(&root, vec![wanted]);
        assert_eq!(
            dls.next_download(),
            [DownloadEvent::Command(
                "download mods/cod4i/^7mod.ff".into()
            )]
        );
        let url = format!("http://127.0.0.1:{port}/mods/cod4i/^7mod.ff");
        let evs = dls.on_message(&DownloadMsg::Redirect {
            url,
            size: body.len() as i32,
            flags: 0,
        });
        assert_eq!(evs, [DownloadEvent::Command("wwwdl ack".into())]);
        let t0 = std::time::Instant::now();
        let mut evs = Vec::new();
        while evs.is_empty() && t0.elapsed().as_secs() < 10 {
            evs = dls.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let first = server.join().unwrap();
        assert_eq!(first, "GET /mods/cod4i/^7mod.ff HTTP/1.0\r\n");
        let dest = root.join("mods/cod4i/^7mod.ff");
        assert_eq!(
            evs,
            [
                DownloadEvent::Command("wwwdl done".into()),
                DownloadEvent::Finished(
                    "mods/cod4i/^7mod.ff".into(),
                    dest.clone(),
                    body.len() as u64
                ),
                DownloadEvent::Command("donedl".into()),
            ]
        );
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stock_iwds() {
        assert!(fs_iw_iwd("main/iw_13", "main"));
        assert!(fs_iw_iwd("main/localized_english_iw05", "main"));
        assert!(!fs_iw_iwd("mods/mw/iw_00", "main"));
    }
}
