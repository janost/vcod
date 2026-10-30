//! Assets by kind and name over any game's storage. CoD1 and CoD2 keep
//! their assets as files in zip archives ([`Pk3Fs`]); CoD4 keeps them in
//! fastfile pools keyed by type and name, with no paths. Code that asks for
//! an [`Asset`] through [`load`] works over either; each asset
//! type says where it lives on a path-based source ([`Asset::find`]).

use crate::game::Game;
use crate::pk3::Pk3Fs;
use anyhow::{Result, bail};
use std::any::Any;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The asset kinds [`Asset`] is implemented for. More join as their
/// consumers move onto [`load`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AssetKind {
    /// A weapon file, by its configstring 7 name (`kar98k_mp`).
    Weapon,
    /// Every localized string table of a language (`english`).
    LocalizedStrings,
    /// Every sound alias table, merged; the name is unused (`""`).
    SoundAliases,
    /// A file read whole, by its path (`maps/mp/gametypes/dm.gsc`,
    /// `default_mp.cfg`): scripts and text tables. CoD4's rawfile pool.
    RawFile,
    /// A texture decoded, by its file's path with the extension
    /// (`gfx/hud/hud@health.dds`, `images/brick.iwi`): CoD1's `.dds`,
    /// `.tga` and `.jpg`, CoD2's `.iwi`. CoD4's image pool.
    Image,
    /// CoD1's `.shader` scripts, every one parsed into a
    /// [`crate::shader::ShaderLib`]; the name is unused (`""`).
    ShaderScripts,
    /// CoD2's binary materials, each compiled with its technique set into
    /// one library (`vcod_cod2::materials::Compiled`); the name is unused.
    Cod2Materials,
    /// An xmodel with its parts and every LOD's surfaces, by its `xmodel/`
    /// entry's name (`viewmodel_kar98k`): CoD1's v14, CoD2's v20, both in
    /// [`crate::model::Model`]. CoD4's xmodel pool.
    Model,
    /// An xanim clip, by its `xanim/` entry's name (`pb_stand_alert`):
    /// CoD1's and CoD2's, both in [`crate::xanim::XAnim`]. CoD4's xanim
    /// pool.
    Anim,
    /// A particle effect, by its `fx/` path (`fx/impacts/dirthit_mortar.efx`):
    /// CoD1's text grammar and CoD2's primitives, both as the client's
    /// `fx::efx::Effect`. CoD4's fx pool.
    Effect,
    /// CoD2's parsed `.efx` (`vcod_cod2::efx::Effect`), by the name
    /// `FX_RegisterEffect` takes (`fx/a/b`, `/fx/a/b`, `fx/a/b.efx`).
    Cod2Effect,
    /// CoD1's impact effect table, every `fx/*.csv` merged; the name is
    /// unused (`""`). CoD4's impactfx pool.
    ImpactEffects,
    /// CoD2's impact effect table, `fx/` then `fx/maps/<map>/`, by the
    /// map's name (`""` for `fx/` alone).
    Cod2ImpactEffects,
    /// A UI menu file parsed in its game's grammar, `#include`s read, by
    /// its path (`ui_mp/main.menu`): CoD1's and CoD2's, both in
    /// [`crate::ui_menu::MenuFile`]. CoD4's menufile pool.
    Menu,
    /// A CoD1 map loaded into a [`crate::mapworld::MapWorld`], by its name
    /// (`mp_carentan`): `maps/` or `maps/mp/<name>.bsp`, IBSP 59. CoD4's
    /// clipmap and gfxworld pools.
    Map,
    /// CoD2's `.d3dbsp` parsed (`vcod_cod2::map::MapFile`), by the map's
    /// name: `maps/mp/<name>.d3dbsp`.
    Cod2Map,
}

/// A game's asset storage. [`Pk3Fs`] is the path-based one: CoD1's pk3s
/// and CoD2's iwds. Shared across threads: the parallel loaders
/// ([`crate::laps::par_map`]) read through one source on every core.
pub trait AssetSource: Send + Sync {
    /// The game whose assets these are; an [`Asset`] picks its path by it.
    fn game(&self) -> Game;

    /// A file's bytes, `None` when missing or unreadable. Case-insensitive.
    fn read(&self, path: &str) -> Option<Vec<u8>>;

