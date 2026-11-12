use anyhow::{Result, bail};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// One open archive file read at a cursor of its own: clones share the
/// handle and read with positioned reads, so no clone moves another's.
#[derive(Clone)]
struct SharedFile {
    file: std::sync::Arc<File>,
    pos: u64,
}

impl SharedFile {
    fn open(path: &Path) -> std::io::Result<Self> {
        Ok(SharedFile {
            file: std::sync::Arc::new(File::open(path)?),
            pos: 0,
        })
    }
}

impl Read for SharedFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::read_at(&*self.file, buf, self.pos)?;
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_read(&*self.file, buf, self.pos)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl std::io::Seek for SharedFile {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        let pos = match to {
            std::io::SeekFrom::Start(p) => Some(p),
            std::io::SeekFrom::End(d) => self.file.metadata()?.len().checked_add_signed(d),
            std::io::SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = pos.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before the start")
        })?;
        Ok(self.pos)
    }
}

/// Largest entry `Pk3Fs::read` will inflate. The biggest stock asset is a
/// lightmapped BSP under 32 MiB; anything past this is a corrupt or hostile pak.
pub const MAX_ENTRY_BYTES: u64 = 64 << 20;

/// Case-insensitive virtual filesystem over the *.pk3 (CoD2: *.iwd) archives of one mod
/// directory. Later archives override earlier ones, as the game layers paks.
pub struct Pk3Fs {
    /// The directories searched, lowest priority first.
    dirs: Vec<PathBuf>,
    archives: Vec<PathBuf>,
    /// Parallel to `archives`.
    paks: Vec<PakInfo>,
    /// `pack->referenced`, set by a read (see `referenced_paks`).
    referenced: Vec<AtomicBool>,
    /// [`Self::set_surfs_unflagged`]: an `xmodelsurfs/` read flags nothing.
    surfs_unflagged: AtomicBool,
    // lowercased entry path -> (archive index, exact entry name in that zip)
    index: HashMap<String, (usize, String)>,
    // Same with '@' normalized to '_'; BSP material names sometimes spell the
    // '@' surface-type separator as '_'. Consulted when the exact lookup misses.
    alias_index: HashMap<String, (usize, String)>,
    // Kept open after first read; re-parsing a central directory per entry
    // dominates when loading ~3000 anims. Reads take &self, hence the lock.
    // The lock only hands out a clone (shared directory, own cursor), so
    // reads on several threads inflate side by side.
    open: Mutex<HashMap<usize, zip::ZipArchive<SharedFile>>>,
    // lowercased path -> bytes `read` returns ahead of every archive.
    overlay: HashMap<String, Vec<u8>>,
    /// `--vanilla`: only [`is_vanilla_pak`]s are on the search path.
    vanilla: bool,
    kind: ArchiveKind,
    assets: crate::asset_source::AssetCache,
}

/// One pak on the search path, as the pure-server lists name it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PakInfo {
    pub path: PathBuf,
    /// The directory it sits in (`main`, a mod's), the `<game>/` of
    /// `sv_referencedPakNames`.
    pub game: String,
    /// The file name without `.pk3` or `.iwd`.
    pub name: String,
    /// `localized_*`: never in `sv_paks`, searched after every other pak.
    pub localized: bool,
    /// `FS_LoadZipFile`'s checksum (`crate::pak_checksum`).
    pub checksum: i32,
    /// The entry CRCs it came from, which the keyed (pure) checksum reuses.
    pub crcs: Vec<u32>,
}

impl PakInfo {
    /// `<game>/<name>`, as `sv_referencedPakNames` spells it.
    pub fn qualified_name(&self) -> String {
        format!("{}/{}", self.game, self.name)
    }

    /// The checksum keyed with a map load's `checksumFeed`.
    pub fn pure_checksum(&self, checksum_feed: i32) -> i32 {
        crate::pak_checksum::checksums_of(&self.crcs, checksum_feed).pure
    }
}

/// A lookup key: case folded, a backslash read as a slash, as
/// `FS_HashFileName` (coduo_lnxded 0x806077a) and `FS_FilenameCompare`
/// (0x806161e) read one. UO's `fx/gmi_impacts.csv` names its explodes
/// `fx\weapon\explosions\...`. A leading slash or backslash is dropped, as
/// `FS_FOpenFileRead` skips it (CoDMP.exe 0x00429a70, CoDUOMP.exe 0x0042d760,
/// coduo_lnxded 0x08061a74): UO's `fx/map_trenches/mortar_mz.efx` chains
/// `/fx/map_trenches/mortar_mz_nest`.
fn path_key(path: &str) -> String {
    path.trim_start_matches(['/', '\\'])
        .to_lowercase()
        .replace('\\', "/")
}

/// `FS_AddGameDirectory`'s sort: names compare case-blind with `mp_` read
/// as `zz` and `localized_` as a space (docs/research/cod11-server-handshake.md,
/// "Pak checksums").
fn retail_sort_key(file: &str) -> String {
    let lower = file.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("mp_") {
        format!("zz{rest}")
    } else if let Some(rest) = lower.strip_prefix("localized_") {
        format!(" {rest}")
    } else {
        lower
    }
}

/// CoD2's `iwdsort` (cod2mp Mac 1.3 0x34412) as a key: names compare
/// upper-cased with `\` and `:` read as `/`, and of two `localized_*` ones an
/// English one sorts first (docs/research/cod2-iwi-iwd.md, "The search
/// path"). Localized and plain archives never meet here: [`search_archives`]
/// splits them.
fn iwd_sort_key(file: &str) -> String {
    let upper = file.to_ascii_uppercase().replace(['\\', ':'], "/");
    let foreign = upper.strip_prefix("LOCALIZED_").is_some_and(|rest| {
        let lang: String = rest.chars().take_while(char::is_ascii_alphabetic).collect();
        lang != "ENGLISH"
    });
    format!("{}{upper}", u8::from(foreign))
}

/// The archives a search path is built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveKind {
    /// CoD1's `*.pk3`.
    Pk3,
    /// CoD2's `*.iwd`, plain zips under another name and sort.
    Iwd,
}

impl ArchiveKind {
    pub fn ext(self) -> &'static str {
        match self {
            ArchiveKind::Pk3 => "pk3",
            ArchiveKind::Iwd => "iwd",
        }
    }

    fn sort_key(self, file: &str) -> String {
        match self {
            ArchiveKind::Pk3 => retail_sort_key(file),
            ArchiveKind::Iwd => iwd_sort_key(file),
        }
    }
}

/// The `.pk3`s directly in `dir`, highest priority first.
fn pk3s_in(dir: &Path) -> Result<Vec<PathBuf>> {
    archives_in(dir, ArchiveKind::Pk3)
}

/// `kind`'s archives directly in `dir`, highest priority first: the engine
/// sorts them ascending and pushes each on the head of the search path.
fn archives_in(dir: &Path, kind: ArchiveKind) -> Result<Vec<PathBuf>> {
    let mut archives: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(kind.ext()))
        })
        .collect();
    archives.sort_by_cached_key(|p| {
        std::cmp::Reverse(kind.sort_key(&p.file_name().unwrap_or_default().to_string_lossy()))
    });
    Ok(archives)
}

