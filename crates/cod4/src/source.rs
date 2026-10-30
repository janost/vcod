//! CoD4's [`AssetSource`]: the walked zones' pools in load order over the
//! install's iwds. Files by path come from the zones' rawfiles, then the
//! iwds (the iwds first under an `fs_game`); an image by name from a zone
//! that carries its pixels, then `images/<name>.iwi` ([`Cod4Source::image`]).
//! A zone's assets shadow those of zones with lower load flags
//! ([`crate::install::zone_flags`]).

use crate::assets::Assets;
use crate::image::{decode_iwi, decode_zone, has_pixels, iwi_path};
use crate::install::Install;
use anyhow::{Context, Result, anyhow};
use std::sync::Arc;
use vcod_common::asset_source::{AssetCache, AssetSource};
use vcod_common::assets::{Image, ImageMissing};
use vcod_common::game::Game;
use vcod_common::pk3::Pk3Fs;

/// One zone of the search: its load flags and the order it was added in.
struct Loaded {
    flags: u8,
    seq: u32,
    assets: Arc<Assets>,
}

/// The zones loaded so far over the iwds of `main/` and the `fs_game`
/// directory.
pub struct Cod4Source {
    /// Lowest priority first: ascending flags, ties in load order.
    loaded: Vec<Loaded>,
    next_seq: u32,
    iwds: Pk3Fs,
    /// Whether a file by path comes from the iwds before the zones'
    /// rawfiles: so under an `fs_game` (`Scr_ReadFile`).
    files_first: bool,
    cache: AssetCache,
}

impl Cod4Source {
    /// The install's `main/*.iwd`, the level's usermap and `fs_game`
    /// directories' over them ([`Install::iwd_dirs`]),
    /// with no zone yet. Opening the iwds reads the wavelet tables out of
    /// `iw3mp.exe` beside `main/`.
    pub fn open(install: &Install) -> Result<Self> {
        let dirs = install.iwd_dirs();
        let iwds = Pk3Fs::open_iwd_layers(&dirs[0], &dirs[1..])?;
        Ok(Self {
            loaded: Vec::new(),
            next_seq: 0,
            iwds,
            files_first: install.fs_game().is_some(),
            cache: AssetCache::default(),
        })
    }

    /// Adds a walked zone over every zone so far (the last zone's flags).
    pub fn add_zone(&mut self, assets: Arc<Assets>) {
        let flags = self.loaded.last().map_or(0, |z| z.flags);
        self.add_zone_flagged(assets, flags);
    }

    /// Adds a walked zone loaded with `flags`: it shadows the zones with
    /// flags at or below its own, as `DB_LinkXAssetEntry` overrides.
    pub fn add_zone_flagged(&mut self, assets: Arc<Assets>, flags: u8) {
        let at = self.loaded.partition_point(|z| z.flags <= flags);
        self.loaded.insert(
            at,
            Loaded {
                flags,
                seq: self.next_seq,
                assets,
            },
        );
        self.next_seq += 1;
        self.cache.clear();
    }

    /// Drops the last zone added, as a map change unloads the map's.
    pub fn pop_zone(&mut self) -> Option<Arc<Assets>> {
        self.cache.clear();
        let i = (0..self.loaded.len()).max_by_key(|&i| self.loaded[i].seq)?;
        Some(self.loaded.remove(i).assets)
    }

    /// The zones, highest priority first.
    pub fn zones(&self) -> impl Iterator<Item = &Arc<Assets>> {
        self.loaded.iter().rev().map(|z| &z.assets)
    }

    pub fn iwds(&self) -> &Pk3Fs {
        &self.iwds
    }

    /// Where image `name`'s pixels are: a zone that carries them, else
    /// the iwds' `.iwi` if one is there.
    pub fn image_origin(&self, name: &str) -> Option<ImageOrigin> {
        if self.zone_image(name).is_some() {
            return Some(ImageOrigin::Zone);
        }
        self.iwds
            .contains(&iwi_path(name))
            .then_some(ImageOrigin::Iwd)
    }

    fn zone_image(&self, name: &str) -> Option<&crate::assets::material::Image> {
        self.zones()
            .flat_map(|z| z.images.iter())
            .find(|i| has_pixels(i) && i.name.eq_ignore_ascii_case(name))
    }

    /// Image `name` decoded (a cube's first face): the zone's pixels, else
    /// the iwds' `.iwi`. A name found in neither fails with
    /// [`ImageMissing`].
    pub fn image(&self, name: &str) -> Result<Image> {
        if let Some(img) = self.zone_image(name) {
            return decode_zone(img, 0);
        }
        let path = iwi_path(name);
        let data = self
            .iwds
            .read(&path)
            .ok_or_else(|| anyhow!(ImageMissing(path.clone())))?;
        decode_iwi(&data).with_context(|| path)
    }
}

impl Cod4Source {
    /// Material `name`'s colour map image (its 2D or colour texture, else
    /// its first), by name, off the highest-priority zone that holds the
    /// material with its textures: `$levelbriefing` names a map's loading
    /// screen out of its `_load` zone.
    pub fn material_image(&self, name: &str) -> Option<String> {
        self.zones().find_map(|z| {
            let m = z
                .materials
                .iter()
                .find(|m| !m.textures.is_empty() && m.name.eq_ignore_ascii_case(name))?;
            let t = m
                .textures
                .iter()
                .find(|t| matches!(t.semantic, 0 | 2) && t.image.is_some())
                .or_else(|| m.textures.iter().find(|t| t.image.is_some()))?;
            let img = &z.images[t.image?.index()].name;
            Some(img.trim_start_matches(',').to_string())
        })
    }

    /// Cube `name`'s faces and levels out of a zone that carries them
    /// ([`crate::image::decode_zone_cube`]): a map's reflection probes.
    pub fn zone_cube(&self, name: &str) -> Result<Vec<Vec<Image>>> {
        let img = self
            .zone_image(name)
            .ok_or_else(|| anyhow!(ImageMissing(name.to_string())))?;
        crate::image::decode_zone_cube(img)
    }
}

/// Where [`Cod4Source::image_origin`] found an image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageOrigin {
    Zone,
    Iwd,
}

impl AssetSource for Cod4Source {
    fn game(&self) -> Game {
        Game::Cod4
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        if self.files_first
            && let Some(b) = self.iwds.read(path)
        {
            return Some(b);
        }
        let raw = self
            .zones()
            .flat_map(|z| z.raw_files.iter())
            .find(|r| r.name.eq_ignore_ascii_case(path));
        match raw {
            Some(r) => Some(r.data.clone()),
            None if self.files_first => None,
            None => self.iwds.read(path),
        }
    }

    fn exists(&self, path: &str) -> bool {
        self.zones()
            .flat_map(|z| z.raw_files.iter())
            .any(|r| r.name.eq_ignore_ascii_case(path))
            || self.iwds.contains(path)
    }

    fn list(&self, dir: &str, ext: &str) -> Vec<String> {
        let ext = ext.to_ascii_lowercase();
        let prefix = match dir.trim_end_matches('/') {
            "" => String::new(),
            d => format!("{}/", d.to_ascii_lowercase()),
        };
        let mut out = self.iwds.list(dir, &ext);
        for z in self.zones() {
            for r in &z.raw_files {
                let n = r.name.to_ascii_lowercase();
                if n.starts_with(&prefix) && n.ends_with(&ext) {
                    out.push(n);
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn cache(&self) -> &AssetCache {
        &self.cache
    }
}