    /// Whether [`Self::read`] would find `path`, without reading it.
    fn exists(&self, path: &str) -> bool;

    /// Every file under `dir/`, at any depth, whose name ends in `ext`
    /// (`".csv"`; `""` for all): lowercased full paths, sorted, each once.
    /// A `dir` of `""` lists the whole source.
    fn list(&self, dir: &str, ext: &str) -> Vec<String>;

    /// Where [`load`] keeps what it loaded.
    fn cache(&self) -> &AssetCache;

    /// Clip `name` out of a source that keeps clips decoded rather than as
    /// `xanim/` files (CoD4's xanim pool); `None` reads the file.
    fn decoded_anim(&self, _name: &str) -> Option<Result<crate::xanim::XAnim>> {
        None
    }

    /// Model `name` out of a source that keeps models decoded rather than
    /// as `xmodel/` files (CoD4's xmodel pool); `None` reads the files.
    fn decoded_model(&self, _name: &str) -> Option<Result<crate::model::Model>> {
        None
    }
}

impl dyn AssetSource + '_ {
    /// [`load`] through a trait object.
    pub fn load<T: Asset>(&self, name: &str) -> Result<Arc<T>> {
        load(self, name)
    }
}

impl Pk3Fs {
    /// [`load`] from this search path.
    pub fn load<T: Asset>(&self, name: &str) -> Result<Arc<T>> {
        load(self, name)
    }
}

/// A type an [`AssetSource`] loads by name.
pub trait Asset: Any + Send + Sync + Sized {
    const KIND: AssetKind;

    /// Builds the asset `name` from `src`, uncached.
    fn find(src: &dyn AssetSource, name: &str) -> Result<Self>;
}

/// A file's bytes as stored, by path. The load reads through
/// [`AssetSource::read`], so a [`Pk3Fs`] marks the pak it came from as a
/// plain read would; [`Pk3Fs::clear_references`] drops the cache with the
/// flags, so the first load after it reads (and marks) again.
pub struct RawFile {
    bytes: Vec<u8>,
}

impl RawFile {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The bytes as text, invalid UTF-8 replaced (`from_utf8_lossy`).
    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.bytes)
    }
}

impl Asset for RawFile {
    const KIND: AssetKind = AssetKind::RawFile;

    fn find(src: &dyn AssetSource, path: &str) -> Result<Self> {
        match src.read(path) {
            Some(bytes) => Ok(RawFile { bytes }),
            None => bail!("{path} not found"),
        }
    }
}

/// Loaded assets keyed by (kind, lowercased name). Only successes stay.
#[derive(Default)]
pub struct AssetCache {
    loaded: Mutex<HashMap<(AssetKind, String), Arc<dyn Any + Send + Sync>>>,
}

impl AssetCache {
    /// Drops everything, so the next load reads its files again.
    pub fn clear(&self) {
        self.lock().clear();
    }

