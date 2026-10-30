//! Plain `http://` GET for the www download redirect
//! (docs/research/cod15-protocol-diff.md, "HTTP downloads"). Retail fetches
//! through libwww, whose protocol modules are HTTP, FTP, Gopher, News and
//! File; there is no TLS, so `https://` is refused like any other scheme.

use anyhow::{Context, bail};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A read that waits longer than this fails the transfer.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Location hops followed before giving up.
const MAX_REDIRECTS: usize = 5;

/// `http://host[:port]/path` split into its parts.
#[derive(Debug, PartialEq, Eq)]
pub struct Url {
    pub host: String,
    pub port: u16,
    pub path: String,
}

/// Parse an absolute `http://` URL. Whitespace and control characters are
/// refused: the path goes into a request line verbatim.
pub fn parse_url(url: &str) -> anyhow::Result<Url> {
    if url.bytes().any(|b| b <= b' ' || b == 0x7f) {
        bail!("bad URL {url:?}");
    }
    let Some(rest) = url
        .get(..7)
        .filter(|s| s.eq_ignore_ascii_case("http://"))
        .map(|_| &url[7..])
    else {
        bail!("{url} is not an http:// URL");
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h,
            p.parse::<u16>()
                .with_context(|| format!("bad port in {url}"))?,
        ),
        None => (authority, 80),
    };
    if host.is_empty() {
        bail!("no host in {url}");
    }
    Ok(Url {
        host: host.to_string(),
        port,
        path: path.to_string(),
    })
}

/// One GET running on its own thread, writing the body into a file.
/// Dropping it stops the thread at its next read.
pub struct Fetch {
    received: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<anyhow::Result<()>>>,
}

impl Fetch {
    /// Start fetching `url` into `out`. A body longer than `limit` bytes, or
    /// shorter than its `Content-Length`, fails the transfer.
    pub fn spawn(url: &str, out: File, limit: u64) -> anyhow::Result<Fetch> {
        let first = parse_url(url)?;
        let received = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (counter, stop) = (received.clone(), cancel.clone());
        let url = url.to_string();
        let handle = std::thread::Builder::new()
            .name("www-download".into())
            .spawn(move || get(&url, first, out, &counter, &stop, limit))
            .context("cannot start the download thread")?;
        Ok(Fetch {
            received,
            cancel,
            handle: Some(handle),
        })
    }

    pub fn received(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    /// `Some` once the transfer ended, with its outcome; `None` while it runs.
    pub fn poll(&mut self) -> Option<anyhow::Result<()>> {
        if !self.handle.as_ref()?.is_finished() {
            return None;
        }
        let h = self.handle.take()?;
        Some(
            h.join()
                .unwrap_or_else(|_| Err(anyhow::anyhow!("download thread panicked"))),
        )
    }
}

impl Drop for Fetch {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn get(
    original: &str,
    mut url: Url,
    mut out: File,
    received: &AtomicU64,
    cancel: &AtomicBool,
    limit: u64,
) -> anyhow::Result<()> {
    for _ in 0..=MAX_REDIRECTS {
        let addr = (url.host.as_str(), url.port)
            .to_socket_addrs()
            .with_context(|| format!("cannot resolve {}", url.host))?
            .next()
            .with_context(|| format!("no address for {}", url.host))?;
        let mut sock = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
            .with_context(|| format!("cannot connect to {addr}"))?;
        sock.set_read_timeout(Some(READ_TIMEOUT))?;
        sock.set_write_timeout(Some(READ_TIMEOUT))?;
        let host = if url.port == 80 {
            url.host.clone()
        } else {
            format!("{}:{}", url.host, url.port)
        };
        // HTTP/1.0 so the body is never chunked and ends at the close. One
        // write: a server may answer and close before a second.
        let req = format!(
            "GET {} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: vcod\r\nAccept: */*\r\n\
             Connection: close\r\n\r\n",
            url.path
        );
        sock.write_all(req.as_bytes())?;
        let mut r = BufReader::new(sock);
        let mut line = String::new();
        r.read_line(&mut line)?;
        let status: u16 = line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .with_context(|| format!("bad HTTP status line {:?}", line.trim_end()))?;
        let mut length = None;
        let mut location = None;
        loop {
            line.clear();
            if r.read_line(&mut line)? == 0 {
                bail!("connection closed inside the HTTP headers");
            }
            let h = line.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                let v = v.trim();
                if k.eq_ignore_ascii_case("content-length") {
                    length = v.parse::<u64>().ok();
                } else if k.eq_ignore_ascii_case("location") {
                    location = Some(v.to_string());
                }
            }
        }
        match status {
            200 => {}
            301 | 302 | 303 | 307 | 308 => {
                let to = location.context("redirect without a Location")?;
                url = if to.starts_with('/') {
                    Url { path: to, ..url }
                } else {
                    parse_url(&to)?
                };
                continue;
            }
            _ => bail!("{original}: HTTP {status}"),
        }
        if length.is_some_and(|n| n > limit) {
            bail!("{original} is {} bytes, over {limit}", length.unwrap());
        }
        let mut buf = [0u8; 16384];
        let mut got = 0u64;
        loop {
            let n = r.read(&mut buf)?;
            if cancel.load(Ordering::Relaxed) {
                bail!("{original}: cancelled");
            }
            if n == 0 {
                break;
            }
            got += n as u64;
            if got > limit {
                bail!("{original} sent more than {limit} bytes");
            }
            out.write_all(&buf[..n])?;
            received.store(got, Ordering::Relaxed);
        }
        if let Some(n) = length
            && got != n
        {
            bail!("{original}: {got} of {n} bytes before the connection closed");
        }
        out.flush()?;
        return Ok(());
    }
    bail!("{original}: more than {MAX_REDIRECTS} redirects")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Instant;

