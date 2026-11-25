//! Downloads on the CoD4 1.7 wire (docs/research/cod4-handshake.md 11): the
//! `svc_download` (5) shapes `SV_WriteDownloadToClient` (0x816f774) writes
//! and `CL_ParseDownload` (iw3mp.exe 0x474440) reads, `FS_iwIwd`'s stock
//! test, and which referenced files a client lacks. On CoD4x 21 the same
//! queue runs over the side channel's `cmd 5` ([`XDownloadMsg`], [`XAsk`];
//! docs/research/cod4x-protocol.md 7.2).

use std::path::{Path, PathBuf};

use anyhow::{Result, bail, ensure};
use vcod_common::game_dir::confined_rel_path;
use vcod_common::net::codec::{MsgReader, MsgWriter};
use vcod_common::net::download::{Block, Download, UdpFile};
use vcod_common::net::http::Fetch;
use vcod_common::net::info_value_for_key;
pub use vcod_common::pak_checksum::fs_iw_iwd;
use vcod_common::text::latin1_bytes;

use super::reliable::CMD_DOWNLOAD;
use super::wire::Dialect;

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

/// A CoD4x server's checksum record for a referenced file: a 64-byte name,
/// the size, 256 CRC-32s of 2 MiB each and the whole file's CRC-32, last
/// (`FS_CalculateChecksumForFile` 0xbea14, cod4x-protocol.md 7.2.2).
pub const X_SUMS_LEN: usize = 0x448;

/// A CoD4x 21 server's download message, a side channel `cmd 5`
/// (`SV_WriteDownloadToClient` 0x1102c0, cod4x-protocol.md 7.2.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XDownloadMsg {
    /// Op 1: the file's size and, from its checksum record when the server
    /// has one, the whole file's CRC-32.
    Begin { size: i32, crc: Option<u32> },
    /// Op 0: the bytes at `pos`; an empty block ends the bytes asked for.
    Block { pos: i32, data: Vec<u8> },
    /// Op 3: `EXE_AUTODL_...\x15<name>`.
    Refused(String),
    /// Op 2 (`SV_WWWRedirect` 0x10e8e0): the URL, the size and
    /// `sv_wwwDlDisconnected`, as 1.7's block -1.
    Redirect { url: String, size: i32, flags: i32 },
}

impl XDownloadMsg {
    /// A message body, its `cmd` included, to its last byte.
    pub fn read(body: &[u8]) -> Result<XDownloadMsg> {
        let mut r = MsgReader::from_plain(body.to_vec());
        ensure!(r.read_long() == CMD_DOWNLOAD, "not a download message");
        let m = match r.read_byte() {
            0 => {
                let pos = r.read_long();
                // `MSG_WriteShort` of up to 0xffff bytes (0x11073e).
                let n = r.read_short() as u16 as usize;
                let data = (0..n).map(|_| r.read_byte()).collect();
                XDownloadMsg::Block { pos, data }
            }
            1 => {
                let size = r.read_long();
                // `SV_WriteChecksumInfo` (0x1201b0): a long 0, then the record
                // or nothing.
                r.read_long();
                let crc = match body.len().saturating_sub(r.readcount()) {
                    0 => None,
                    X_SUMS_LEN => {
                        // The name, the size and the chunks' CRCs, then the
                        // whole file's.
                        for _ in 0..X_SUMS_LEN / 4 - 1 {
                            r.read_long();
                        }
                        Some(r.read_long() as u32)
                    }
                    n => bail!("a checksum record of {n} bytes"),
                };
                XDownloadMsg::Begin { size, crc }
            }
            2 => {
                let url = latin1(&r.read_string_bytes());
                let size = r.read_long();
                let flags = r.read_long();
                XDownloadMsg::Redirect { url, size, flags }
            }
            3 => XDownloadMsg::Refused(latin1(&r.read_string_bytes())),
            op => bail!("download op {op}"),
        };
        ensure!(
            !r.is_overflowed() && r.readcount() == body.len(),
            "a download message of {} bytes read as {m:?}",
            body.len()
        );
        Ok(m)
    }
}

