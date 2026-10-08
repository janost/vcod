//! `servercache.dat`, where CoDMP.exe keeps the browser's Internet list and
//! the favourites between runs (`LAN_LoadCachedServers` 0x417490,
//! `LAN_SaveServersToCache` 0x417570; docs/research/cod11-front-end.md,
//! "Favourites"). vcod reads and writes only the favourites block and leaves
//! every other byte as it found it.

use std::net::{Ipv4Addr, SocketAddrV4};

/// One `serverInfo_t`.
pub const ENTRY: usize = 0xb8;
/// The list caps: Internet (`MAX_GLOBAL_SERVERS`) and favourites.
pub const MAX_GLOBAL: usize = 0x800;
pub const MAX_FAVORITES: usize = 0x80;
/// Four counts, then the three blocks; the fourth count is this size.
const HEADER: usize = 16;
const BLOCKS: usize = MAX_GLOBAL * ENTRY + MAX_FAVORITES * ENTRY + 0x3000;
pub const FILE_LEN: usize = HEADER + BLOCKS;
const FAVORITES_AT: usize = HEADER + MAX_GLOBAL * ENTRY;

/// `netadrtype_t`'s `NA_IP`.
const NA_IP: i32 = 4;

/// `serverInfo_t` field offsets (CoDMP.exe `LAN_GetServerInfo` 0x417a10).
const HOSTNAME: usize = 0x14;
const MAPNAME: usize = 0x34;
const GAMETYPE: usize = 0x78;
const CLIENTS: usize = 0x98;
const MAX_CLIENTS: usize = 0x9c;
const PING: usize = 0xa8;
const VISIBLE: usize = 0xac;
const PASSWORD: usize = 0xb4;
const STRING: usize = 32;

/// A cached server. Entries vcod cannot address (IPX, loopback) keep their
/// bytes and are written back unchanged.
#[derive(Clone, PartialEq, Eq)]
pub struct Entry(pub [u8; ENTRY]);

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Entry({:?}, {:?})", self.addr(), self.hostname())
    }
}

impl Entry {
    /// What `LAN_AddServer` (0x417640) fills in: the address, the name cut
    /// to 31 bytes, and the visible flag.
    pub fn new(addr: SocketAddrV4, name: &str) -> Entry {
        let mut e = Entry([0; ENTRY]);
        e.0[0..4].copy_from_slice(&NA_IP.to_le_bytes());
        e.0[4..8].copy_from_slice(&addr.ip().octets());
        e.0[18..20].copy_from_slice(&addr.port().to_be_bytes());
        e.set_str(HOSTNAME, name);
        e.set_i32(VISIBLE, 1);
        e
    }

    pub fn addr(&self) -> Option<SocketAddrV4> {
        if self.i32(0) != NA_IP {
            return None;
        }
        let b = &self.0;
        let port = u16::from_be_bytes([b[18], b[19]]);
        Some(SocketAddrV4::new(
            Ipv4Addr::new(b[4], b[5], b[6], b[7]),
            port,
        ))
    }

    pub fn hostname(&self) -> String {
        self.str(HOSTNAME)
    }

    /// What an `infoResponse` writes over a cached server
    /// (`CL_SetServerInfo`): the row's fields and the ping.
    pub fn set_info(&mut self, info: &super::master::ServerInfo, ping: i32) {
        self.set_str(HOSTNAME, &info.hostname);
        self.set_str(MAPNAME, &info.mapname);
        self.set_str(GAMETYPE, &info.gametype);
        self.set_i32(CLIENTS, info.clients as i32);
        self.set_i32(MAX_CLIENTS, info.max_clients as i32);
        self.set_i32(PASSWORD, info.password as i32);
        self.set_i32(PING, ping);
    }

    fn i32(&self, at: usize) -> i32 {
        i32::from_le_bytes(self.0[at..at + 4].try_into().unwrap())
    }

