use anyhow::{Result, bail};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Largest entry `Pk3Fs::read` will inflate. The biggest stock asset is a
/// lightmapped BSP under 32 MiB; anything past this is a corrupt or hostile pak.
pub const MAX_ENTRY_BYTES: u64 = 64 << 20;

/// Case-insensitive virtual filesystem over the *.pk3 archives of one mod
/// directory. Later archives override earlier ones, as the game layers paks.
pub struct Pk3Fs {
    archives: Vec<PathBuf>,
    /// Parallel to `archives`.
    paks: Vec<PakInfo>,
    /// `pack->referenced`, set by a read (see `referenced_paks`).
    referenced: Vec<AtomicBool>,
    // lowercased entry path -> (archive index, exact entry name in that zip)
    index: HashMap<String, (usize, String)>,
    // Same with '@' normalized to '_'; BSP material names sometimes spell the
    // '@' surface-type separator as '_'. Consulted when the exact lookup misses.
    alias_index: HashMap<String, (usize, String)>,
    // Kept open after first read; re-parsing a central directory per entry
    // dominates when loading ~3000 anims. Reads take &self, hence the lock.
    open: Mutex<HashMap<usize, zip::ZipArchive<File>>>,
    // lowercased path -> bytes `read` returns ahead of every archive.
    overlay: HashMap<String, Vec<u8>>,
}

/// One pak on the search path, as the pure-server lists name it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PakInfo {
    pub path: PathBuf,
    /// The directory it sits in (`main`, a mod's), the `<game>/` of
    /// `sv_referencedPakNames`.
    pub game: String,
    /// The file name without `.pk3`.
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

/// The `.pk3`s directly in `dir`, highest priority first.
fn pk3s_in(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut archives: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pk3")))
        .collect();
    archives.sort_by_cached_key(|p| {
        std::cmp::Reverse(retail_sort_key(
            &p.file_name().unwrap_or_default().to_string_lossy(),
        ))
    });
    Ok(archives)
}

