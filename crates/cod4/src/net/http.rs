//! The client's HTTP fetch for a `sv_wwwDownload` redirect, in place of
//! 1.7's libwww one (`DL_BeginDownload` at iw3mp.exe 0x500ae0,
//! `DL_DownloadLoop` at 0x500e10; cod4-handshake.md 11): a plain
//! `http://host[:port]/path` GET on a thread of its own, the bytes spooled
//! into a file while the netchan keeps running.

use std::io::{BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;
use vcod_common::net::http::{Guarded, read_head};

/// libwww's event timeout for an `http` URL, 30000 ms (0x500b61).
const TIMEOUT: Duration = Duration::from_secs(30);

/// One GET in flight.
pub struct HttpFetch {
    bytes: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<u64, String>>>,
}

/// `http://host[:port]/path` split into the address to dial, the `Host`
/// header and the request path. `None` for any other scheme or a URL
/// carrying `user:pass@`, which vcod does not send.
fn split_url(url: &str) -> Option<(String, String, String)> {
    let rest = url
        .get(..7)
        .filter(|s| s.eq_ignore_ascii_case("http://"))
        .map(|_| &url[7..])?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if host.is_empty() || host.contains('@') {
        return None;
    }
    let dial = if host
        .rsplit_once(':')
        .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
    {
        host.to_string()
    } else {
        format!("{host}:80")
    };
    Some((dial, host.to_string(), path.replace(' ', "%20")))
}

impl HttpFetch {
    /// `DL_BeginDownload`: starts the GET of `url` into `dest` (created or
    /// truncated). Err when the URL is not one this can fetch, as
    /// `DL_BeginDownload` returning 0 (the caller answers `wwwdl fail`).
    /// More than `limit` bytes fail the fetch.
    pub fn start(url: &str, dest: &Path, limit: u64) -> Result<HttpFetch, String> {
        let (dial, host, path) = split_url(url).ok_or_else(|| format!("cannot fetch {url}"))?;
        let dest: PathBuf = dest.to_path_buf();
        let bytes = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (b, s) = (bytes.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("cod4-http".into())
            .spawn(move || get(&dial, &host, &path, &dest, limit, &b, &s))
            .map_err(|e| e.to_string())?;
        Ok(HttpFetch {
            bytes,
            stop,
            thread: Some(thread),
        })
    }

    /// The bytes written so far.
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    /// `DL_DownloadLoop`: `None` while the transfer runs, then once its
    /// end, the bytes written or why it failed.
    pub fn poll(&mut self) -> Option<Result<u64, String>> {
        if !self.thread.as_ref()?.is_finished() {
            return None;
        }
        let t = self.thread.take()?;
        Some(
            t.join()
                .unwrap_or_else(|_| Err("fetch thread panicked".into())),
        )
    }
}

impl Drop for HttpFetch {
    /// An abandoned fetch stops at its next read.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn get(
    dial: &str,
    host: &str,
    path: &str,
    dest: &Path,
    limit: u64,
    bytes: &AtomicU64,
    stop: &AtomicBool,
) -> Result<u64, String> {
    let addr = dial
        .to_socket_addrs()
        .map_err(|e| format!("{dial}: {e}"))?
        .next()
        .ok_or_else(|| format!("{dial}: no address"))?;
    let mut sock =
        TcpStream::connect_timeout(&addr, TIMEOUT).map_err(|e| format!("{dial}: {e}"))?;
    sock.set_read_timeout(Some(TIMEOUT))
        .map_err(|e| e.to_string())?;
    // HTTP/1.0 so the body is never chunked; it ends at the close.
    write!(
        sock,
        "GET {path} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: vcod\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;
    let mut r = BufReader::new(Guarded::head(sock, stop));
    let head = read_head(&mut r).map_err(|e| format!("{e:#}"))?;
    r.get_mut().deadline = None;
    let status = head[0].split_whitespace().nth(1).unwrap_or("");
    if status != "200" {
        return Err(format!("HTTP status {:?}", head[0]));
    }
    let mut length = None;
    for l in &head[1..] {
        if let Some((k, v)) = l.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            length = v.trim().parse::<u64>().ok();
        }
    }
    if let Some(l) = length.filter(|&l| l > limit) {
        return Err(format!("{l} bytes, more than {limit}"));
    }
    let mut out = std::fs::File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut buf = [0u8; 16384];
    let mut got = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let n = r.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return match length {
                Some(l) if l != got => Err(format!("{got} of {l} bytes")),
                _ => Ok(got),
            };
        }
        if got + n as u64 > limit {
            return Err(format!("more than {limit} bytes"));
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        got += n as u64;
        bytes.store(got, Ordering::Relaxed);
    }
    Err("stopped".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A head of endless header lines fails at 64 KiB, before any file is
    /// made.
    #[test]
    fn an_endless_head_fails_bounded() {
        use std::net::TcpListener;
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let _ = s.read(&mut [0u8; 512]);
            let _ = s.write_all(b"HTTP/1.0 200 OK\r\n");
            let _ = s.write_all(&b"X: a\r\n".repeat(1 << 17));
        });
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("x");
        let mut f = HttpFetch::start(&format!("http://127.0.0.1:{port}/x"), &dest, 100).unwrap();
        let res = loop {
            if let Some(r) = f.poll() {
                break r;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(res.unwrap_err().contains("over 65536"));
        assert!(!dest.exists());
    }

    #[test]
    fn urls() {
        assert_eq!(
            split_url("http://127.0.0.1:8080/mods/m/z.iwd"),
            Some((
                "127.0.0.1:8080".into(),
                "127.0.0.1:8080".into(),
                "/mods/m/z.iwd".into()
            ))
        );
        assert_eq!(
            split_url("HTTP://example.test/a b").map(|u| (u.0, u.2)),
            Some(("example.test:80".into(), "/a%20b".into()))
        );
        assert_eq!(split_url("ftp://x/y"), None);
        assert_eq!(split_url("http://u:p@x/y"), None);
    }
}
