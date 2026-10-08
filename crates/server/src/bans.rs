//! The ban list. Retail 1.1 keeps none: `banUser` and `banClient` send
//! `banUser <ip>` to the authorize server, which denies that address at its
//! next `getIpAuthorize` (docs/research/cod11-server-handshake.md,
//! "Bans"). vcod has no authorize detour, so it holds the list itself and
//! answers the challenge the way retail relays a denial.

use std::collections::BTreeSet;
use std::net::Ipv4Addr;
use std::path::PathBuf;

#[derive(Default)]
pub struct Bans {
    ips: BTreeSet<Ipv4Addr>,
    /// The file the list lives in, one address per line; `None` keeps it
    /// for this run only.
    path: Option<PathBuf>,
}

impl Bans {
    /// Reads `path` if it exists. Lines that are not a dotted quad are
    /// skipped, so a `//` comment survives a hand edit but not a rewrite.
    pub fn load(path: PathBuf) -> std::io::Result<Bans> {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        Ok(Bans {
            ips: text.lines().filter_map(|l| l.trim().parse().ok()).collect(),
            path: Some(path),
        })
    }

    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        self.ips.contains(&ip)
    }

    /// Adds `ip` and rewrites the file. A write failure is logged; the ban
    /// still holds for this run.
    pub fn add(&mut self, ip: Ipv4Addr) {
        if !self.ips.insert(ip) {
            return;
        }
        let Some(path) = &self.path else {
            return;
        };
        let text: String = self.ips.iter().map(|ip| format!("{ip}\n")).collect();
        if let Err(e) = std::fs::write(path, text) {
            log::error!("writing {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_round_trips_and_skips_junk() {
        let dir = std::env::temp_dir().join(format!("vcod-bans-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ban.txt");
        std::fs::write(&path, "// banned\n10.1.2.3\nnot an ip\n").unwrap();
        let mut bans = Bans::load(path.clone()).unwrap();
        assert!(bans.contains(Ipv4Addr::new(10, 1, 2, 3)));
        bans.add(Ipv4Addr::new(8, 8, 4, 4));
        let again = Bans::load(path.clone()).unwrap();
        assert!(again.contains(Ipv4Addr::new(8, 8, 4, 4)));
        assert!(again.contains(Ipv4Addr::new(10, 1, 2, 3)));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
