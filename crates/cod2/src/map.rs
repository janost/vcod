//! The map loader: a `.d3dbsp` read once ([`MapFile`]) and what the client
//! and the server build from it, the neutral [`MapWorld`] and the world's
//! [`ClipMap`] ([`LoadedMap`]). Past this module nothing needs the
//! `D3dBsp`: the collision takes [`ClipLumps`], which a CoD4 `clipMap_t`
//! loader can fill as well.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use vcod_common::asset_source::{Asset, AssetKind, AssetSource};
use vcod_common::collision::{CollisionWorld, ModelTri};
use vcod_common::mapworld::MapWorld;

use crate::collision::{ClipLumps, ClipMap};
use crate::d3dbsp::D3dBsp;

/// A map's file, parsed once.
pub struct MapFile {
    bsp: D3dBsp,
}

impl MapFile {
    /// Parses a `.d3dbsp` ([`D3dBsp::parse`]).
    pub fn parse(data: &[u8]) -> Result<MapFile> {
        Ok(MapFile {
            bsp: D3dBsp::parse(data)?,
        })
    }

    /// The neutral world (`crate::world::from_bsp`).
    pub fn world(&self) -> MapWorld {
        crate::world::from_bsp(&self.bsp)
    }

    /// The world's collision model.
    pub fn clip(&self) -> Result<ClipMap> {
        clip_map(&self.bsp)
    }

    /// The box the `$outdoor` image spans ([`crate::outdoor::sky_bounds`]).
    pub fn outdoor_bounds(&self) -> (glam::Vec3, glam::Vec3) {
        crate::outdoor::sky_bounds(&self.bsp)
    }

    /// The world and the collision, the latter built on a thread beside
    /// the former.
    pub fn load(&self) -> Result<LoadedMap> {
        let (world, clip) = std::thread::scope(|sc| {
            let clip = sc.spawn(|| self.clip());
            let world = self.world();
            (world, vcod_common::laps::joined(clip))
        });
        Ok(LoadedMap {
            world,
            clip: Arc::new(clip?),
        })
    }
}

/// A map by its name (`mp_toujane`), `maps/mp/<name>.d3dbsp` read through
/// [`AssetSource::read`] (so a `Pk3Fs` marks the map's iwd) and parsed.
/// Built through [`Asset::find`], uncached: the level keeps what it builds
/// off the file, and the source's cache would keep the bytes' parse resident.
impl Asset for MapFile {
    const KIND: AssetKind = AssetKind::Cod2Map;

    fn find(src: &dyn AssetSource, name: &str) -> Result<Self> {
        let path = format!("maps/mp/{name}.d3dbsp");
        let data = src
            .read(&path)
            .ok_or_else(|| anyhow!("{path} not in the mounted paks"))?;
        MapFile::parse(&data)
    }
}

/// A map as the game takes it: the world and its collision model, one
/// [`ClipMap`] shared by every engine trace and the combat's.
pub struct LoadedMap {
    pub world: MapWorld,
    pub clip: Arc<ClipMap>,
}

impl LoadedMap {
    /// The map `name` off `src` ([`MapFile::find`]) with both halves built.
    pub fn find(src: &dyn AssetSource, name: &str) -> Result<LoadedMap> {
        MapFile::find(src, name)?.load()
    }

    /// Parses `data` and builds both halves ([`MapFile::load`]).
    pub fn from_bytes(data: &[u8]) -> Result<LoadedMap> {
        MapFile::parse(data)?.load()
    }

    /// The engine's collision world over the shared clip
    /// ([`crate::collision::collision_world`]).
    pub fn collision(&self, model_tris: &[ModelTri]) -> CollisionWorld {
        crate::collision::collision_world(&self.world, self.clip.clone(), model_tris)
    }
}

/// What [`crate::collision::build`] reads off a parsed `.d3dbsp`.
pub fn clip_lumps(b: &D3dBsp) -> ClipLumps<'_> {
    ClipLumps {
        planes: &b.planes,
        materials: &b.materials,
        brushes: &b.brushes,
        brush_sides: &b.brush_sides,
        brush_boxes: b.brush_boxes(),
        nodes: &b.nodes,
        leafs: &b.leafs,
        leaf_brushes: &b.leaf_brushes,
        coll_verts: &b.coll_verts,
        coll_edges: &b.coll_edges,
        coll_tris: &b.coll_tris,
        coll_borders: &b.coll_borders,
        coll_partitions: &b.coll_partitions,
        coll_aabb_trees: &b.coll_aabb_trees,
    }
}

/// A parsed map's [`ClipMap`].
pub fn clip_map(b: &D3dBsp) -> Result<ClipMap> {
    crate::collision::build(&clip_lumps(b))
}

/// [`crate::collision::collision_world`] over a parsed map, `world` its
/// `crate::world::from_bsp`.
pub fn collision_world(
    b: &D3dBsp,
    world: &MapWorld,
    model_tris: &[ModelTri],
) -> Result<CollisionWorld> {
    Ok(crate::collision::collision_world(
        world,
        Arc::new(clip_map(b)?),
        model_tris,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_map_asset_reads_its_d3dbsp_and_marks_its_iwd() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let data = fs.read("maps/mp/mp_toujane.d3dbsp").unwrap();
        let direct = MapFile::parse(&data).unwrap().world();
        fs.clear_references();
        let found = MapFile::find(&fs, "mp_toujane").unwrap().world();
        assert_eq!(found.entities, direct.entities);
        assert_eq!(fs.referenced_paks().len(), 1);
        assert!(MapFile::find(&fs, "mp_nowhere").is_err());
    }
}