/// The readable paks of `dirs` (lowest priority first, the base then a
/// mod) in retail's search order, highest priority first: every directory's
/// paks, the later directory's ahead, then every directory's `localized_*`
/// paks the same way. A directory that does not exist adds nothing; an
/// unreadable pak is skipped.
pub fn search_paks(dirs: &[&Path]) -> Vec<PakInfo> {
    let mut main = Vec::new();
    let mut localized = Vec::new();
    for dir in dirs.iter().rev() {
        let game = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        for path in pk3s_in(dir).unwrap_or_default() {
            let crcs = match std::fs::File::open(&path)
                .map_err(anyhow::Error::from)
                .and_then(|f| crate::pak_checksum::entry_crcs(std::io::BufReader::new(f)))
            {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("skipping unreadable pk3 {}: {e}", path.display());
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
        Self::open_search(base, game, None)
    }

    /// [`open_layered`](Self::open_layered) on a pure server: with `pure`
    /// set (the systeminfo's `sv_paks`), a pak whose checksum is not in it
    /// is left off the search path, as `FS_PakIsPure` hides it. `localized_*`
    /// paks stay, since `sv_paks` never lists them.
    pub fn open_search(base: &Path, game: Option<&Path>, pure: Option<&[i32]>) -> Result<Self> {
        if !base.is_dir() {
            bail!("cannot read mod dir {}", base.display());
        }
        let mut dirs = vec![base];
        dirs.extend(game.filter(|g| g.is_dir()));
        let mut paks = search_paks(&dirs);
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
            bail!("no .pk3 archives found in {}", base.display());
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
                    log::warn!("skipping unreadable pk3 {}: {e}", pak.path.display());
                    continue;
                }
            };
            let i = readable.len();
            for name in zip.file_names() {
                let key = name.to_lowercase();
                if key.contains('@') {
                    alias_index.insert(key.replace('@', "_"), (i, name.to_string()));
                }
                index.insert(key, (i, name.to_string()));
            }
            readable.push(pak);
        }
        if readable.is_empty() {
            bail!("no readable .pk3 archives in {}", base.display());
        }
        let referenced = readable.iter().map(|_| AtomicBool::new(false)).collect();
        Ok(Self {
            archives: readable.iter().map(|p| p.path.clone()).collect(),
            paks: readable,
            referenced,
            index,
            alias_index,
            open: Mutex::new(HashMap::new()),
            overlay: HashMap::new(),
        })
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

    /// Marks `path`'s pak referenced as a read would, without reading it.
    pub fn touch(&self, path: &str) {
        let key = path.to_lowercase();
        if let Some((ai, _)) = self.index.get(&key).or_else(|| self.alias_index.get(&key))
            && flags_reference(&key)
        {
            self.referenced[*ai].store(true, Ordering::Relaxed);
        }
    }

    /// The `cp` command `CL_SendPureChecksums` sends after a map load, keyed
    /// with the gamestate's `checksumFeed` (`crate::pak_checksum::pure_command`).
    /// The general list is every referenced pak but the `localized_*` ones.
    pub fn pure_command(&self, checksum_feed: i32) -> String {
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
        crate::pak_checksum::pure_command(
            dll("cgame_mp_x86.dll"),
            dll("ui_mp_x86.dll"),
            &general,
            checksum_feed,
        )
    }

    /// The pak `path` comes from, by the same lookup as `read`.
    pub fn source_pak(&self, path: &str) -> Option<&PakInfo> {
        let key = path.to_lowercase();
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
            archives: Vec::new(),
            paks: Vec::new(),
            referenced: Vec::new(),
            index: HashMap::new(),
            alias_index: HashMap::new(),
            open: Mutex::new(HashMap::new()),
            overlay: HashMap::new(),
        }
    }

    /// Serves `bytes` for `path` ahead of every archive, the way a later pak
    /// overrides an earlier one. For tests that run a patched copy of an asset.
    pub fn overlay(&mut self, path: &str, bytes: Vec<u8>) {
        self.overlay.insert(path.to_lowercase(), bytes);
    }

    /// Same lookup as `read`, without touching the archive.
    pub fn contains(&self, path: &str) -> bool {
        let key = path.to_lowercase();
        self.overlay.contains_key(&key)
            || self.index.contains_key(&key)
            || self.alias_index.contains_key(&key)
    }

    /// The archive `read(path)` would hit.
    pub fn source_archive(&self, path: &str) -> Option<&Path> {
        let key = path.to_lowercase();
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
        let key = path.to_lowercase();
        if let Some(bytes) = self.overlay.get(&key) {
            return (bytes.len() as u64 <= limit).then(|| bytes.clone());
        }
        let (ai, entry) = self
            .index
            .get(&key)
            .or_else(|| self.alias_index.get(&key))?;
        if flags_reference(&key) {
            self.referenced[*ai].store(true, Ordering::Relaxed);
        }
        // A poisoned lock still holds a usable cache; `by_name` re-seeks from
        // the central directory.
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        let zip = match open.entry(*ai) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => {
                let file = File::open(&self.archives[*ai]).ok()?;
                e.insert(zip::ZipArchive::new(file).ok()?)
            }
        };
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

    /// Lowercased entry paths starting with `prefix`, e.g. "xanim/".
    pub fn list_prefix(&self, prefix: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .index
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        names.sort();
        names
    }

    pub fn find_maps(&self) -> Vec<String> {
        let mut maps: Vec<String> = self
            .index
            .keys()
            .filter(|k| k.ends_with(".bsp"))
            .filter(|k| {
                let dir = &k[..k.rfind('/').unwrap_or(0)];
                dir == "maps" || dir == "maps/mp"
            })
            .map(|k| k[k.rfind('/').unwrap() + 1..k.len() - 4].to_string())
            .collect();
        maps.sort();
        maps.dedup();
        maps
    }

    pub fn resolve_map(&self, name: &str) -> Option<String> {
        let name = name.to_lowercase();
        for candidate in [format!("maps/{name}.bsp"), format!("maps/mp/{name}.bsp")] {
            if let Some((_, entry)) = self.index.get(&candidate) {
                return Some(entry.clone());
            }
        }
        None
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

    #[test]
    fn case_insensitive_lookup() {
        let dir = tempfile::tempdir().unwrap();
        make_pk3(dir.path(), "pak0.pk3", &[("maps/MP/mp_test.bsp", "data")]);
        let fs = Pk3Fs::open(dir.path()).unwrap();
        assert_eq!(fs.read("maps/mp/MP_TEST.bsp").unwrap(), b"data");
        assert!(fs.read("maps/mp/missing.bsp").is_none());
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
        let fs = Pk3Fs::open_search(dir.path(), None, Some(&[pak0])).unwrap();
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