/// File `name` off the search path `FS_FOpenFileRead` walks over `dirs`,
/// highest priority first: each directory's archives (`localized_*`
/// aside), then its loose file; then every directory's `localized_*`
/// archives. CoD4 1.7's `path` lists a home path's iwds, its directory,
/// then the base path's (docs/research/cod4-handshake.md 14.1).
pub fn read_search(dirs: &[PathBuf], kind: ArchiveKind, name: &str) -> Option<Vec<u8>> {
    let key = path_key(name);
    let member = |pak: &Path| -> Option<Vec<u8>> {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(pak).ok()?).ok()?;
        let entry = zip.file_names().find(|n| path_key(n) == key)?.to_string();
        let mut f = zip.by_name(&entry).ok()?;
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut f, &mut out).ok()?;
        Some(out)
    };
    let localized = |p: &PathBuf| {
        p.file_name().is_some_and(|n| {
            n.to_string_lossy()
                .to_ascii_lowercase()
                .starts_with("localized_")
        })
    };
    for dir in dirs {
        let paks = archives_in(dir, kind).unwrap_or_default();
        if let Some(data) = paks
            .iter()
            .filter(|p| !localized(p))
            .find_map(|p| member(p))
        {
            return Some(data);
        }
        if let Ok(data) = std::fs::read(dir.join(name)) {
            return Some(data);
        }
    }
    dirs.iter()
        .flat_map(|d| archives_in(d, kind).unwrap_or_default())
        .filter(|p| localized(p))
        .find_map(|p| member(&p))
}

/// The readable paks of `dirs` (lowest priority first, the base then a
/// mod) in retail's search order, highest priority first: every directory's
/// paks, the later directory's ahead, then every directory's `localized_*`
/// paks the same way. A directory that does not exist adds nothing; an
/// unreadable pak is skipped.
pub fn search_paks(dirs: &[&Path]) -> Vec<PakInfo> {
    search_archives(dirs, ArchiveKind::Pk3)
}

/// [`search_paks`] over `kind`'s archives. CoD2 orders its directories and
/// its `localized_*` iwds the same way (`FS_AddGameDirectory`, cod2mp Mac
/// 1.3 0x34ffe).
pub fn search_archives(dirs: &[&Path], kind: ArchiveKind) -> Vec<PakInfo> {
    let mut main = Vec::new();
    let mut localized = Vec::new();
    for dir in dirs.iter().rev() {
        let game = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        for path in archives_in(dir, kind).unwrap_or_default() {
            let crcs = match std::fs::File::open(&path)
                .map_err(anyhow::Error::from)
                .and_then(|f| crate::pak_checksum::entry_crcs(std::io::BufReader::new(f)))
            {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("skipping unreadable {} {}: {e}", kind.ext(), path.display());
                    continue;
                }
            };
            let name = path
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let info = PakInfo {
                localized: name.to_ascii_lowercase().starts_with("localized_"),
                checksum: crate::pak_checksum::checksums_of(&crcs, 0).checksum,
                crcs,
                game: game.clone(),
                name,
                path,
            };
            if info.localized {
                localized.push(info);
            } else {
                main.push(info);
            }
        }
    }
    main.extend(localized);
    main
}

/// What `--vanilla` keeps of an install whose paks make it `proto`'s
/// (`net::protocol::install_protocol_of`): the stock paks `FS_idPak` names
/// (`net::download::is_stock_pak`, `localized_*` ones included) and, on
/// United Offensive, `uo/uomappack00`, which a stock 1.51b server lists in
/// `sv_pakNames` (crates/server/tests/fixtures/configstrings/mp_cassino-dm-uo-pure.txt).
pub fn is_vanilla_pak(pak: &PakInfo, t: &crate::net::wire::Transport) -> bool {
    let name = pak.qualified_name();
    let uo = t.version == crate::net::protocol::PROTOCOL_V22.transport.version;
    crate::net::download::is_stock_pak(&name, t)
        || (uo && name.eq_ignore_ascii_case("uo/uomappack00"))
}

/// A stock iwd by its qualified name (`main/iw_07`): `FS_iwIwd(name,
/// "main")`, the same test in CoD2 1.3 (lnxded 0x8065324) and CoD4 1.7
/// (0x81286c2), `iw_00` .. `iw_24` or `localized_*` holding `_iw00` ..
/// `_iw24` (`vcod_cod2::net::download::fs_iw_iwd`).
pub fn is_stock_iwd(qualified: &str) -> bool {
    let name = qualified.to_ascii_lowercase().replace('\\', "/");
    let Some(base) = name.strip_prefix("main/") else {
        return false;
    };
    if (0..25).any(|n| base == format!("iw_{n:02}")) {
        return true;
    }
    base.strip_prefix("localized_")
        .is_some_and(|rest| (0..25).any(|n| rest.contains(&format!("_iw{n:02}"))))
}

/// `base` and what sits under it, lowest priority first: a base other than
/// `main` sits over its sibling `main/`, the way United Offensive searches
/// `uo/` then `main/` (docs/research/coduo-diff.md, section 1).
pub fn base_dirs(base: &Path) -> Vec<PathBuf> {
    let main = base
        .file_name()
        .filter(|n| !n.eq_ignore_ascii_case("main"))
        .and_then(|_| base.parent())
        .map(|p| p.join("main"))
        .filter(|m| m.is_dir());
    main.into_iter().chain([base.to_path_buf()]).collect()
}

/// One entry of the Mods menu: a directory beside the base game and its
/// `description.txt`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModEntry {
    pub dir: String,
    /// Empty when the mod has no description; the menu shows `dir` then.
    pub description: String,
}

/// `FS_GetModList` (CoDMP.exe 0x43b030): the directories of `game_dir` other
/// than `main`, `base` and hidden ones that hold at least one `.pk3`, each with
/// the first 48 bytes of its `description.txt`. Sorted by name, as a Windows
/// directory listing comes back.
pub fn mod_list(game_dir: &Path, base: &str) -> Vec<ModEntry> {
    let Ok(rd) = std::fs::read_dir(game_dir) else {
        return Vec::new();
    };
    let mut mods: Vec<ModEntry> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|d| {
            !d.starts_with('.') && !d.eq_ignore_ascii_case("main") && !d.eq_ignore_ascii_case(base)
        })
        .filter(|d| pk3s_in(&game_dir.join(d)).is_ok_and(|p| !p.is_empty()))
        .map(|dir| {
            let description = std::fs::read(game_dir.join(&dir).join("description.txt"))
                .map(|b| {
                    let b = &b[..b.len().min(48)];
                    String::from_utf8_lossy(b).trim_end().to_string()
                })
                .unwrap_or_default();
            ModEntry { dir, description }
        })
        .collect();
    mods.sort_by_key(|m| m.dir.to_ascii_lowercase());
    mods
}

impl Pk3Fs {
    pub fn open(mod_dir: &Path) -> Result<Self> {
        Self::open_layered(mod_dir, None)
    }

    /// `base`'s paks, then `game`'s on top of them: the search path a
    /// systeminfo `fs_game` gives (docs/research/cod11-front-end.md, section
    /// 16). A `game` directory that does not exist yet adds nothing.
    pub fn open_layered(base: &Path, game: Option<&Path>) -> Result<Self> {
        Self::open_search(base, game, None, false)
    }