    fn set_i32(&mut self, at: usize, v: i32) {
        self.0[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn str(&self, at: usize) -> String {
        let field = &self.0[at..at + STRING];
        let end = field.iter().position(|&b| b == 0).unwrap_or(STRING);
        field[..end].iter().map(|&b| b as char).collect()
    }

    /// `Q_strncpyz` into a 32-byte field: at most 31 bytes and a NUL.
    fn set_str(&mut self, at: usize, s: &str) {
        let field = &mut self.0[at..at + STRING];
        field.fill(0);
        for (d, c) in field[..STRING - 1].iter_mut().zip(s.chars()) {
            *d = c as u32 as u8;
        }
    }
}

/// The favourites in a cache file; empty when the file is not one
/// (`LAN_LoadCachedServers` drops a file whose size field is wrong).
pub fn read_favorites(file: &[u8]) -> Vec<Entry> {
    if !valid(file) {
        return Vec::new();
    }
    let n = (i32::from_le_bytes(file[4..8].try_into().unwrap()).max(0) as usize).min(MAX_FAVORITES);
    (0..n)
        .map(|i| {
            let at = FAVORITES_AT + i * ENTRY;
            Entry(file[at..at + ENTRY].try_into().unwrap())
        })
        .collect()
}

/// `old` with its favourites replaced by `favorites`; a fresh file, with
/// empty Internet and address lists, when `old` is not a cache file.
pub fn write_favorites(old: Option<&[u8]>, favorites: &[Entry]) -> Vec<u8> {
    let mut out = match old.filter(|f| valid(f)) {
        Some(f) => f.to_vec(),
        None => {
            let mut f = vec![0; FILE_LEN];
            f[12..16].copy_from_slice(&(BLOCKS as i32).to_le_bytes());
            f
        }
    };
    let favorites = &favorites[..favorites.len().min(MAX_FAVORITES)];
    out[4..8].copy_from_slice(&(favorites.len() as i32).to_le_bytes());
    out[FAVORITES_AT..FAVORITES_AT + MAX_FAVORITES * ENTRY].fill(0);
    for (i, e) in favorites.iter().enumerate() {
        let at = FAVORITES_AT + i * ENTRY;
        out[at..at + ENTRY].copy_from_slice(&e.0);
    }
    out
}

fn valid(file: &[u8]) -> bool {
    file.len() >= FILE_LEN && file[12..16] == (BLOCKS as i32).to_le_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_size_matches_retail() {
        // An untouched retail 1.1 install's servercache.dat is 412688 bytes.
        assert_eq!(FILE_LEN, 412688);
        assert_eq!(BLOCKS, 0x64c00);
    }

    #[test]
    fn favourites_round_trip_and_keep_the_rest() {
        let mut old = write_favorites(None, &[]);
        old[0] = 7; // an Internet count the favourites must not disturb
        old[100] = 0xaa;
        let a: SocketAddrV4 = "10.0.0.5:28961".parse().unwrap();
        let file = write_favorites(Some(&old), &[Entry::new(a, "My server")]);
        assert_eq!(file.len(), FILE_LEN);
        assert_eq!((file[0], file[100]), (7, 0xaa));
        let favs = read_favorites(&file);
        assert_eq!(favs.len(), 1);
        assert_eq!(favs[0].addr(), Some(a));
        assert_eq!(favs[0].hostname(), "My server");
        // The address sits as a netadr_t: type 4, IP bytes, port big-endian.
        let at = FAVORITES_AT;
        assert_eq!(&file[at..at + 8], &[4, 0, 0, 0, 10, 0, 0, 5]);
        assert_eq!(&file[at + 18..at + 20], &28961u16.to_be_bytes());
    }

    #[test]
    fn names_are_cut_to_31_bytes() {
        let e = Entry::new("1.2.3.4:1".parse().unwrap(), &"x".repeat(40));
        assert_eq!(e.hostname().len(), 31);
    }

    #[test]
    fn a_short_or_foreign_file_has_no_favourites() {
        assert!(read_favorites(&[0; 64]).is_empty());
        assert!(read_favorites(&vec![1; FILE_LEN]).is_empty());
    }
}
