//! The server cache, where each game keeps the browser's Internet list and
//! the favourites between runs (`LAN_LoadCachedServers`,
//! `LAN_SaveServersToCache`; docs/research/cod11-front-end.md 8,
//! "Favourites", and "The other games' caches"). Two file shapes: 1.1's,
//! and the one 1.5, UO, CoD2 and 1.7 share with their own entry sizes and
//! version words. vcod writes only the favourites and leaves every other
//! byte as it found it; a file that is not a cache of the install's own
//! layout it neither reads nor writes.

use std::net::{Ipv4Addr, SocketAddrV4};

/// Which game's file: the shape, the entry size and where the fields sit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// 1.1 (CoDMP.exe 0x417490, 0x417570): `servercache.dat`.
    Cod11,
    /// 1.5 (CoDMP.exe 0x4190d0, 0x419140): `servercache.dat`.
    Cod15,
    /// UO 1.51 (CoDUOMP.exe 0x41a5b0, 0x41a620): `uoservercache.dat`.
    Uo,
    /// CoD2 1.3 (CoD2MP_s.exe 0x4b56e0, 0x4b5750): `servercache.dat`.
    Cod2,
    /// CoD4 1.7 (iw3mp.exe 0x4764d0, 0x476540): `servercache.dat`,
    /// version 3 (cod4-front-end.md).
    Cod4,
}

/// The list caps: 1.1's Internet list (`MAX_GLOBAL_SERVERS`), the later
/// games' (`0x4e20`), and the favourites.
const MAX_GLOBAL_11: usize = 0x800;
const MAX_GLOBAL: usize = 20000;
pub const MAX_FAVORITES: usize = 0x80;
/// Four ints before the blocks.
const HEADER: usize = 16;
/// 1.1's third block, the master's overflow addresses.
const OVERFLOW_11: usize = 0x3000;

/// `netadrtype_t`'s `NA_IP`, the same in every game.
const NA_IP: i32 = 4;
const STRING: usize = 32;