    fn lock(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<(AssetKind, String), Arc<dyn Any + Send + Sync>>> {
        // A poisoned map still holds whole entries.
        self.loaded.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// `T`'s asset `name` from `src`, found on the first ask and shared after
/// through `src`'s [`AssetCache`].
pub fn load<T: Asset>(src: &dyn AssetSource, name: &str) -> Result<Arc<T>> {
    let key = (T::KIND, name.to_ascii_lowercase());
    let hit = src.cache().lock().get(&key).cloned();
    let any = match hit {
        Some(any) => any,
        None => {
            // Found outside the lock: an asset's find may load others.
            let fresh: Arc<dyn Any + Send + Sync> = Arc::new(T::find(src, name)?);
            src.cache().lock().entry(key).or_insert(fresh).clone()
        }
    };
    any.downcast::<T>()
        .map_err(|_| anyhow::anyhow!("{:?} {name}: cached under another type", T::KIND))
}

impl AssetSource for Pk3Fs {
    fn game(&self) -> Game {
        Pk3Fs::game(self)
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        Pk3Fs::read(self, path)
    }

    fn exists(&self, path: &str) -> bool {
        self.contains(path)
    }

    fn list(&self, dir: &str, ext: &str) -> Vec<String> {
        let ext = ext.to_ascii_lowercase();
        let prefix = match dir.trim_end_matches('/') {
            "" => String::new(),
            dir => format!("{}/", dir.to_ascii_lowercase()),
        };
        let mut names = self.list_prefix(&prefix);
        names.retain(|n| n.ends_with(&ext));
        names
    }

    fn cache(&self) -> &AssetCache {
        self.asset_cache()
    }
}

/// `game`'s search path over `base` (the install's `main/`), `game_dir`'s
/// archives on top: CoD1's pk3s (UO's `uo/` over its sibling `main/`,
/// [`crate::pk3::base_dirs`]), CoD2's iwds. CoD4 has no path-based
/// storage, so it fails.
pub fn open_game_fs(game: Game, base: &Path, game_dir: Option<&Path>) -> Result<Pk3Fs> {
    match game {
        Game::Cod1 => Pk3Fs::open_layered(base, game_dir),
        Game::Cod2 => Pk3Fs::open_iwd(base, game_dir),
        Game::Cod4 => bail!("CoD4 assets live in fastfiles, which vcod cannot read yet"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Files in a map, for the trait's own tests.
    struct MemSource {
        game: Game,
        files: HashMap<String, Vec<u8>>,
        reads: AtomicUsize,
        cache: AssetCache,
    }

    impl MemSource {
        fn new(game: Game, files: &[(&str, &str)]) -> Self {
            MemSource {
                game,
                files: files
                    .iter()
                    .map(|(p, b)| (p.to_ascii_lowercase(), b.as_bytes().to_vec()))
                    .collect(),
                reads: AtomicUsize::new(0),
                cache: AssetCache::default(),
            }
        }
    }

    impl AssetSource for MemSource {
        fn game(&self) -> Game {
            self.game
        }
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            self.files.get(&path.to_ascii_lowercase()).cloned()
        }
        fn exists(&self, path: &str) -> bool {
            self.files.contains_key(&path.to_ascii_lowercase())
        }
        fn list(&self, dir: &str, ext: &str) -> Vec<String> {
            let prefix = format!("{}/", dir.trim_end_matches('/').to_ascii_lowercase());
            let mut v: Vec<String> = self
                .files
                .keys()
                .filter(|k| k.starts_with(&prefix) && k.ends_with(ext))
                .cloned()
                .collect();
            v.sort();
            v
        }
        fn cache(&self) -> &AssetCache {
            &self.cache
        }
    }

    const KAR: &str = "WEAPONFILE\\displayName\\WEAPON_KAR98K\\damage\\100";

    #[test]
    fn load_caches_by_kind_and_name() {
        let src = MemSource::new(Game::Cod1, &[("weapons/mp/kar98k_mp", KAR)]);
        let a = load::<crate::weapon::WeaponDef>(&src, "kar98k_mp").unwrap();
        let b = load::<crate::weapon::WeaponDef>(&src, "KAR98K_MP").unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(src.reads.load(Ordering::Relaxed), 1);
        assert_eq!(a.damage, 100);
        // A miss is not cached, and errors every time.
        assert!(load::<crate::weapon::WeaponDef>(&src, "mp44_mp").is_err());
        assert!(load::<crate::weapon::WeaponDef>(&src, "mp44_mp").is_err());
        assert_eq!(src.reads.load(Ordering::Relaxed), 3);
        src.cache().clear();
        let dyn_src: &dyn AssetSource = &src;
        let c = dyn_src
            .load::<crate::weapon::WeaponDef>("kar98k_mp")
            .unwrap();
        assert!(!Arc::ptr_eq(&a, &c));
        assert_eq!(src.reads.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn localized_strings_path_follows_the_game() {
        let text = "REFERENCE HELLO\nLANG_ENGLISH \"Hello\"\n";
        let cod1 = MemSource::new(Game::Cod1, &[("localizedstrings/english/menu.str", text)]);
        let cod2 = MemSource::new(Game::Cod2, &[("localizedstrings/menu.str", text)]);
        for src in [&cod1, &cod2] {
            let l = load::<crate::localize::Localized>(src, "english").unwrap();
            assert_eq!(l.get("MENU_HELLO"), Some("Hello"), "{:?}", src.game);
        }
        // Each game's path alone: the other's layout is not read.
        let crossed = MemSource::new(Game::Cod2, &[("localizedstrings/english/menu.str", text)]);
        assert_eq!(
            load::<crate::localize::Localized>(&crossed, "english")
                .unwrap()
                .get("MENU_HELLO"),
            None
        );
    }

    #[test]
    fn pk3fs_lists_by_dir_and_ext() {
        let mut fs = Pk3Fs::empty();
        fs.overlay("soundaliases/A.csv", b"a".to_vec());
        fs.overlay("soundaliases/sub/b.csv", b"b".to_vec());
        fs.overlay("soundaliases/c.txt", b"c".to_vec());
        fs.overlay("soundaliasesx/d.csv", b"d".to_vec());
        let src: &dyn AssetSource = &fs;
        assert_eq!(
            src.list("soundaliases", ".csv"),
            ["soundaliases/a.csv", "soundaliases/sub/b.csv"]
        );
        assert!(src.exists("SOUNDALIASES/a.csv"));
        assert_eq!(src.game(), Game::Cod1);
    }

    #[test]
    fn raw_files_mark_their_pak_after_each_clear() {
        let dir = tempfile::tempdir().unwrap();
        let pk3 = crate::testing::pk3_with(&[("maps/MP/a.gsc", b"main() {}")]);
        std::fs::write(dir.path().join("pak0.pk3"), pk3).unwrap();
        let fs = Pk3Fs::open(dir.path()).unwrap();
        let referenced = |fs: &Pk3Fs| fs.referenced_paks().len();
        let a = fs.load::<RawFile>("maps/mp/a.gsc").unwrap();
        assert_eq!((a.text().as_ref(), referenced(&fs)), ("main() {}", 1));
        fs.clear_references();
        assert_eq!(referenced(&fs), 0);
        let b = fs.load::<RawFile>("MAPS/mp/a.gsc").unwrap();
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(referenced(&fs), 1);
        assert!(fs.load::<RawFile>("maps/mp/b.gsc").is_err());
    }

    #[test]
    fn overlay_drops_the_cache() {
        let mut fs = Pk3Fs::empty();
        fs.overlay("weapons/mp/kar98k_mp", KAR.as_bytes().to_vec());
        let a = fs.load::<crate::weapon::WeaponDef>("kar98k_mp").unwrap();
        fs.overlay("weapons/mp/kar98k_mp", b"WEAPONFILE\\damage\\50".to_vec());
        let b = fs.load::<crate::weapon::WeaponDef>("kar98k_mp").unwrap();
        assert_eq!((a.damage, b.damage), (100, 50));
    }

    #[test]
    fn cod4_has_no_path_fs() {
        let dir = tempfile::tempdir().unwrap();
        let Err(e) = open_game_fs(Game::Cod4, dir.path(), None) else {
            panic!("CoD4 opened");
        };
        assert!(e.to_string().contains("fastfile"), "{e}");
    }

    #[test]
    fn cod2_fs_loads_typed_assets() {
        let Some(fs) = crate::testing::cod2_fs() else {
            return;
        };
        assert_eq!(AssetSource::game(&fs), Game::Cod2);
        let kar = fs.load::<crate::weapon::WeaponDef>("kar98k_mp").unwrap();
        assert_eq!(kar.display_name, "WEAPON_KAR98K");
        let loc = fs.load::<crate::localize::Localized>("english").unwrap();
        assert_eq!(
            loc.get("DUHOC_PLATFORM_USEROPE"),
            Some("Press USE [&&1] to climb the rope.")
        );
        let dir = std::path::PathBuf::from(std::env::var_os("COD2_DIR").unwrap()).join("main");
        let opened = open_game_fs(Game::Cod2, &dir, None).unwrap();
        assert_eq!(opened.paks().count(), fs.paks().count());
    }

    #[test]
    fn cod1_fs_loads_typed_assets() {
        let Some(fs) = crate::testing::stock_fs() else {
            return;
        };
        assert_eq!(AssetSource::game(&fs), Game::Cod1);
        let kar = fs.load::<crate::weapon::WeaponDef>("kar98k_mp").unwrap();
        let direct = crate::weapon::load(&fs, "kar98k_mp").unwrap();
        assert_eq!(
            (&kar.display_name, kar.damage),
            (&direct.display_name, direct.damage)
        );
        let loc = fs.load::<crate::localize::Localized>("english").unwrap();
        assert!(loc.get("MPSCRIPT_KILLCAM").is_some());
    }
}