    /// [`open_layered`](Self::open_layered) on a pure server: with `pure`
    /// set (the systeminfo's `sv_paks`), a pak whose checksum is not in it
    /// is left off the search path, as `FS_PakIsPure` hides it. `localized_*`
    /// paks stay, since `sv_paks` never lists them. Every one of
    /// [`base_dirs`]`(base)` is searched. With `vanilla` `game` is left off
    /// and so is every pak but an [`is_vanilla_pak`].
    pub fn open_search(
        base: &Path,
        game: Option<&Path>,
        pure: Option<&[i32]>,
        vanilla: bool,
    ) -> Result<Self> {
        if !base.is_dir() {
            bail!("cannot read mod dir {}", base.display());
        }
        let mut dirs = base_dirs(base);
        if let Some(g) = game.filter(|g| g.is_dir()) {
            if vanilla {
                log::info!("vanilla: not searching {}", g.display());
            } else {
                dirs.push(g.to_path_buf());
            }
        }
        Self::open_dirs(dirs, pure, vanilla)
    }

    /// The paks of `dirs`, lowest priority first, as [`search_paks`] orders
    /// them; `pure` and `vanilla` as in [`open_search`](Self::open_search).
    pub fn open_dirs(dirs: Vec<PathBuf>, pure: Option<&[i32]>, vanilla: bool) -> Result<Self> {
        Self::open_archives(dirs, pure, vanilla, ArchiveKind::Pk3)
    }

    /// A CoD2 search path: `base`'s `.iwd`s, then `game`'s on top of them.
    /// No pure list and no `--vanilla` yet; those are CoD1's.
    pub fn open_iwd(base: &Path, game: Option<&Path>) -> Result<Self> {
        Self::open_iwd_dirs(base, game, false)
    }

    /// [`Self::open_iwd`] over `base` alone with its [`is_stock_iwd`]s
    /// only: tests that assert the stock assets read it, so a third-party
    /// iwd in `main/` cannot move them.
    pub fn open_iwd_stock(base: &Path) -> Result<Self> {
        Self::open_iwd_dirs(base, None, true)
    }

    fn open_iwd_dirs(base: &Path, game: Option<&Path>, stock: bool) -> Result<Self> {
        if !base.is_dir() {
            bail!("cannot read game dir {}", base.display());
        }
        // the wavelet .iwi tables live only in the install's render DLL
        if let Some(root) = base.parent() {
            crate::iwi::load_wavelet_tables(root);
        }
        let mut dirs = vec![base.to_path_buf()];
        dirs.extend(game.filter(|g| g.is_dir()).map(Path::to_path_buf));
        Self::open_archives(dirs, None, stock, ArchiveKind::Iwd)
    }

    /// [`Self::open_iwd`] with any number of directories over `base`, each
    /// over the last (CoD4's `usermaps/<map>/` then `fs_game`).
    pub fn open_iwd_layers(base: &Path, over: &[PathBuf]) -> Result<Self> {
        if !base.is_dir() {
            bail!("cannot read game dir {}", base.display());
        }
        if let Some(root) = base.parent() {
            crate::iwi::load_wavelet_tables(root);
        }
        let mut dirs = vec![base.to_path_buf()];
        dirs.extend(over.iter().filter(|g| g.is_dir()).cloned());
        Self::open_archives(dirs, None, false, ArchiveKind::Iwd)
    }

    fn open_archives(
        dirs: Vec<PathBuf>,
        pure: Option<&[i32]>,
        vanilla: bool,
        kind: ArchiveKind,
    ) -> Result<Self> {
        let base = dirs.first().cloned().unwrap_or_default();
        let dir_refs: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
        let mut paks = search_archives(&dir_refs, kind);
        if vanilla {
            let proto = crate::net::protocol::install_protocol_of(&paks);
            paks.retain(|p| {
                let keep = match kind {
                    ArchiveKind::Pk3 => is_vanilla_pak(p, &proto.transport),
                    ArchiveKind::Iwd => is_stock_iwd(&p.qualified_name()),
                };
                if !keep {
                    log::info!("vanilla: skipping {}", p.path.display());
                }
                keep
            });
        }
        if let Some(pure) = pure {
            paks.retain(|p| {
                let keep = p.localized || pure.contains(&p.checksum);
                if !keep {
                    log::info!("{}: not on the pure server's list", p.path.display());
                }
                keep
            });
        }
        // Override order: the lowest priority first.
        paks.reverse();
        if paks.is_empty() {
            bail!("no .{} archives found in {}", kind.ext(), base.display());
        }
        let mut index = HashMap::new();
        let mut alias_index = HashMap::new();
        // A corrupt pak (a half-finished download is the usual one) is
        // skipped so the rest still loads; archive indices count the readable ones.
        let mut readable = Vec::with_capacity(paks.len());
        for pak in paks {
            let zip = std::fs::File::open(&pak.path)
                .map_err(anyhow::Error::from)
                .and_then(|f| Ok(zip::ZipArchive::new(f)?));
            let zip = match zip {
                Ok(z) => z,
                Err(e) => {
                    log::warn!(
                        "skipping unreadable {} {}: {e}",
                        kind.ext(),
                        pak.path.display()
                    );
                    continue;
                }
            };
            let i = readable.len();
            for name in zip.file_names() {
                let key = path_key(name);
                if key.contains('@') {
                    alias_index.insert(key.replace('@', "_"), (i, name.to_string()));
                }
                index.insert(key, (i, name.to_string()));
            }
            readable.push(pak);
        }
        if readable.is_empty() {
            bail!("no readable .{} archives in {}", kind.ext(), base.display());
        }
        let referenced = readable.iter().map(|_| AtomicBool::new(false)).collect();
        Ok(Self {
            dirs,
            archives: readable.iter().map(|p| p.path.clone()).collect(),
            paks: readable,
            referenced,
            surfs_unflagged: AtomicBool::new(false),
            index,
            alias_index,
            open: Mutex::new(HashMap::new()),
            overlay: HashMap::new(),
            vanilla,
            kind,
            assets: Default::default(),
        })
    }

    /// Whether the search path was opened `--vanilla`.
    pub fn vanilla(&self) -> bool {
        self.vanilla
    }

    /// The game whose archives these are: CoD2's for iwds, else CoD1's.
    pub fn game(&self) -> crate::game::Game {
        match self.kind {
            ArchiveKind::Pk3 => crate::game::Game::Cod1,
            ArchiveKind::Iwd => crate::game::Game::Cod2,
        }
    }

    /// What `Pk3Fs::load` loaded from this search path.
    pub(crate) fn asset_cache(&self) -> &crate::asset_source::AssetCache {
        &self.assets
    }