impl Layout {
    /// The file the game reads, beside its executable.
    pub fn file_name(self) -> &'static str {
        match self {
            Layout::Uo => "uoservercache.dat",
            _ => "servercache.dat",
        }
    }

    /// One `serverInfo_t`.
    pub fn entry(self) -> usize {
        match self {
            Layout::Cod11 => 0xb8,
            Layout::Cod15 | Layout::Cod2 => 0x94,
            Layout::Uo => 0x98,
            Layout::Cod4 => 0x9c,
        }
    }

    /// The later layouts' first header int, which their loaders compare.
    fn version(self) -> i32 {
        match self {
            Layout::Cod4 => 3,
            _ => 1,
        }
    }

    /// The most favourites a file of this layout can hold and still load:
    /// the later games' loaders refuse a count of `0x80` (`jae`), though
    /// `LAN_AddServer` adds up to it.
    pub fn max_favorites(self) -> usize {
        match self {
            Layout::Cod11 => MAX_FAVORITES,
            _ => MAX_FAVORITES - 1,
        }
    }

    /// The size the fourth header int must hold: the blocks after it.
    fn blocks(self) -> usize {
        match self {
            Layout::Cod11 => {
                MAX_GLOBAL_11 * self.entry() + MAX_FAVORITES * self.entry() + OVERFLOW_11
            }
            _ => MAX_GLOBAL * self.entry() + MAX_FAVORITES * self.entry(),
        }
    }

    /// The file's whole length.
    pub fn file_len(self) -> usize {
        HEADER + self.blocks()
    }

    /// 1.1: Internet count, favourite count, overflow count, size; the
    /// others: version, Internet count, favourite count, size.
    fn internet_count_at(self) -> usize {
        match self {
            Layout::Cod11 => 0,
            _ => 4,
        }
    }

    fn favorite_count_at(self) -> usize {
        match self {
            Layout::Cod11 => 4,
            _ => 8,
        }
    }

    fn favorites_at(self) -> usize {
        HEADER
            + self.entry()
                * match self {
                    Layout::Cod11 => MAX_GLOBAL_11,
                    _ => MAX_GLOBAL,
                }
    }

    fn hostname(self) -> usize {
        match self {
            Layout::Cod11 => 0x14,
            Layout::Cod15 | Layout::Cod2 => 0x2a,
            Layout::Uo => 0x2e,
            Layout::Cod4 => 0x31,
        }
    }

    /// The big-endian port in the entry's `netadr_t`: after CoD1's IPX
    /// bytes, right after the IPv4 address on 1.7.
    fn port_at(self) -> usize {
        match self {
            Layout::Cod4 => 8,
            _ => 18,
        }
    }

    /// The visible flag `LAN_AddServer` sets: an int on 1.1, a byte after.
    fn visible(self) -> usize {
        match self {
            Layout::Cod11 => 0xac,
            Layout::Cod4 => 0x1b,
            _ => 0x17,
        }
    }

    fn header(self, file: &[u8]) -> [i32; 4] {
        std::array::from_fn(|i| i32::from_le_bytes(file[i * 4..i * 4 + 4].try_into().unwrap()))
    }

    /// Whether `file` loads as this layout: 1.1 checks the size int alone;
    /// the others the version, both counts and the size.
    fn valid(self, file: &[u8]) -> bool {
        if file.len() != self.file_len() {
            return false;
        }
        let [a, b, c, d] = self.header(file);
        match self {
            Layout::Cod11 => d == self.blocks() as i32,
            _ => {
                a == self.version()
                    && (b as u32) < MAX_GLOBAL as u32
                    && (c as u32) < MAX_FAVORITES as u32
                    && d == self.blocks() as i32
            }
        }
    }

    /// Whether `file` is 1.1's cache where this layout's belongs: vcod
    /// wrote 1.1's shape under every game's name before it knew the
    /// others. 1.5 and CoD2 share the name; UO's is its own.
    fn migrates(self, file: &[u8]) -> bool {
        matches!(self, Layout::Cod15 | Layout::Cod2) && Layout::Cod11.valid(file)
    }

    /// An empty cache, as the game writes it with no servers.
    fn fresh(self) -> Vec<u8> {
        let mut f = vec![0; self.file_len()];
        if self != Layout::Cod11 {
            f[0..4].copy_from_slice(&self.version().to_le_bytes());
        }
        f[12..16].copy_from_slice(&(self.blocks() as i32).to_le_bytes());
        f
    }
}

/// A cached server. Entries vcod cannot address (IPX, loopback) keep their
/// bytes and are written back unchanged.
#[derive(Clone, PartialEq, Eq)]
pub struct Entry {
    layout: Layout,
    bytes: Vec<u8>,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Entry({:?}, {:?})", self.addr(), self.hostname())
    }
}

/// 1.1's `serverInfo_t` fields an `infoResponse` fills (CoDMP.exe
/// `LAN_GetServerInfo` 0x417a10).
const MAPNAME_11: usize = 0x34;
const GAMETYPE_11: usize = 0x78;
const CLIENTS_11: usize = 0x98;
const MAX_CLIENTS_11: usize = 0x9c;
const PING_11: usize = 0xa8;
const PASSWORD_11: usize = 0xb4;

impl Entry {
    /// What `LAN_AddServer` fills in: the 20-byte `netadr_t` (type, IP,
    /// big-endian port at [`Layout::port_at`]), the name cut to 31 bytes,
    /// the visible flag.
    pub fn new(layout: Layout, addr: SocketAddrV4, name: &str) -> Entry {
        let mut e = Entry {
            layout,
            bytes: vec![0; layout.entry()],
        };
        e.bytes[0..4].copy_from_slice(&NA_IP.to_le_bytes());
        e.bytes[4..8].copy_from_slice(&addr.ip().octets());
        let port = layout.port_at();
        e.bytes[port..port + 2].copy_from_slice(&addr.port().to_be_bytes());
        e.set_str(layout.hostname(), name);
        match layout {
            Layout::Cod11 => e.set_i32(layout.visible(), 1),
            _ => e.bytes[layout.visible()] = 1,
        }
        e
    }