/// What a CoD4x client asks in a side channel `cmd 5`: the op byte through
/// the table at 0x2c1e60 (`SV_ExecuteDownloadCmd` 0x110ef0,
/// cod4x-protocol.md 7.2.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XAsk {
    /// Op 0 (`SV_BeginDownloadX_f` 0x110c00): a file by its referenced
    /// name, read into 64 bytes.
    Begin(String),
    /// Op 1 (`SV_DoneDownload_f` 0x110bc0): the server sends the gamestate
    /// again.
    Done,
    /// Op 2 (`SV_StopDownload_f` 0x10e9d0).
    Stop,
    /// Op 3 (`SV_SelectDownloadBlocksX_f` 0x10ea80): the bytes `pos..pos +
    /// len`.
    Blocks { pos: i32, len: i32 },
    /// Op 4 (`SV_WWWDownload_Fail_f` 0x110d50): the server sends the
    /// gamestate again and the next ask over the side channel.
    WwwFail,
    /// Op 5 (`SV_WWWDownload_Done_f` 0x110cb0).
    WwwDone,
    /// Op 6 (`SV_WWWDownload_BBL8R_f` 0x110c60): the server drops the client.
    WwwDisconnect,
}

impl XAsk {
    /// The message body, `cmd 5` first.
    pub fn write(&self) -> Vec<u8> {
        let mut b = CMD_DOWNLOAD.to_le_bytes().to_vec();
        let op = match self {
            XAsk::Begin(_) => 0,
            XAsk::Done => 1,
            XAsk::Stop => 2,
            XAsk::Blocks { .. } => 3,
            XAsk::WwwFail => 4,
            XAsk::WwwDone => 5,
            XAsk::WwwDisconnect => 6,
        };
        b.push(op);
        match self {
            XAsk::Begin(name) => {
                b.extend_from_slice(&latin1_bytes(name));
                b.push(0);
            }
            XAsk::Blocks { pos, len } => {
                b.extend_from_slice(&pos.to_le_bytes());
                b.extend_from_slice(&len.to_le_bytes());
            }
            _ => {}
        }
        b
    }
}

/// What the queue asks the server, put in its wire's words by
/// [`Downloads::ask`].
enum Ask<'a> {
    Download(&'a str),
    Next(i32),
    Retransmit(i32),
    Stop,
    Done,
    WwwAck,
    WwwDone,
    WwwFail,
    WwwDisconnect,
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

/// An install's own directories, which a server's `fs_game` may not name:
/// a download never lands in them.
const STOCK_DIRS: [&str; 8] = [
    "main",
    "main_shared",
    "zone",
    "players",
    "usermaps",
    "miles",
    "video",
    "docs",
];

/// libwww's event timeout for an `http` URL, 30000 ms (iw3mp.exe 0x500b61).
const WWW_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The most bytes one download takes, on either transport: vcod's cap,
/// past any mod's iwd or fastfile.
pub const MAX_DOWNLOAD_BYTES: u64 = 1 << 30;

/// The directory a server's own files sit in: `fs_game` when set, else
/// `None` (stock `main`). One that leaves the install or names one of its
/// own directories is `None` too: nothing is fetched into it.
pub fn fs_game(systeminfo: &str) -> Option<String> {
    let g = info_value_for_key(systeminfo, "fs_game")?;
    let rel = confined_rel_path(g)?;
    let top = rel.components().next()?.as_os_str().to_str()?;
    (!STOCK_DIRS.iter().any(|d| top.eq_ignore_ascii_case(d))).then(|| g.to_string())
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
        (in_game || lower.starts_with("usermaps/")) && confined_rel_path(name).is_some()
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
            let rel = confined_rel_path(&format!("{name}.ff"))?;
            std::fs::metadata(crate::install::resolve_ci(root, &rel))
                .ok()
                .map(|m| m.len())
        },
    )
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

/// A file's CRC-32, zlib's (the CoD4x server's `CRC32 for %s is %x`
/// matched it, cod4x-protocol.md 7.2.2); `None` when it does not read.
fn file_crc(path: &Path) -> Option<u32> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut crc = flate2::Crc::new();
    let mut buf = vec![0; 1 << 16];
    loop {
        match f.read(&mut buf).ok()? {
            0 => return Some(crc.sum()),
            n => crc.update(&buf[..n]),
        }
    }
}