    /// The directories searched, lowest priority first.
    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }

    /// The search path's paks, highest priority first.
    pub fn paks(&self) -> impl Iterator<Item = &PakInfo> {
        self.paks.iter().rev()
    }

    /// The paks a file was read from since the search path opened (retail
    /// clears the flags at each `FS_Restart`), skipping the file types
    /// `FS_FOpenFileRead` leaves unflagged; highest priority first.
    pub fn referenced_paks(&self) -> Vec<&PakInfo> {
        self.paks
            .iter()
            .zip(&self.referenced)
            .rev()
            .filter(|(_, r)| r.load(Ordering::Relaxed))
            .map(|(p, _)| p)
            .collect()
    }

    /// The search path's paks, highest priority first, each with whether a
    /// file was read from it (`pack->referenced`).
    pub fn paks_read(&self) -> impl Iterator<Item = (&PakInfo, bool)> {
        self.paks
            .iter()
            .zip(&self.referenced)
            .rev()
            .map(|(p, r)| (p, r.load(Ordering::Relaxed)))
    }

    /// `FS_ClearPakReferences(0)`: what `SV_SpawnServer` runs before the
    /// map loads, so the referenced lists name what this level read.
    pub fn clear_references(&self) {
        for r in &self.referenced {
            r.store(false, Ordering::Relaxed);
        }
        // A cached asset would skip the reads that mark its paks again.
        self.assets.clear();
    }

    /// What [`Self::read`] of `path` marks, after a [`Self::read_unflagged`]
    /// of it: an overlaid path marks nothing, any other its pak.
    pub fn mark_read(&self, path: &str) {
        if !self.overlay.contains_key(&path_key(path)) {
            self.touch(path);
        }
    }

    /// Marks `path`'s pak referenced as a read would, without reading it.
    pub fn touch(&self, path: &str) {
        let key = path_key(path);
        if let Some((ai, _)) = self.index.get(&key).or_else(|| self.alias_index.get(&key))
            && self.flags(&key)
        {
            self.referenced[*ai].store(true, Ordering::Relaxed);
        }
    }

    /// The `cp` command `CL_SendPureChecksums` sends after a map load, keyed
    /// with the gamestate's `checksumFeed` (`crate::pak_checksum::pure_command`),
    /// opening with the paks of `dlls`, the protocol's client game and UI
    /// modules (`Protocol::pure_dlls`). The general list is every referenced
    /// pak but the `localized_*` ones.
    pub fn pure_command(&self, checksum_feed: i32, dlls: [&str; 2]) -> String {
        let dll = |name: &str| {
            self.source_pak(name)
                .map(|p| p.pure_checksum(checksum_feed))
        };
        let general: Vec<i32> = self
            .referenced_paks()
            .iter()
            .filter(|p| !p.localized)
            .map(|p| p.pure_checksum(checksum_feed))
            .collect();
        crate::pak_checksum::pure_command(dll(dlls[0]), dll(dlls[1]), &general, checksum_feed)
    }

    /// The pak `path` comes from, by the same lookup as `read`.
    pub fn source_pak(&self, path: &str) -> Option<&PakInfo> {
        let key = path_key(path);
        let (ai, _) = self
            .index
            .get(&key)
            .or_else(|| self.alias_index.get(&key))?;
        Some(&self.paks[*ai])
    }

    /// A filesystem with no archives; every lookup misses. For tests that
    /// need the type but not the game.
    pub fn empty() -> Self {
        Self {
            dirs: Vec::new(),
            archives: Vec::new(),
            paks: Vec::new(),
            referenced: Vec::new(),
            surfs_unflagged: AtomicBool::new(false),
            index: HashMap::new(),
            alias_index: HashMap::new(),
            open: Mutex::new(HashMap::new()),
            overlay: HashMap::new(),
            vanilla: false,
            kind: ArchiveKind::Pk3,
            assets: Default::default(),
        }
    }

    /// Serves `bytes` for `path` ahead of every archive, the way a later pak
    /// overrides an earlier one. For tests that run a patched copy of an asset.
    pub fn overlay(&mut self, path: &str, bytes: Vec<u8>) {
        self.overlay.insert(path_key(path), bytes);
        self.assets.clear();
    }

    /// Same lookup as `read`, without touching the archive.
    pub fn contains(&self, path: &str) -> bool {
        let key = path_key(path);
        self.overlay.contains_key(&key)
            || self.index.contains_key(&key)
            || self.alias_index.contains_key(&key)
    }

    /// The archive `read(path)` would hit.
    pub fn source_archive(&self, path: &str) -> Option<&Path> {
        let key = path_key(path);
        let (ai, _) = self
            .index
            .get(&key)
            .or_else(|| self.alias_index.get(&key))?;
        Some(&self.archives[*ai])
    }

    /// `read` with `MAX_ENTRY_BYTES` as the size limit.
    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.read_limited(path, MAX_ENTRY_BYTES)
    }

    /// `None` when the entry is missing, unreadable, or inflates past
    /// `limit` bytes. The zip central directory's size field only sizes the
    /// buffer; the stream itself is bounded, since the field can lie.
    pub fn read_limited(&self, path: &str, limit: u64) -> Option<Vec<u8>> {
        self.read_flagging(path, limit, true)
    }

    /// [`Self::read`] that marks no pak referenced: a read ahead of need
    /// (a load's parallel prefetch), whose user calls [`Self::mark_read`]
    /// on the path if and when it takes the bytes.
    pub fn read_unflagged(&self, path: &str) -> Option<Vec<u8>> {
        self.read_flagging(path, MAX_ENTRY_BYTES, false)
    }

    fn read_flagging(&self, path: &str, limit: u64, flag: bool) -> Option<Vec<u8>> {
        let key = path_key(path);
        if let Some(bytes) = self.overlay.get(&key) {
            return (bytes.len() as u64 <= limit).then(|| bytes.clone());
        }
        let (ai, entry) = self
            .index
            .get(&key)
            .or_else(|| self.alias_index.get(&key))?;
        if flag && self.flags(&key) {
            self.referenced[*ai].store(true, Ordering::Relaxed);
        }
        // `fs_debug 1`'s line, for diffing what a load reads against retail.
        log::trace!("read {path} from {}", self.paks[*ai].qualified_name());
        let mut zip = self.archive(*ai)?;
        let f = zip.by_name(entry).ok()?;
        if f.size() > limit {
            log::warn!("{path}: declared size {} exceeds {limit} bytes", f.size());
            return None;
        }
        let mut buf = Vec::with_capacity(f.size() as usize);
        // One byte past the limit is read on purpose: it tells "exactly
        // limit" from "more than limit".
        f.take(limit + 1).read_to_end(&mut buf).ok()?;
        if buf.len() as u64 > limit {
            log::warn!("{path}: entry inflates past {limit} bytes");
            return None;
        }
        Some(buf)
    }

    /// Archive `ai` off the open cache, its central directory parsed once
    /// per `Pk3Fs`. A poisoned lock still holds a usable cache; `by_name`
    /// re-seeks from the central directory.
    fn archive(&self, ai: usize) -> Option<zip::ZipArchive<SharedFile>> {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        match open.entry(ai) {
            Entry::Occupied(e) => Some(e.get().clone()),
            Entry::Vacant(e) => {
                let file = SharedFile::open(&self.archives[ai]).ok()?;
                Some(e.insert(zip::ZipArchive::new(file).ok()?).clone())
            }
        }
    }

    /// Lowercased entry paths ending in `suffix`, e.g. ".shader".
    pub fn names_with_suffix(&self, suffix: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .index
            .keys()
            .filter(|k| k.ends_with(suffix))
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// Lowercased entry paths starting with `prefix`, e.g. "xanim/",
    /// overlaid ones included.
    pub fn list_prefix(&self, prefix: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .index
            .keys()
            .chain(self.overlay.keys())
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// `FS_GetFileList(dir, "")` over the paks: the files directly under
    /// `dir/`, highest-priority pak first and each pak in its own
    /// central-directory order, a name already listed (case-insensitively)
    /// skipped. No sort; names as the pak spells them, without `dir/`
    /// (docs/research/coduo-vehicles.md 10.1).
    pub fn list_dir_in_search_order(&self, dir: &str) -> Vec<String> {
        let prefix = format!("{}/", dir.to_lowercase());
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        // Off the open cache: reparsing every pak's central directory cost
        // UO's `map_restart` 100 ms (cod11-map-cycle.md 4.9).
        for ai in (0..self.archives.len()).rev() {
            let Some(zip) = self.archive(ai) else {
                continue;
            };
            for i in 0..zip.len() {
                let Some(name) = zip.name_for_index(i) else {
                    continue;
                };
                let Some(rest) = name
                    .get(..prefix.len())
                    .filter(|p| p.eq_ignore_ascii_case(&prefix))
                    .map(|_| &name[prefix.len()..])
                else {
                    continue;
                };
                if rest.is_empty() || rest.contains('/') {
                    continue;
                }
                if seen.insert(rest.to_lowercase()) {
                    out.push(rest.to_string());
                }
            }
        }
        out
    }

    /// `FS_GetFileList(dir, ext)` (`FS_ListFilteredFiles`, CoDMP.exe
    /// 0x42b2a0): [`Self::list_dir_in_search_order`] keeping the names that
    /// end in `ext` (case-insensitively), the overlay's (sorted) first.
    /// Full paths, the pak's spelling.
    pub fn file_list(&self, dir: &str, ext: &str) -> Vec<String> {
        let dir = dir.trim_end_matches('/');
        let prefix = format!("{}/", dir.to_lowercase());
        let ends = |name: &str| {
            name.len() >= ext.len()
                && name.is_char_boundary(name.len() - ext.len())
                && name[name.len() - ext.len()..].eq_ignore_ascii_case(ext)
        };
        let mut overlaid: Vec<&String> = self
            .overlay
            .keys()
            .filter(|k| k.strip_prefix(&prefix).is_some_and(|r| !r.contains('/')))
            .collect();
        overlaid.sort();
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        let names = overlaid
            .into_iter()
            .map(|k| k[prefix.len()..].to_string())
            .chain(self.list_dir_in_search_order(dir));
        for name in names {
            if ends(&name) && seen.insert(name.to_lowercase()) {
                out.push(format!("{dir}/{name}"));
            }
        }
        out
    }

    /// A map file's extension: CoD1's `.bsp`, CoD2's `.d3dbsp`.
    fn map_ext(&self) -> &'static str {
        match self.kind {
            ArchiveKind::Pk3 => ".bsp",
            ArchiveKind::Iwd => ".d3dbsp",
        }
    }

    pub fn find_maps(&self) -> Vec<String> {
        let ext = self.map_ext();
        let mut maps: Vec<String> = self
            .index
            .keys()
            .filter(|k| k.ends_with(ext))
            .filter(|k| {
                let dir = &k[..k.rfind('/').unwrap_or(0)];
                dir == "maps" || dir == "maps/mp"
            })
            .map(|k| k[k.rfind('/').unwrap() + 1..k.len() - ext.len()].to_string())
            .collect();
        maps.sort();
        maps.dedup();
        maps
    }

    pub fn resolve_map(&self, name: &str) -> Option<String> {
        let (name, ext) = (name.to_lowercase(), self.map_ext());
        for candidate in [format!("maps/{name}{ext}"), format!("maps/mp/{name}{ext}")] {
            if let Some((_, entry)) = self.index.get(&candidate) {
                return Some(entry.clone());
            }
        }
        None
    }
}