    pub fn addr(&self) -> Option<SocketAddrV4> {
        if self.i32(0) != NA_IP {
            return None;
        }
        let (b, port) = (&self.bytes, self.layout.port_at());
        let port = u16::from_be_bytes([b[port], b[port + 1]]);
        Some(SocketAddrV4::new(
            Ipv4Addr::new(b[4], b[5], b[6], b[7]),
            port,
        ))
    }

    pub fn hostname(&self) -> String {
        self.str(self.layout.hostname())
    }

    /// The `serverInfo_t` as the file holds it, [`Layout::entry`] bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// A record of `layout` made elsewhere (1.7's typed row).
    pub fn from_bytes(layout: Layout, bytes: Vec<u8>) -> Entry {
        assert_eq!(bytes.len(), layout.entry(), "{layout:?} entry");
        Entry { layout, bytes }
    }

    /// What an `infoResponse` writes over a 1.1 cached server
    /// (`CL_SetServerInfo`): the row's fields and the ping. The later
    /// layouts keep the entry as read or added.
    pub fn set_info(&mut self, info: &super::master::ServerInfo, ping: i32) {
        if self.layout != Layout::Cod11 {
            return;
        }
        self.set_str(self.layout.hostname(), &info.hostname);
        self.set_str(MAPNAME_11, &info.mapname);
        self.set_str(GAMETYPE_11, &info.gametype);
        self.set_i32(CLIENTS_11, info.clients as i32);
        self.set_i32(MAX_CLIENTS_11, info.max_clients as i32);
        self.set_i32(PASSWORD_11, info.password as i32);
        self.set_i32(PING_11, ping);
    }

    fn i32(&self, at: usize) -> i32 {
        i32::from_le_bytes(self.bytes[at..at + 4].try_into().unwrap())
    }

    fn set_i32(&mut self, at: usize, v: i32) {
        self.bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn str(&self, at: usize) -> String {
        let field = &self.bytes[at..at + STRING];
        let end = field.iter().position(|&b| b == 0).unwrap_or(STRING);
        crate::text::latin1(&field[..end])
    }

    /// `Q_strncpyz` into a 32-byte field: at most 31 bytes and a NUL.
    fn set_str(&mut self, at: usize, s: &str) {
        let field = &mut self.bytes[at..at + STRING];
        field.fill(0);
        for (d, c) in field[..STRING - 1].iter_mut().zip(s.chars()) {
            *d = c as u32 as u8;
        }
    }
}

/// The favourites in a cache file; `None` when the file is not a cache of
/// `layout`, which the game would drop too.
pub fn read_favorites(layout: Layout, file: &[u8]) -> Option<Vec<Entry>> {
    if layout.migrates(file) {
        let old = read_favorites(Layout::Cod11, file)?;
        let moved = old
            .iter()
            .filter_map(|e| Some(Entry::new(layout, e.addr()?, &e.hostname())));
        return Some(moved.collect());
    }
    if !layout.valid(file) {
        return None;
    }
    let (at, base) = (layout.favorite_count_at(), layout.favorites_at());
    Some(entries(layout, file, at, base, MAX_FAVORITES))
}

/// The Internet list in a cache file, as [`read_favorites`] reads the
/// favourites; a 1.1-shaped file under a later game's name gives none.
pub fn read_internet(layout: Layout, file: &[u8]) -> Option<Vec<Entry>> {
    if layout.migrates(file) {
        return Some(Vec::new());
    }
    if !layout.valid(file) {
        return None;
    }
    let cap = match layout {
        Layout::Cod11 => MAX_GLOBAL_11,
        _ => MAX_GLOBAL,
    };
    Some(entries(
        layout,
        file,
        layout.internet_count_at(),
        HEADER,
        cap,
    ))
}

/// The records of the block at `base` whose count int sits at `count_at`,
/// at most `cap`.
fn entries(layout: Layout, file: &[u8], count_at: usize, base: usize, cap: usize) -> Vec<Entry> {
    let at = count_at;
    let n = i32::from_le_bytes(file[at..at + 4].try_into().unwrap()).max(0) as usize;
    let size = layout.entry();
    (0..n.min(cap))
        .map(|i| Entry {
            layout,
            bytes: file[base + i * size..base + (i + 1) * size].to_vec(),
        })
        .collect()
}

/// `old` with its favourites replaced by `favorites`, at most
/// [`Layout::max_favorites`]; an empty cache with them when there is no
/// file (`None`) or `old` is vcod's old 1.1-shaped write in its place, its
/// favourites read through [`read_favorites`]. `None` back when `old` is
/// not a cache of `layout`: that file is left alone.
pub fn write_favorites(layout: Layout, old: Option<&[u8]>, favorites: &[Entry]) -> Option<Vec<u8>> {
    let mut out = match old {
        Some(f) if layout.valid(f) => f.to_vec(),
        Some(f) if layout.migrates(f) => layout.fresh(),
        Some(_) => return None,
        None => layout.fresh(),
    };
    let favorites: Vec<&Entry> = (favorites.iter())
        .filter(|e| e.layout == layout)
        .take(layout.max_favorites())
        .collect();
    let at = layout.favorite_count_at();
    out[at..at + 4].copy_from_slice(&(favorites.len() as i32).to_le_bytes());
    let (base, size) = (layout.favorites_at(), layout.entry());
    out[base..base + MAX_FAVORITES * size].fill(0);
    for (i, e) in favorites.iter().enumerate() {
        out[base + i * size..base + (i + 1) * size].copy_from_slice(&e.bytes);
    }
    debug_assert!(read_favorites(layout, &out).is_some_and(|r| r.len() == favorites.len()));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Layout; 5] = [
        Layout::Cod11,
        Layout::Cod15,
        Layout::Uo,
        Layout::Cod2,
        Layout::Cod4,
    ];