    #[test]
    fn urls_split_into_host_port_and_path() {
        assert_eq!(
            parse_url("http://203.0.113.5:23140/main/foo.pk3").unwrap(),
            Url {
                host: "203.0.113.5".into(),
                port: 23140,
                path: "/main/foo.pk3".into()
            }
        );
        assert_eq!(parse_url("HTTP://example.org").unwrap().path, "/");
        assert_eq!(parse_url("http://example.org/a").unwrap().port, 80);
        assert!(parse_url("https://example.org/a").is_err());
        assert!(parse_url("ftp://example.org/a").is_err());
        assert!(parse_url("http://example.org/a b").is_err());
        assert!(parse_url("http://example.org/a\r\nX: y").is_err());
        assert!(parse_url("http:///a").is_err());
    }

    /// Serve each canned response to one connection, in order, returning
    /// the requests seen.
    fn serve(responses: Vec<Vec<u8>>) -> (u16, JoinHandle<Vec<String>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for resp in responses {
                let (s, _) = l.accept().unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut req = String::new();
                loop {
                    let mut line = String::new();
                    r.read_line(&mut line).unwrap();
                    req.push_str(&line);
                    if line.trim_end().is_empty() {
                        break;
                    }
                }
                seen.push(req);
                let mut s = s;
                s.write_all(&resp).unwrap();
            }
            seen
        });
        (port, h)
    }

    fn run(url: &str, limit: u64) -> (anyhow::Result<()>, Vec<u8>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out");
        let mut f = Fetch::spawn(url, File::create(&path).unwrap(), limit).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let res = loop {
            if let Some(r) = f.poll() {
                break r;
            }
            assert!(Instant::now() < deadline, "fetch hung");
            std::thread::sleep(Duration::from_millis(5));
        };
        (res, std::fs::read(&path).unwrap())
    }

    #[test]
    fn a_200_body_lands_in_the_file_after_a_redirect() {
        let (port, h) = serve(vec![
            b"HTTP/1.1 302 Found\r\nLocation: /real/foo.pk3\r\n\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello".to_vec(),
        ]);
        let (res, body) = run(&format!("http://127.0.0.1:{port}/main/foo.pk3"), 100);
        res.unwrap();
        assert_eq!(body, b"hello");
        let seen = h.join().unwrap();
        assert!(seen[0].starts_with("GET /main/foo.pk3 HTTP/1.0\r\n"));
        assert!(seen[0].contains(&format!("Host: 127.0.0.1:{port}\r\n")));
        assert!(seen[1].starts_with("GET /real/foo.pk3 "));
    }

    #[test]
    fn a_404_a_short_body_and_an_oversize_body_fail() {
        let (port, _h) = serve(vec![b"HTTP/1.0 404 Not Found\r\n\r\n".to_vec()]);
        let (res, _) = run(&format!("http://127.0.0.1:{port}/x"), 100);
        assert!(res.unwrap_err().to_string().contains("404"));

        let (port, _h) = serve(vec![
            b"HTTP/1.0 200 OK\r\nContent-Length: 9\r\n\r\nabc".to_vec(),
        ]);
        let (res, _) = run(&format!("http://127.0.0.1:{port}/x"), 100);
        assert!(res.unwrap_err().to_string().contains("3 of 9"));

        let (port, _h) = serve(vec![b"HTTP/1.0 200 OK\r\n\r\nabcdef".to_vec()]);
        let (res, _) = run(&format!("http://127.0.0.1:{port}/x"), 4);
        assert!(res.is_err());
    }
}