/// One iwd's unkeyed checksum; `None` when it does not read as a zip.
pub fn iwd_file_checksum(path: &Path) -> Option<i32> {
    let f = std::fs::File::open(path).ok()?;
    let crcs = vcod_common::pak_checksum::entry_crcs(std::io::BufReader::new(f)).ok()?;
    Some(super::pure::iwd_checksum(&crcs, 0))
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
    /// CoD4x 21: a side channel message to send in its place.
    Side(XAsk),
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
    /// The spool, `clc.downloadBlock` (0xc5b28c) and the announced size.
    udp: UdpFile,
    /// The HTTP fetch after a redirect (the client's www download flag, 0xc5b6a4).
    www: Option<Fetch>,
    /// CoD4x: the whole file's CRC-32 off the server's checksum record.
    crc: Option<u32>,
}

/// The client's queue after a gamestate (`CL_InitDownloads`, `CL_NextDownload`,
/// `CL_ParseDownload` at iw3mp.exe 0x474440): one file at a time into
/// `root` at its referenced path, `donedl` once the queue is empty, which
/// has the server send the gamestate again (VERIFIED live, section 11).
/// A www redirect is fetched over HTTP ([`Fetch`], with libwww's 30 s),
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
    /// The wire the asks go out on: 1.7's reliable commands or CoD4x's
    /// side channel.
    dialect: Dialect,
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
            dialect: Dialect::V17,
        }
    }

    /// The wire the asks go out on ([`DownloadEvent::Side`] on CoD4x 21).
    pub fn with_dialect(mut self, dialect: Dialect) -> Self {
        self.dialect = dialect;
        self
    }

    /// `a` in the wire's words: a 1.7 reliable command, or a CoD4x side
    /// channel message, none for an ask CoD4x has no op for.
    fn ask(&self, a: Ask) -> Option<DownloadEvent> {
        if self.dialect == Dialect::X21 {
            return match a {
                Ask::Download(name) => Some(XAsk::Begin(name.into())),
                Ask::Stop => Some(XAsk::Stop),
                Ask::Done => Some(XAsk::Done),
                Ask::WwwDone => Some(XAsk::WwwDone),
                Ask::WwwFail => Some(XAsk::WwwFail),
                Ask::WwwDisconnect => Some(XAsk::WwwDisconnect),
                // No op acknowledges a redirect, and the bytes come as
                // asked for ([`Self::on_side`]).
                Ask::WwwAck | Ask::Next(_) | Ask::Retransmit(_) => None,
            }
            .map(DownloadEvent::Side);
        }
        Some(DownloadEvent::Command(match a {
            Ask::Download(name) => format!("download {name}"),
            Ask::Next(block) => format!("nextdl {block}"),
            Ask::Retransmit(block) => format!("retransdl {block}"),
            Ask::Stop => "stopdl".into(),
            Ask::Done => "donedl".into(),
            Ask::WwwAck => "wwwdl ack".into(),
            Ask::WwwDone => "wwwdl done".into(),
            Ask::WwwFail => "wwwdl fail".into(),
            Ask::WwwDisconnect => "wwwdl bbl8r".into(),
        }))
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
            bytes: c.www.as_ref().map_or(c.udp.spool.received, Fetch::received),
            total: c.udp.size,
        })
    }

    /// A gamestate came: after `wwwdl fail` the file is asked for again,
    /// and the server, its www transfer marked failed, sends it over UDP
    /// (0x816fc72).
    pub fn on_gamestate(&mut self) -> Vec<DownloadEvent> {
        // CoD4x answers a name off its lists with the gamestate, not a
        // refusal (`SV_BeginDownloadX_f` 0x110c48).
        if self.dialect == Dialect::X21
            && let Some(cur) = self.current.take_if(|c| c.udp.size == 0)
        {
            let why = format!("The server does not serve {}", cur.wanted.remote);
            return vec![self.abandon(why)];
        }
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
        let got = cur.www.take().map_or(0, |f| f.received());
        match done {
            Ok(()) => {
                cur.udp.spool.received = got;
                let mut out = Vec::new();
                if !self.disconnected {
                    out.extend(self.ask(Ask::WwwDone));
                }
                let landed = cur.finish();
                let failed = matches!(landed, DownloadEvent::Failed(_));
                out.push(landed);
                if failed {
                    self.queue.clear();
                    self.started = false;
                } else if self.disconnected {
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
        self.ask(Ask::WwwFail).into_iter().collect()
    }

    /// `CL_NextDownload`: the next file's `download`, or `donedl` once
    /// every file came (only when one was asked for).
    pub fn next_download(&mut self) -> Vec<DownloadEvent> {
        let mut out = Vec::new();
        while let Some(wanted) = self.queue.pop_front() {
            let Some(rel) = confined_rel_path(&wanted.remote) else {
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
            match Download::replacing(&wanted.remote, &dest) {
                Ok(spool) => {
                    out.extend(self.ask(Ask::Download(&wanted.remote)));
                    self.current = Some(Current {
                        wanted,
                        udp: UdpFile::new(spool),
                        www: None,
                        crc: None,
                    });
                    self.started = true;
                    return out;
                }
                Err(e) => out.push(DownloadEvent::Skipped(format!("{e:#}"))),
            }
        }
        if self.started {
            self.started = false;
            out.extend(self.ask(Ask::Done));
        }
        out
    }

    /// `CL_ParseDownload` for one message.
    pub fn on_message(&mut self, msg: &DownloadMsg) -> Vec<DownloadEvent> {
        if self.current.is_none() {
            // 0x474545: blocks nobody asked for are stopped.
            return match msg {
                DownloadMsg::Block { .. } => self.ask(Ask::Stop).into_iter().collect(),
                _ => Vec::new(),
            };
        }
        let (block, size, data) = match msg {
            DownloadMsg::Redirect { url, size, flags } => {
                return self.redirect(url, *size, *flags);
            }
            DownloadMsg::Refused(reason) => return self.refused(reason),
            DownloadMsg::Block { block, size, data } => (*block, *size, data),
        };
        // 0x4744fb: a block past the one due asks for it again; an older
        // one is a resend already written. vcod's bound is CoD1's and
        // CoD2's: the announced size, at most `MAX_DOWNLOAD_BYTES`.
        let cur = self.current.as_mut().unwrap();
        match cur.udp.take(block, size, data, MAX_DOWNLOAD_BYTES) {
            Block::Old => Vec::new(),
            Block::Ahead(due) => self.ask(Ask::Retransmit(due)).into_iter().collect(),
            Block::TooBig(why) => self.stop(why),
            Block::Unwritable(why) => self.pass_over(why),
            Block::Took { block, eof } => {
                let mut out: Vec<_> = self.ask(Ask::Next(block)).into_iter().collect();
                if eof {
                    let cur = self.current.take().unwrap();
                    out.extend(self.land(cur));
                }
                out
            }
        }
    }
}

impl Downloads {
    /// A CoD4x server's download message: its `Begin` asks for every byte
    /// at once, which the side channel brings in order, then an empty block
    /// (`SV_WriteDownloadToClient` 0x110763..0x1107ca, cod4x-protocol.md
    /// 7.2.3).
    pub fn on_side(&mut self, msg: &XDownloadMsg) -> Vec<DownloadEvent> {
        let Some(cur) = self.current.as_mut() else {
            return match msg {
                XDownloadMsg::Block { .. } => self.ask(Ask::Stop).into_iter().collect(),
                _ => Vec::new(),
            };
        };
        match msg {
            XDownloadMsg::Redirect { url, size, flags } => self.redirect(url, *size, *flags),
            XDownloadMsg::Refused(reason) => self.refused(reason),
            XDownloadMsg::Begin { size, crc } => {
                cur.udp.size = (*size).max(0) as u64;
                cur.crc = *crc;
                if cur.udp.size > MAX_DOWNLOAD_BYTES {
                    let why = format!("{} announced {size} bytes", cur.wanted.remote);
                    return self.stop(why);
                }
                let (pos, len) = (cur.udp.spool.received as i32, *size);
                vec![DownloadEvent::Side(XAsk::Blocks { pos, len })]
            }
            XDownloadMsg::Block { pos, data } => {
                let got = cur.udp.spool.received;
                if i64::from(*pos) != got as i64 {
                    let why = format!(
                        "{}: a block at {pos} with {got} bytes in",
                        cur.wanted.remote
                    );
                    return self.stop(why);
                }
                if data.is_empty() {
                    let cur = self.current.take().unwrap();
                    return self.land(cur);
                }
                let landed = got + data.len() as u64;
                if landed > cur.udp.size {
                    let why = format!(
                        "{} sent {landed} bytes, {} announced",
                        cur.wanted.remote, cur.udp.size
                    );
                    return self.stop(why);
                }
                match cur.udp.spool.accept_block(data) {
                    Ok(()) => Vec::new(),
                    Err(e) => self.pass_over(format!("{e:#}")),
                }
            }
        }
    }

    /// The server refused the file: the queue ends with its reason.
    fn refused(&mut self, reason: &str) -> Vec<DownloadEvent> {
        self.current = None;
        self.queue.clear();
        self.started = false;
        vec![DownloadEvent::Failed(reason.to_string())]
    }

    /// The file in flight stopped and given up for `why`: the queue ends.
    fn stop(&mut self, why: String) -> Vec<DownloadEvent> {
        self.current = None;
        let mut out: Vec<_> = self.ask(Ask::Stop).into_iter().collect();
        out.push(self.abandon(why));
        out
    }

    /// The file in flight stopped and passed over for `why`; the next one
    /// asked for.
    fn pass_over(&mut self, why: String) -> Vec<DownloadEvent> {
        self.current = None;
        let mut out: Vec<_> = self.ask(Ask::Stop).into_iter().collect();
        out.push(DownloadEvent::Skipped(why));
        out.extend(self.next_download());
        out
    }

    /// The file renamed into place and the next one asked for; a file
    /// that fails its check ends the queue.
    fn land(&mut self, cur: Current) -> Vec<DownloadEvent> {
        match cur.finish() {
            e @ DownloadEvent::Failed(_) => {
                self.queue.clear();
                self.started = false;
                vec![e]
            }
            e => {
                let mut out = vec![e];
                out.extend(self.next_download());
                out
            }
        }
    }

    /// The file in flight given up for `why`, its spool gone with it: the
    /// queue ended.
    fn abandon(&mut self, why: String) -> DownloadEvent {
        self.queue.clear();
        self.started = false;
        DownloadEvent::Failed(why)
    }

    /// `CL_ParseDownload`'s block -1 (iw3mp.exe 0x474270..0x474439).
    fn redirect(&mut self, url: &str, size: i32, flags: i32) -> Vec<DownloadEvent> {
        let mut cur = self.current.take().unwrap();
        if flags & DL_FLAG_URL != 0 {
            // 0x474311: the URL opened in a browser, `wwwdl bbl8r` and
            // `quit`.
            self.queue.clear();
            self.started = false;
            let mut out: Vec<_> = self.ask(Ask::WwwDisconnect).into_iter().collect();
            out.push(DownloadEvent::Failed(format!(
                "The server sends its files from {url}"
            )));
            return out;
        }
        cur.udp.size = size.max(0) as u64;
        log::info!("cod4: Server redirected download: {url}");
        let mut out: Vec<_> = self.ask(Ask::WwwAck).into_iter().collect();
        let fetch = if self.www {
            // libwww sends a space in the path as `%20`.
            let url = url.replace(' ', "%20");
            cur.udp
                .spool
                .spool()
                .map_err(anyhow::Error::from)
                .and_then(|f| Fetch::spawn_timed(&url, f, cur.limit(), WWW_TIMEOUT))
        } else {
            Err(anyhow::anyhow!("cl_wwwDownload is off"))
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
                out.extend(self.fall_back(cur.wanted));
                return out;
            }
        }
        if flags & DL_FLAG_DISCON != 0 {
            out.extend(self.ask(Ask::WwwDisconnect));
            self.disconnected = true;
        }
        out
    }
}

impl Current {
    /// The most bytes an HTTP fetch may take: the announced size, under
    /// [`MAX_DOWNLOAD_BYTES`].
    fn limit(&self) -> u64 {
        match self.udp.size {
            0 => MAX_DOWNLOAD_BYTES,
            s => s.min(MAX_DOWNLOAD_BYTES),
        }
    }

    /// Why the landed spool is not the file the server listed: short of
    /// its announced size, a fastfile off its listed size, an iwd off its
    /// listed checksum.
    fn check(&self) -> Option<String> {
        let (got, size) = (self.udp.spool.received, self.udp.size);
        if size > 0 && got != size {
            return Some(format!("{got} of {size} bytes"));
        }
        if let Some(listed) = self.wanted.size
            && got as i64 != listed
        {
            return Some(format!("{got} bytes, the server lists {listed}"));
        }
        if let Some(sum) = self.wanted.checksum
            && iwd_file_checksum(self.udp.spool.spooled()) != Some(sum)
        {
            return Some(format!("not the iwd of checksum {sum}"));
        }
        if let Some(crc) = self.crc
            && file_crc(self.udp.spool.spooled()) != Some(crc)
        {
            return Some(format!("not the file of CRC-32 {crc:08x}"));
        }
        None
    }

    /// The spool renamed into place once it is the file the server listed
    /// ([`Self::check`]); 1.7 renames it unchecked (`CL_WWWDownload`
    /// 0x472a4b, `CL_ParseDownload` 0x474614) and compares it on the next
    /// gamestate. A spool that fails is removed and ends the queue.
    fn finish(self) -> DownloadEvent {
        if let Some(why) = self.check() {
            return DownloadEvent::Failed(format!("{}: {why}", self.wanted.remote));
        }
        let (dest, size) = (self.udp.spool.dest().to_path_buf(), self.udp.size);
        match self.udp.spool.land() {
            Ok(()) => DownloadEvent::Finished(self.wanted.remote, dest, size),
            Err(e) => DownloadEvent::Skipped(format!("{e:#}")),
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

    /// Referenced names go through the shared file-name rule: empty
    /// components fold, `..` anywhere and a drive colon list nothing.
    #[test]
    fn referenced_names_stay_under_the_install() {
        let si = "\\sv_referencedFFCheckSums\\1 2 3 \\sv_referencedFFNames\\usermaps/mp_x//mp_x \
                  usermaps/../zone/common_mp usermaps/c:x";
        let names: Vec<String> = missing_files(si, |_| true, |_| None)
            .into_iter()
            .map(|w| w.remote)
            .collect();
        assert_eq!(names, ["usermaps/mp_x//mp_x.ff"]);
        let (tmp, mut dls) = queued(Wanted {
            remote: names[0].clone(),
            checksum: None,
            size: None,
        });
        dls.on_message(&DownloadMsg::Block {
            block: 0,
            size: Some(0),
            data: Vec::new(),
        });
        assert!(tmp.path().join("usermaps/mp_x/mp_x.ff").is_file());
    }

    /// A redirect to a name holding a colour code (a CoD4x update
    /// backend's `mods/cod4i/^7mod.ff`): the GET carries the `^` raw, as
    /// libwww sends the path, and the file lands under that name
    /// (cod4-handshake.md 11, 13).
    #[test]
    fn a_colour_coded_name_is_fetched_raw_and_kept() {
        use std::io::{BufRead, Write};
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
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
            size: Some(body.len() as i64),
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
    }

    #[test]
    fn fs_game_names_no_stock_directory() {
        assert_eq!(fs_game("\\fs_game\\mods/mw").as_deref(), Some("mods/mw"));
        for bad in [
            "",
            "main",
            "Zone",
            "zone/english",
            "players",
            "usermaps",
            "../x",
            "/x",
            "c:/x",
        ] {
            assert_eq!(fs_game(&format!("\\fs_game\\{bad}")), None, "{bad}");
        }
        // A server naming the install's own zones fetches nothing.
        let si = "\\fs_game\\zone\\sv_referencedFFCheckSums\\5 \\sv_referencedFFNames\\zone/english/common_mp";
        assert!(missing_files(si, |_| false, |_| None).is_empty());
    }

    /// A scratch root, removed on drop, and one queued file, its `download` sent.
    fn queued(wanted: Wanted) -> (tempfile::TempDir, Downloads) {
        let root = tempfile::tempdir().unwrap();
        let mut dls = Downloads::new(root.path(), vec![wanted]);
        assert!(
            matches!(&dls.next_download()[..], [DownloadEvent::Command(c)] if c.starts_with("download "))
        );
        (root, dls)
    }

    #[test]
    fn a_transfer_past_its_announced_size_aborts_and_removes_its_spool() {
        let ff = Wanted {
            remote: "mods/m/mod.ff".into(),
            checksum: None,
            size: Some(4),
        };
        let (tmp, mut dls) = queued(ff);
        let root = tmp.path();
        let block = |block, size, data: &[u8]| DownloadMsg::Block {
            block,
            size,
            data: data.to_vec(),
        };
        assert_eq!(
            dls.on_message(&block(0, Some(4), b"abc")),
            [DownloadEvent::Command("nextdl 0".into())]
        );
        let evs = dls.on_message(&block(1, None, b"de"));
        assert_eq!(evs[0], DownloadEvent::Command("stopdl".into()));
        assert!(
            matches!(&evs[1], DownloadEvent::Failed(w) if w.contains("5 bytes, 4 announced")),
            "{evs:?}"
        );
        assert!(dls.is_done());
        assert!(!root.join("mods/m/mod.ff.tmp").exists());
        assert!(!root.join("mods/m/mod.ff").exists());
    }

    /// An announced size past the cap fails on block 0, before a byte of
    /// it is written.
    #[test]
    fn an_announced_size_past_the_cap_fails_at_once() {
        let ff = Wanted {
            remote: "mods/m/mod.ff".into(),
            checksum: None,
            size: None,
        };
        let (_tmp, mut dls) = queued(ff);
        let evs = dls.on_message(&DownloadMsg::Block {
            block: 0,
            size: Some(MAX_DOWNLOAD_BYTES as i32 + 1),
            data: vec![7; 10],
        });
        assert_eq!(evs[0], DownloadEvent::Command("stopdl".into()));
        assert!(matches!(&evs[1], DownloadEvent::Failed(_)), "{evs:?}");
        assert!(dls.is_done());
    }

    #[test]
    fn an_iwd_off_its_listed_checksum_is_removed() {
        let iwd = Wanted {
            remote: "mods/m/z.iwd".into(),
            checksum: Some(1234),
            size: None,
        };
        let (tmp, mut dls) = queued(iwd);
        let root = tmp.path();
        let block = |block, size, data: &[u8]| DownloadMsg::Block {
            block,
            size,
            data: data.to_vec(),
        };
        dls.on_message(&block(0, Some(3), b"zip"));
        let evs = dls.on_message(&block(1, None, b""));
        assert!(
            matches!(&evs[1], DownloadEvent::Failed(w) if w.contains("checksum 1234")),
            "{evs:?}"
        );
        assert_eq!(evs.len(), 2);
        assert!(!root.join("mods/m/z.iwd.tmp").exists());
        assert!(!root.join("mods/m/z.iwd").exists());
    }

    /// A CoD4x server's `cmd 5` body: the length word's message after it.
    fn x_body(op: u8, rest: &[u8]) -> Vec<u8> {
        [&CMD_DOWNLOAD.to_le_bytes()[..], &[op], rest].concat()
    }

    /// A begin with `size` and a checksum record whose whole-file CRC-32
    /// is `crc` (`SV_WriteChecksumInfo` 0x1201b0).
    fn x_begin(size: i32, crc: u32) -> Vec<u8> {
        let mut sums = vec![0; X_SUMS_LEN];
        sums[X_SUMS_LEN - 4..].copy_from_slice(&crc.to_le_bytes());
        x_body(1, &[&size.to_le_bytes()[..], &[0; 4], &sums].concat())
    }

    /// Each server op reads to its body's end, a begin with or without its
    /// checksum record; a record of another length and a body past the
    /// op's fields fail. The asks write the ops the server's reader takes
    /// (cod4x-protocol.md 7.2.1, 7.2.2).
    #[test]
    fn cod4x_download_messages_read_and_write_as_the_server_does() {
        let block = x_body(
            0,
            &[&7i32.to_le_bytes()[..], &[0xb0, 0x04], &[9; 0x4b0]].concat(),
        );
        assert_eq!(
            XDownloadMsg::read(&block).unwrap(),
            XDownloadMsg::Block {
                pos: 7,
                data: vec![9; 1200]
            }
        );
        assert_eq!(
            XDownloadMsg::read(&x_begin(2970, 0xb434_1929)).unwrap(),
            XDownloadMsg::Begin {
                size: 2970,
                crc: Some(0xb434_1929)
            }
        );
        let bare = x_body(1, &[&5i32.to_le_bytes()[..], &[0; 4]].concat());
        assert_eq!(
            XDownloadMsg::read(&bare).unwrap(),
            XDownloadMsg::Begin { size: 5, crc: None }
        );
        let mut short = x_begin(5, 1);
        short.pop();
        assert!(XDownloadMsg::read(&short).is_err());
        let refused = x_body(3, b"EXE_AUTODL_FILENOTONSERVER\x15mods/x/z.iwd\0");
        assert_eq!(
            XDownloadMsg::read(&refused).unwrap(),
            XDownloadMsg::Refused("EXE_AUTODL_FILENOTONSERVER\x15mods/x/z.iwd".into())
        );
        let url = x_body(
            2,
            &[&b"http://h/mods/x/z.iwd\0"[..], &[3, 0, 0, 0, 1, 0, 0, 0]].concat(),
        );
        assert_eq!(
            XDownloadMsg::read(&url).unwrap(),
            XDownloadMsg::Redirect {
                url: "http://h/mods/x/z.iwd".into(),
                size: 3,
                flags: 1
            }
        );
        let mut long = url.clone();
        long.push(0);
        assert!(XDownloadMsg::read(&long).is_err());
        assert_eq!(
            XAsk::Begin("mods/x/z.iwd".into()).write(),
            x_body(0, b"mods/x/z.iwd\0")
        );
        assert_eq!(
            XAsk::Blocks { pos: 1, len: 2 }.write(),
            x_body(3, &[1, 0, 0, 0, 2, 0, 0, 0])
        );
        assert_eq!(XAsk::Done.write(), x_body(1, &[]));
    }

    /// On CoD4x the queue asks on the side channel: the file by name, its
    /// every byte once the size came, then `Done` past the empty block; no
    /// per-block answer goes. A file off the record's CRC-32 is removed and
    /// ends the queue.
    #[test]
    fn a_cod4x_file_comes_in_the_bytes_asked_for() {
        let tmp = tempfile::tempdir().unwrap();
        let ff = |name: &str| Wanted {
            remote: format!("mods/x/{name}.ff"),
            checksum: None,
            size: Some(9),
        };
        let mut dls =
            Downloads::new(tmp.path(), vec![ff("mod"), ff("bad")]).with_dialect(Dialect::X21);
        let side = |a| DownloadEvent::Side(a);
        assert_eq!(
            dls.next_download(),
            [side(XAsk::Begin("mods/x/mod.ff".into()))]
        );
        let crc = 0xcbf4_3926; // "123456789"'s
        let begin = XDownloadMsg::read(&x_begin(9, crc)).unwrap();
        assert_eq!(dls.on_side(&begin), [side(XAsk::Blocks { pos: 0, len: 9 })]);
        let block = |pos, data: &[u8]| XDownloadMsg::Block {
            pos,
            data: data.to_vec(),
        };
        assert!(dls.on_side(&block(0, b"12345")).is_empty());
        assert!(dls.on_side(&block(5, b"6789")).is_empty());
        let dest = tmp.path().join("mods/x/mod.ff");
        assert_eq!(
            dls.on_side(&block(9, b"")),
            [
                DownloadEvent::Finished("mods/x/mod.ff".into(), dest.clone(), 9),
                side(XAsk::Begin("mods/x/bad.ff".into())),
            ]
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"123456789");
        dls.on_side(&begin);
        dls.on_side(&block(0, b"123456780"));
        let evs = dls.on_side(&block(9, b""));
        assert!(
            matches!(&evs[..], [DownloadEvent::Failed(w)] if w.contains("CRC-32 cbf43926")),
            "{evs:?}"
        );
        assert!(dls.is_done());
        assert!(!tmp.path().join("mods/x/bad.ff").exists());
    }

    /// A CoD4x server answers a name off its lists with the gamestate
    /// (`SV_BeginDownloadX_f` 0x110c48): the queue ends there.
    #[test]
    fn a_cod4x_gamestate_in_place_of_the_begin_ends_the_queue() {
        let tmp = tempfile::tempdir().unwrap();
        let iwd = Wanted {
            remote: "mods/x/z.iwd".into(),
            checksum: Some(1),
            size: None,
        };
        let mut dls = Downloads::new(tmp.path(), vec![iwd]).with_dialect(Dialect::X21);
        dls.next_download();
        let evs = dls.on_gamestate();
        assert!(
            matches!(&evs[..], [DownloadEvent::Failed(w)] if w.contains("does not serve mods/x/z.iwd")),
            "{evs:?}"
        );
        assert!(dls.is_done());
    }

    #[test]
    fn stock_iwds() {
        assert!(fs_iw_iwd("main/iw_13", "main"));
        assert!(fs_iw_iwd("main/localized_english_iw05", "main"));
        assert!(!fs_iw_iwd("mods/mw/iw_00", "main"));
    }
}