    #[test]
    fn file_sizes_match_retail() {
        // An untouched retail 1.1 install's servercache.dat is 412688 bytes,
        // a retail UO's uoservercache.dat 3059472.
        assert_eq!(Layout::Cod11.file_len(), 412688);
        assert_eq!(Layout::Cod11.blocks(), 0x64c00);
        // The size ints the 1.5, UO and CoD2 loaders compare (CoDMP.exe
        // 0x41907c, CoDUOMP.exe 0x41a55b, CoD2MP_s.exe 0x4b568b).
        assert_eq!(Layout::Cod15.blocks(), 0x2d7480);
        assert_eq!(Layout::Uo.blocks(), 0x2eaf00);
        assert_eq!(Layout::Cod2.blocks(), 0x2d7480);
        assert_eq!(Layout::Uo.file_len(), 3059472);
        // Each block's read size (1.5 0x419087, 0x4190a5; UO 0x41a566,
        // 0x41a584).
        assert_eq!(MAX_GLOBAL * Layout::Cod15.entry(), 0x2d2a80);
        assert_eq!(MAX_FAVORITES * Layout::Cod15.entry(), 0x4a00);
        assert_eq!(MAX_GLOBAL * Layout::Uo.entry(), 0x2e6300);
        assert_eq!(MAX_FAVORITES * Layout::Uo.entry(), 0x4c00);
        // 1.7's blocks' size word (cod4-front-end.md).
        assert_eq!(Layout::Cod4.blocks(), 0x2fe980);
        assert_eq!(Layout::Cod4.file_len(), 3_139_984);
    }