impl Pk3Fs {
    /// A CoD2 dedicated server's reads: its xmodel surface loader (cod2_lnxded
    /// 0x80c7436) is a stub that reads nothing, so the meshes vcod loads for
    /// collision and bones leave their iwds out of `sv_referencedIwds`
    /// (cod2-protocol.md 11.3).
    pub fn set_surfs_unflagged(&self, on: bool) {
        self.surfs_unflagged.store(on, Ordering::Relaxed);
    }

    fn flags(&self, key: &str) -> bool {
        flags_reference(key)
            && !(key.starts_with("xmodelsurfs/") && self.surfs_unflagged.load(Ordering::Relaxed))
    }
}

/// Whether a read of `path` marks its pak referenced: `FS_FOpenFileRead`
/// (CoDMP.exe 0x429e3e) skips these types and anything under `levelshots`.
fn flags_reference(path: &str) -> bool {
    const SKIP: [&str; 7] = [
        ".shader", ".txt", ".cfg", ".config", ".bot", ".arena", ".menu",
    ];
    !SKIP.iter().any(|e| path.ends_with(e)) && !path.contains("levelshots")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_pk3(dir: &std::path::Path, file: &str, entries: &[(&str, &str)]) {
        let f = std::fs::File::create(dir.join(file)).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, content) in entries {
            z.start_file(*name, opts).unwrap();
            z.write_all(content.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    /// CoD4 1.7's `exec` off the disk (cod4-handshake.md 14.1): a home
    /// path iwd beats a loose file beside it, the home directory beats the
    /// base path's iwds, and the localized iwds come last.
    #[test]
    fn read_search_takes_each_dirs_iwds_then_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home/main");
        let base = dir.path().join("base/main");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&base).unwrap();
        make_pk3(
            &home,
            "zz_vtest.iwd",
            &[("vt_both.cfg", "iwd"), ("VT_IWD.cfg", "iwd")],
        );
        std::fs::write(home.join("vt_both.cfg"), "loose").unwrap();
        std::fs::write(home.join("vt_loose.cfg"), "loose").unwrap();
        make_pk3(
            &base,
            "iw_00.iwd",
            &[("vt_loose.cfg", "base"), ("b.cfg", "base iwd")],
        );
        make_pk3(
            &base,
            "localized_english_iw00.iwd",
            &[("b.cfg", "en"), ("c.cfg", "en")],
        );
        std::fs::write(base.join("c.cfg"), "base loose").unwrap();
        let dirs = [home, base];
        let read = |p: &str| {
            read_search(&dirs, ArchiveKind::Iwd, p).map(|b| String::from_utf8(b).unwrap())
        };
        assert_eq!(read("vt_both.cfg").as_deref(), Some("iwd"));
        assert_eq!(read("vt_iwd.cfg").as_deref(), Some("iwd"));
        assert_eq!(read("vt_loose.cfg").as_deref(), Some("loose"));
        assert_eq!(read("b.cfg").as_deref(), Some("base iwd"));
        assert_eq!(read("c.cfg").as_deref(), Some("base loose"));
        assert_eq!(read("nothere.cfg"), None);
    }

    #[test]
    fn iwd_search_order() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let game = dir.path().join("mod");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&game).unwrap();
        make_pk3(&main, "iw_00.iwd", &[("a.txt", "00"), ("b.txt", "00")]);
        make_pk3(&main, "iw_15.iwd", &[("a.txt", "15")]);
        make_pk3(&main, "IW_06.IWD", &[("a.txt", "06"), ("e.txt", "06")]);
        make_pk3(
            &main,
            "localized_english_iw00.iwd",
            &[("b.txt", "en"), ("c.txt", "en")],
        );
        make_pk3(&main, "localized_french_iw00.iwd", &[("c.txt", "fr")]);
        make_pk3(&main, "pak0.pk3", &[("d.txt", "pk3")]);
        make_pk3(&game, "z.iwd", &[("e.txt", "mod")]);
        let fs = Pk3Fs::open_iwd(&main, Some(&game)).unwrap();
        let read = |p: &str| fs.read(p).map(|b| String::from_utf8(b).unwrap());
        assert_eq!(read("a.txt").as_deref(), Some("15"));
        // localized iwds sit under every plain one
        assert_eq!(read("b.txt").as_deref(), Some("00"));
        // an English one sorts first, so any other language overrides it
        assert_eq!(read("c.txt").as_deref(), Some("fr"));
        assert_eq!(read("d.txt"), None);
        assert_eq!(read("e.txt").as_deref(), Some("mod"));
        let names: Vec<&str> = fs.paks().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "z",
                "iw_15",
                "IW_06",
                "iw_00",
                "localized_french_iw00",
                "localized_english_iw00"
            ]
        );
    }

    /// A map pack's iwd, sorted after every stock one, overrides them on
    /// the full search path and is left off the stock one.
    #[test]
    fn the_stock_iwd_search_leaves_a_map_pack_off() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        std::fs::create_dir_all(&main).unwrap();
        make_pk3(&main, "iw_00.iwd", &[("maps/mp/gametypes/dm.gsc", "stock")]);
        make_pk3(&main, "localized_english_iw11.iwd", &[("l.txt", "en")]);
        make_pk3(
            &main,
            "mp_pack.iwd",
            &[("maps/mp/gametypes/dm.gsc", "pack")],
        );
        make_pk3(&main, "~~~.iwd", &[("maps/mp/toujane_fx.gsc", "pack")]);
        let read = |fs: &Pk3Fs, p: &str| fs.read(p).map(|b| String::from_utf8(b).unwrap());
        let all = Pk3Fs::open_iwd(&main, None).unwrap();
        assert_eq!(
            read(&all, "maps/mp/gametypes/dm.gsc").as_deref(),
            Some("pack")
        );
        let stock = Pk3Fs::open_iwd_stock(&main).unwrap();
        assert_eq!(
            read(&stock, "maps/mp/gametypes/dm.gsc").as_deref(),
            Some("stock")
        );
        assert_eq!(read(&stock, "maps/mp/toujane_fx.gsc"), None);
        let names: Vec<&str> = stock.paks().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["iw_00", "localized_english_iw11"]);
        assert!(is_stock_iwd("MAIN\\IW_24"));
        assert!(!is_stock_iwd("main/iw_25"));
        assert!(!is_stock_iwd("mod/iw_00"));
    }

    #[test]
    fn iwd_maps_are_d3dbsp() {
        let dir = tempfile::tempdir().unwrap();
        make_pk3(
            dir.path(),
            "iw_04.iwd",
            &[
                ("maps/mp/mp_toujane.d3dbsp", "data"),
                ("maps/mp/old.bsp", "x"),
            ],
        );
        let fs = Pk3Fs::open_iwd(dir.path(), None).unwrap();
        assert_eq!(fs.find_maps(), ["mp_toujane"]);
        assert_eq!(
            fs.resolve_map("MP_Toujane").as_deref(),
            Some("maps/mp/mp_toujane.d3dbsp")
        );
        assert_eq!(fs.resolve_map("old"), None);
    }

    #[test]
    fn case_insensitive_lookup() {
        let dir = tempfile::tempdir().unwrap();
        make_pk3(dir.path(), "pak0.pk3", &[("maps/MP/mp_test.bsp", "data")]);
        let fs = Pk3Fs::open(dir.path()).unwrap();
        assert_eq!(fs.read("maps/mp/MP_TEST.bsp").unwrap(), b"data");
        assert!(fs.read("maps/mp/missing.bsp").is_none());
    }

    /// `FS_GetFileList`'s order: the newest pak first, each in its zip
    /// order, a name a newer pak listed skipped, the extension's case and
    /// a subdirectory ignored.
    #[test]
    fn a_file_list_runs_in_search_order() {
        let dir = tempfile::tempdir().unwrap();
        let gt = |f: &str| format!("maps/mp/gametypes/{f}");
        let (sd, dm, tdm, bel) = (gt("sd.gsc"), gt("dm.gsc"), gt("tdm.GSC"), gt("bel.gsc"));
        let (txt, sub) = (gt("dm.txt"), gt("x/ctf.gsc"));
        make_pk3(dir.path(), "pak5.pk3", &[(&sd, ""), (&dm, ""), (&txt, "")]);
        make_pk3(
            dir.path(),
            "zzz_mod.pk3",
            &[
                (&tdm, ""),
                (&sub, ""),
                ("maps/MP/gametypes/SD.gsc", ""),
                (&bel, ""),
            ],
        );
        let fs = Pk3Fs::open(dir.path()).unwrap();
        let got = fs.file_list("maps/mp/gametypes", ".gsc");
        let want = ["tdm.GSC", "SD.gsc", "bel.gsc", "dm.gsc"].map(gt);
        assert_eq!(got, want);
    }

    /// UO's impact table names its explodes with backslashes, which the
    /// engine's lookup reads as slashes; a leading slash is skipped.
    #[test]
    fn a_backslash_reads_as_a_slash() {
        let dir = tempfile::tempdir().unwrap();
        make_pk3(
            dir.path(),
            "pakuo00.pk3",
            &[("fx/weapon/explosions/tank_dirt.efx", "data")],
        );
        let fs = Pk3Fs::open(dir.path()).unwrap();
        let path = r"fx\weapon\explosions\tank_dirt.efx";
        assert_eq!(fs.read(path).unwrap(), b"data");
        assert!(fs.contains(path));
        assert_eq!(
            fs.read("/fx/weapon/explosions/tank_dirt.efx").unwrap(),
            b"data"
        );
    }

    /// A UO `cp` opens with the paks of UO's own client modules
    /// (coduo-diff.md 23), which sit beside 1.5's in a UO install.
    #[test]
    fn the_cp_opens_with_the_protocols_client_module_paks() {
        use crate::net::protocol::{PROTOCOL_V6, PROTOCOL_V22};
        let root = tempfile::tempdir().unwrap();
        let [main, uo] = ["main", "uo"].map(|d| root.path().join(d));
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&uo).unwrap();
        let dlls = [("cgame_mp_x86.dll", "c"), ("ui_mp_x86.dll", "u")];
        make_pk3(&main, "pakb.pk3", &dlls);
        let uo_dlls = [("uo_cgame_mp_x86.dll", "C"), ("uo_ui_mp_x86.dll", "U")];
        make_pk3(&uo, "pakuo07.pk3", &uo_dlls);
        let fs = Pk3Fs::open(&uo).unwrap();
        let opens = |p: &crate::net::protocol::Protocol, pak: &str| {
            let sum = fs.paks().find(|i| i.name == pak).unwrap().pure_checksum(5);
            fs.pure_command(5, p.pure_dlls)
                .starts_with(&format!("cp {sum} {sum} @"))
        };
        assert!(opens(&PROTOCOL_V22, "pakuo07"));
        assert!(opens(&PROTOCOL_V6, "pakb"));
    }

    /// `--vanilla` keeps the install protocol's stock paks and drops a
    /// third-party map, a `zzz_*` mod, a download and the `fs_game` dir.
    #[test]
    fn vanilla_searches_only_the_stock_paks() {
        let root = tempfile::tempdir().unwrap();
        let [main, game] = ["main", "mymod"].map(|d| root.path().join(d));
        for d in [&main, &game] {
            std::fs::create_dir_all(d).unwrap();
        }
        make_pk3(&main, "pak0.pk3", &[("a.txt", "stock"), ("b.txt", "stock")]);
        make_pk3(&main, "localized_english_pak0.pk3", &[("c.txt", "stock")]);
        make_pk3(&main, "zzz_mod.pk3", &[("a.txt", "zzz")]);
        make_pk3(&main, "mp_custom.pk3", &[("maps/mp/mp_custom.bsp", "x")]);
        // 1.5's pak9 is not 1.1's: this install's pak0 is 1.1's.
        make_pk3(&main, "pak9.pk3", &[("b.txt", "pak9")]);
        make_pk3(&game, "mod.pk3", &[("c.txt", "mod")]);

        let all = Pk3Fs::open_search(&main, Some(&game), None, false).unwrap();
        assert_eq!(all.read("a.txt").unwrap(), b"zzz");
        assert!(!all.vanilla());

        let fs = Pk3Fs::open_search(&main, Some(&game), None, true).unwrap();
        assert!(fs.vanilla());
        assert_eq!(fs.read("a.txt").unwrap(), b"stock");
        assert_eq!(fs.read("b.txt").unwrap(), b"stock");
        assert_eq!(fs.read("c.txt").unwrap(), b"stock");
        assert!(!fs.contains("maps/mp/mp_custom.bsp"));
        let names: Vec<String> = fs.paks().map(PakInfo::qualified_name).collect();
        assert_eq!(names, ["main/pak0", "main/localized_english_pak0"]);
    }

    /// On a UO install `--vanilla` keeps `uo/`'s stock paks and the map
    /// pack, and 1.5's hex-numbered paks under them.
    #[test]
    fn vanilla_keeps_uos_paks_and_its_map_pack() {
        let root = tempfile::tempdir().unwrap();
        let [main, uo] = ["main", "uo"].map(|d| root.path().join(d));
        for d in [&main, &uo] {
            std::fs::create_dir_all(d).unwrap();
        }
        make_pk3(&main, "pakb.pk3", &[("a.txt", "main")]);
        make_pk3(&main, "zzz_main.pk3", &[("a.txt", "zzz")]);
        make_pk3(&uo, "pakuo00.pk3", &[("b.txt", "uo")]);
        make_pk3(&uo, "localized_english_pakuo00.pk3", &[("c.txt", "uo")]);
        make_pk3(&uo, "uomappack00.pk3", &[("d.txt", "uo")]);
        make_pk3(&uo, "nsf_mp_j5outpost.pk3", &[("d.txt", "custom")]);
        let fs = Pk3Fs::open_search(&uo, None, None, true).unwrap();
        let names: Vec<String> = fs.paks().map(PakInfo::qualified_name).collect();
        assert_eq!(
            names,
            [
                "uo/uomappack00",
                "uo/pakuo00",
                "main/pakb",
                "uo/localized_english_pakuo00"
            ]
        );
        assert_eq!(fs.read("a.txt").unwrap(), b"main");
        assert_eq!(fs.read("d.txt").unwrap(), b"uo");
    }

    /// `--mod-dir uo`: United Offensive's paks over the 1.5 `main/` ones,
    /// a mod over both.
    #[test]
    fn a_base_other_than_main_sits_over_main() {
        let root = tempfile::tempdir().unwrap();
        let [main, uo, game] = ["main", "uo", "mymod"].map(|d| root.path().join(d));
        for d in [&main, &uo, &game] {
            std::fs::create_dir_all(d).unwrap();
        }
        make_pk3(
            &main,
            "pak0.pk3",
            &[("a.txt", "main"), ("b.txt", "main"), ("c.txt", "main")],
        );
        make_pk3(&uo, "pakuo00.pk3", &[("a.txt", "uo"), ("b.txt", "uo")]);
        make_pk3(&game, "mod.pk3", &[("a.txt", "mod")]);
        let fs = Pk3Fs::open_layered(&uo, Some(&game)).unwrap();
        assert_eq!(fs.read("a.txt").unwrap(), b"mod");
        assert_eq!(fs.read("b.txt").unwrap(), b"uo");
        assert_eq!(fs.read("c.txt").unwrap(), b"main");
        let names: Vec<String> = fs.paks().map(PakInfo::qualified_name).collect();
        assert_eq!(names, ["mymod/mod", "uo/pakuo00", "main/pak0"]);
    }

    #[test]
    fn a_game_dir_overrides_the_base_whatever_the_pak_names() {
        let root = tempfile::tempdir().unwrap();
        let (main, game) = (root.path().join("main"), root.path().join("mymod"));
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&game).unwrap();
        make_pk3(
            &main,
            "zzz_late.pk3",
            &[("a.txt", "main"), ("b.txt", "main")],
        );
        make_pk3(&game, "aaa_early.pk3", &[("a.txt", "mod")]);
        let fs = Pk3Fs::open_layered(&main, Some(&game)).unwrap();
        assert_eq!(fs.read("a.txt").unwrap(), b"mod");
        assert_eq!(fs.read("b.txt").unwrap(), b"main");
        let missing = root.path().join("nothere");
        let fs = Pk3Fs::open_layered(&main, Some(&missing)).unwrap();
        assert_eq!(fs.read("a.txt").unwrap(), b"main");
    }

    #[test]
    fn underscore_matches_at_sign_in_entry_names() {
        let dir = tempfile::tempdir().unwrap();
        make_pk3(
            dir.path(),
            "pak0.pk3",
            &[("textures/x/snow@1024fill.dds", "tex")],
        );
        let fs = Pk3Fs::open(dir.path()).unwrap();
        assert_eq!(fs.read("textures/x/snow_1024fill.dds").unwrap(), b"tex");
        assert_eq!(fs.read("textures/x/snow@1024fill.dds").unwrap(), b"tex");
        assert!(fs.contains("textures/x/SNOW@1024fill.dds"));
        assert!(fs.contains("textures/x/snow_1024fill.dds"));
        assert!(!fs.contains("textures/x/missing.dds"));
    }

    #[test]
    fn exact_entry_beats_alias() {
        let dir = tempfile::tempdir().unwrap();
        make_pk3(
            dir.path(),
            "pak0.pk3",
            &[("a/b@c.txt", "aliased"), ("a/b_c.txt", "exact")],
        );
        let fs = Pk3Fs::open(dir.path()).unwrap();
        assert_eq!(fs.read("a/b_c.txt").unwrap(), b"exact");
    }

    #[test]
    fn later_pk3_overrides_earlier() {
        let dir = tempfile::tempdir().unwrap();
        make_pk3(dir.path(), "pak0.pk3", &[("a.txt", "old")]);
        make_pk3(dir.path(), "pak1.pk3", &[("A.TXT", "new")]);
        let fs = Pk3Fs::open(dir.path()).unwrap();
        assert_eq!(fs.read("a.txt").unwrap(), b"new");
    }

    #[test]
    fn finds_and_resolves_maps() {
        let dir = tempfile::tempdir().unwrap();
        make_pk3(
            dir.path(),
            "pak0.pk3",
            &[
                ("maps/MP/mp_test.bsp", "x"),
                ("maps/training.bsp", "x"),
                ("maps/MP/readme.txt", "x"),
            ],
        );
        let fs = Pk3Fs::open(dir.path()).unwrap();
        assert_eq!(
            fs.find_maps(),
            vec!["mp_test".to_string(), "training".to_string()]
        );
        assert_eq!(fs.resolve_map("MP_TEST").unwrap(), "maps/MP/mp_test.bsp");
        assert_eq!(fs.resolve_map("training").unwrap(), "maps/training.bsp");
        assert!(fs.resolve_map("nope").is_none());
    }

    /// Overwrites the uncompressed-size field of the single central
    /// directory entry so `size()` lies about the payload.
    fn patch_central_size(path: &std::path::Path, size: u32) {
        let mut d = std::fs::read(path).unwrap();
        let cd = d
            .windows(4)
            .position(|w| w == b"PK\x01\x02")
            .expect("central directory");
        d[cd + 24..cd + 28].copy_from_slice(&size.to_le_bytes());
        std::fs::write(path, d).unwrap();
    }

    #[test]
    fn read_refuses_entries_over_the_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let body = "x".repeat(100);
        make_pk3(dir.path(), "pak0.pk3", &[("big.txt", &body)]);
        let fs = Pk3Fs::open(dir.path()).unwrap();
        assert_eq!(fs.read_limited("big.txt", 100).unwrap().len(), 100);
        // declared size beyond the limit: refused before allocating
        assert!(fs.read_limited("big.txt", 99).is_none());
        assert!(fs.read("big.txt").is_some());
    }

    #[test]
    fn read_stops_when_the_stream_outgrows_its_declared_size() {
        let dir = tempfile::tempdir().unwrap();
        let body = "x".repeat(100);
        make_pk3(dir.path(), "pak0.pk3", &[("lie.txt", &body)]);
        // central directory claims 10 bytes; the stream holds 100
        patch_central_size(&dir.path().join("pak0.pk3"), 10);
        let fs = Pk3Fs::open(dir.path()).unwrap();
        assert!(fs.read_limited("lie.txt", 50).is_none());
        assert_eq!(fs.read_limited("lie.txt", 100).unwrap().len(), 100);
    }

    #[test]
    fn huge_declared_size_does_not_preallocate() {
        let dir = tempfile::tempdir().unwrap();
        make_pk3(dir.path(), "pak0.pk3", &[("lie.txt", "tiny")]);
        patch_central_size(&dir.path().join("pak0.pk3"), u32::MAX - 1);
        let fs = Pk3Fs::open(dir.path()).unwrap();
        assert!(fs.read("lie.txt").is_none());
    }

    #[test]
    fn corrupt_archive_is_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        make_pk3(dir.path(), "pak0.pk3", &[("a.txt", "ok")]);
        std::fs::write(dir.path().join("pak1.pk3"), b"not a zip at all").unwrap();
        make_pk3(dir.path(), "pak2.pk3", &[("b.txt", "also ok")]);
        let fs = Pk3Fs::open(dir.path()).unwrap();
        assert_eq!(fs.read("a.txt").unwrap(), b"ok");
        assert_eq!(fs.read("b.txt").unwrap(), b"also ok");
        assert_eq!(
            fs.source_archive("b.txt").unwrap(),
            dir.path().join("pak2.pk3")
        );
    }

    #[test]
    fn only_corrupt_archives_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pak0.pk3"), b"garbage").unwrap();
        assert!(Pk3Fs::open(dir.path()).is_err());
        assert!(Pk3Fs::open(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn search_order_is_retails() {
        let root = tempfile::tempdir().unwrap();
        let (main, game) = (root.path().join("main"), root.path().join("mymod"));
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&game).unwrap();
        for f in [
            "pak0.pk3",
            "pak6.pk3",
            "bonneville.pk3",
            "mp_x.pk3",
            "localized_english_pak0.pk3",
            "localized_english_pak1.pk3",
        ] {
            make_pk3(&main, f, &[("a.txt", f)]);
        }
        make_pk3(&game, "mod.pk3", &[("a.txt", "mod.pk3")]);
        make_pk3(&game, "localized_mod.pk3", &[("a.txt", "x")]);
        let names: Vec<String> = search_paks(&[&main, &game])
            .iter()
            .map(PakInfo::qualified_name)
            .collect();
        // The order of the retail capture's `sv_referencedPakNames`, with
        // `mp_` sorting as `zz`.
        assert_eq!(
            names,
            [
                "mymod/mod",
                "main/mp_x",
                "main/pak6",
                "main/pak0",
                "main/bonneville",
                "mymod/localized_mod",
                "main/localized_english_pak1",
                "main/localized_english_pak0",
            ]
        );
        let fs = Pk3Fs::open_layered(&main, Some(&game)).unwrap();
        assert_eq!(fs.read("a.txt").unwrap(), b"mod.pk3");
        let fs = Pk3Fs::open(&main).unwrap();
        assert_eq!(fs.read("a.txt").unwrap(), b"mp_x.pk3");
    }

    #[test]
    fn a_pure_list_hides_other_paks_but_not_localized_ones() {
        let dir = tempfile::tempdir().unwrap();
        make_pk3(dir.path(), "pak0.pk3", &[("a.txt", "pak0")]);
        make_pk3(dir.path(), "zzz.pk3", &[("a.txt", "zzz")]);
        make_pk3(dir.path(), "localized_x.pk3", &[("b.txt", "loc")]);
        let paks = search_paks(&[dir.path()]);
        let pak0 = paks.iter().find(|p| p.name == "pak0").unwrap().checksum;
        let fs = Pk3Fs::open_search(dir.path(), None, Some(&[pak0]), false).unwrap();
        assert_eq!(fs.read("a.txt").unwrap(), b"pak0");
        assert_eq!(fs.read("b.txt").unwrap(), b"loc");
    }

    #[test]
    fn reads_mark_their_pak_referenced() {
        let dir = tempfile::tempdir().unwrap();
        make_pk3(dir.path(), "pak0.pk3", &[("a.bsp", "x"), ("b.menu", "y")]);
        make_pk3(dir.path(), "pak1.pk3", &[("c.shader", "z")]);
        let fs = Pk3Fs::open(dir.path()).unwrap();
        fs.read("c.shader").unwrap();
        fs.read("b.menu").unwrap();
        assert!(
            fs.referenced_paks().is_empty(),
            "shaders and menus do not count"
        );
        fs.read("a.bsp").unwrap();
        let refs: Vec<&str> = fs
            .referenced_paks()
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(refs, ["pak0"]);
        fs.touch("c.shader");
        assert_eq!(fs.referenced_paks().len(), 1);
    }

    #[test]
    fn mods_are_directories_with_paks() {
        let root = tempfile::tempdir().unwrap();
        for d in ["main", "uo", "zmod", "Amod", "empty", ".hidden"] {
            std::fs::create_dir_all(root.path().join(d)).unwrap();
        }
        for d in ["main", "uo", "zmod", "Amod", ".hidden"] {
            make_pk3(&root.path().join(d), "x.pk3", &[("a.txt", "a")]);
        }
        std::fs::write(
            root.path().join("zmod/description.txt"),
            "Zombie mod, a description longer than forty-eight bytes\n",
        )
        .unwrap();
        let mods = mod_list(root.path(), "uo");
        assert_eq!(
            mods,
            [
                ModEntry {
                    dir: "Amod".into(),
                    description: String::new()
                },
                ModEntry {
                    dir: "zmod".into(),
                    description: "Zombie mod, a description longer than forty-eigh".into(),
                },
            ]
        );
    }

    #[test]
    fn real_game_dir_smoke() {
        let dir = crate::testing::game_dir().join("main");
        if !dir.is_dir() {
            return;
        }
        let fs = Pk3Fs::open(&dir).unwrap();
        assert!(fs.resolve_map("mp_pavlov").is_some());
        assert!(fs.find_maps().contains(&"mp_pavlov".to_string()));
    }
}