    /// Every layout writes favourites it reads back, keeping the Internet
    /// list and every other byte.
    #[test]
    fn favourites_round_trip_and_keep_the_rest() {
        for layout in ALL {
            let mut old = write_favorites(layout, None, &[]).unwrap();
            assert_eq!(old.len(), layout.file_len());
            assert_eq!(read_favorites(layout, &old), Some(Vec::new()));
            // An Internet count and one of its entries' bytes.
            let count_at = if layout == Layout::Cod11 { 0 } else { 4 };
            old[count_at] = 7;
            old[HEADER + 3] = 0xaa;
            let a: SocketAddrV4 = "10.0.0.5:28961".parse().unwrap();
            let file =
                write_favorites(layout, Some(&old), &[Entry::new(layout, a, "My server")]).unwrap();
            assert_eq!(file.len(), layout.file_len());
            assert_eq!((file[count_at], file[HEADER + 3]), (7, 0xaa), "{layout:?}");
            assert_eq!(&file[..count_at], &old[..count_at]);
            let favs = read_favorites(layout, &file).unwrap();
            assert_eq!(favs.len(), 1);
            assert_eq!(read_internet(layout, &file).map(|i| i.len()), Some(7));
            assert_eq!(favs[0].addr(), Some(a));
            assert_eq!(favs[0].hostname(), "My server");
            // The address sits as a netadr_t: type 4, IP bytes, port
            // big-endian; the name at the layout's hostname.
            let at = layout.favorites_at();
            assert_eq!(&file[at..at + 8], &[4, 0, 0, 0, 10, 0, 0, 5]);
            let port = layout.port_at();
            assert_eq!(&file[at + port..at + port + 2], &28961u16.to_be_bytes());
            assert_eq!(&file[at + layout.hostname()..][..9], b"My server");
            // Removing it leaves the rest as it was.
            let none = write_favorites(layout, Some(&file), &[]).unwrap();
            assert_eq!(none, old, "{layout:?}");
        }
    }

    /// The later games' header: version 1 (1.7's 3), then the counts, then
    /// the size; a 128th favourite would make the file one they drop, so it
    /// stays out.
    #[test]
    fn the_later_layouts_keep_a_file_their_loader_takes() {
        let a: SocketAddrV4 = "10.0.0.5:1".parse().unwrap();
        for layout in [Layout::Cod15, Layout::Uo, Layout::Cod2, Layout::Cod4] {
            let favs = vec![Entry::new(layout, a, "x"); 0x80];
            let file = write_favorites(layout, None, &favs).unwrap();
            let version = if layout == Layout::Cod4 { 3 } else { 1 };
            assert_eq!(&file[0..4], &i32::to_le_bytes(version));
            assert_eq!(&file[8..12], &0x7fi32.to_le_bytes());
            assert_eq!(read_favorites(layout, &file).unwrap().len(), 0x7f);
            let mut bad = file.clone();
            bad[8..12].copy_from_slice(&0x80i32.to_le_bytes());
            assert_eq!(read_favorites(layout, &bad), None);
        }
        let file = write_favorites(
            Layout::Cod11,
            None,
            &vec![Entry::new(Layout::Cod11, a, "x"); 0x80],
        );
        assert_eq!(
            read_favorites(Layout::Cod11, &file.unwrap()).unwrap().len(),
            0x80
        );
    }

    /// A file of another layout is neither read nor written: 1.1's on a
    /// 1.5 install, UO's for 1.5, a short or foreign one.
    #[test]
    fn another_layouts_file_is_left_alone() {
        for layout in ALL {
            for other in ALL {
                // 1.5's and CoD2's files are one shape; 1.1's migrates
                // into theirs.
                let same = other == layout || other.file_len() == layout.file_len();
                if same
                    || (other == Layout::Cod11 && matches!(layout, Layout::Cod15 | Layout::Cod2))
                {
                    continue;
                }
                let f = write_favorites(other, None, &[]).unwrap();
                assert_eq!(
                    read_favorites(layout, &f),
                    None,
                    "{layout:?} reads {other:?}"
                );
                assert_eq!(write_favorites(layout, Some(&f), &[]), None);
            }
            assert_eq!(read_favorites(layout, &[0; 64]), None);
            assert_eq!(write_favorites(layout, Some(&[0; 64]), &[]), None);
            assert_eq!(read_favorites(layout, &vec![1; layout.file_len()]), None);
            // One byte long: never truncated to the layout's size.
            let mut long = layout.fresh();
            long.push(0);
            assert_eq!(write_favorites(layout, Some(&long), &[]), None);
        }
    }

    /// Where an install's executables sit: the dir itself, and, for a
    /// tree of linked paks (the UO test base), the directory the links
    /// lead back to.
    fn install_roots(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut roots = vec![dir.to_path_buf()];
        for sub in ["main", "uo"] {
            let pak = std::fs::read_dir(dir.join(sub))
                .into_iter()
                .flatten()
                .flatten()
                .find(|e| e.path().extension().is_some_and(|x| x == "pk3"));
            if let Some(root) = pak
                .and_then(|e| std::fs::canonicalize(e.path()).ok())
                .and_then(|p| Some(p.parent()?.parent()?.to_path_buf()))
                && !roots.contains(&root)
            {
                roots.push(root);
            }
        }
        roots
    }

    /// Each install's own cache file, when it has one of its layout: a copy
    /// reads, and writing its favourites back gives the same bytes.
    #[test]
    fn the_installs_cache_files_read() {
        let tmp = tempfile::tempdir().unwrap();
        let mut seen = 0;
        for (var, game, layouts) in [
            (
                "COD_DIR",
                "cod1",
                &[Layout::Cod11, Layout::Cod15, Layout::Uo][..],
            ),
            ("COD2_DIR", "cod2", &[Layout::Cod2][..]),
        ] {
            let Some(dir) = crate::testing::install_root(var, game, "main") else {
                continue;
            };
            for root in install_roots(&dir) {
                for &layout in layouts {
                    let path = root.join(layout.file_name());
                    let copy = tmp.path().join(format!("{seen}-{}", layout.file_name()));
                    if std::fs::copy(&path, &copy).is_err() {
                        continue;
                    }
                    seen += 1;
                    let bytes = std::fs::read(&copy).unwrap();
                    // 1.5's and CoD2's name can hold vcod's old 1.1-shaped
                    // write: it reads, and the write-back is the game's own.
                    if layout.migrates(&bytes) {
                        let favs = read_favorites(layout, &bytes).unwrap();
                        let new = write_favorites(layout, Some(&bytes), &favs).unwrap();
                        assert_eq!(read_favorites(layout, &new).unwrap(), favs);
                        continue;
                    }
                    if bytes.len() != layout.file_len() {
                        continue;
                    }
                    let favs = read_favorites(layout, &bytes)
                        .unwrap_or_else(|| panic!("{} is not a {layout:?} cache", path.display()));
                    let back = write_favorites(layout, Some(&bytes), &favs).unwrap();
                    assert!(back == bytes, "{} changed on a write-back", path.display());
                }
            }
        }
    }

    /// vcod's old 1.1-shaped write under 1.5's or CoD2's name gives its
    /// favourites to the new file, which then has the game's own shape.
    #[test]
    fn an_old_11_shaped_file_moves_into_the_games_own() {
        let a: SocketAddrV4 = "10.0.0.5:28961".parse().unwrap();
        let old =
            write_favorites(Layout::Cod11, None, &[Entry::new(Layout::Cod11, a, "Home")]).unwrap();
        for layout in [Layout::Cod15, Layout::Cod2] {
            let favs = read_favorites(layout, &old).unwrap();
            assert_eq!(
                (favs[0].addr(), favs[0].hostname().as_str()),
                (Some(a), "Home")
            );
            let file = write_favorites(layout, Some(&old), &favs).unwrap();
            assert_eq!(file.len(), layout.file_len());
            assert_eq!(read_favorites(layout, &file).unwrap(), favs);
        }
        assert_eq!(read_favorites(Layout::Uo, &old), None);
        assert_eq!(write_favorites(Layout::Uo, Some(&old), &[]), None);
    }

    #[test]
    fn names_are_cut_to_31_bytes() {
        for layout in ALL {
            let e = Entry::new(layout, "203.0.113.4:1".parse().unwrap(), &"x".repeat(40));
            assert_eq!(e.hostname().len(), 31);
        }
    }
}
