//! CoD2's collision: the world's [`ClipMap`] built from a `.d3dbsp`'s
//! collision lumps ([`build`]) and the engine's [`CollisionWorld`] over it.
//! The runtime records and the trace kernels are the CoD2 family's, in
//! `vcod_common::clipmap`, re-exported here.

use std::sync::Arc;

use anyhow::Result;
use glam::Vec3;
use vcod_common::collision::{CollisionWorld, ModelTri};
use vcod_common::mapworld::{LeafLists, MapWorld};

pub use vcod_common::clipmap::*;

use crate::d3dbsp::{
    BRUSH_CONTENTS_MASK, Brush, BrushBox, BrushSide, CollEdge, CollVert, Leaf, Material, Node,
    Plane, partition_brushes,
};

/// What [`build`] reads: the collision lumps as the map loader
/// hands them over (`crate::map::clip_lumps` for a `.d3dbsp`), and each
/// brush's box (`D3dBsp::brush_boxes`).
pub struct ClipLumps<'a> {
    pub planes: &'a [Plane],
    pub materials: &'a [Material],
    pub brushes: &'a [Brush],
    pub brush_sides: &'a [BrushSide],
    pub brush_boxes: Vec<BrushBox>,
    pub nodes: &'a [Node],
    pub leafs: &'a [Leaf],
    pub leaf_brushes: &'a [i32],
    pub coll_verts: &'a [CollVert],
    pub coll_edges: &'a [CollEdge],
    pub coll_tris: &'a [CollTri],
    pub coll_borders: &'a [CollBorder],
    pub coll_partitions: &'a [CollPartition],
    pub coll_aabb_trees: &'a [CollAabbTree],
}
/// `CM_LoadMapFromBsp`'s collision half: the [`ClipMap`] of `b`.
pub fn build(b: &ClipLumps) -> Result<ClipMap> {
    let planes = b
        .planes
        .iter()
        .map(|p| {
            let n = p.normal;
            let kind = (0..3).find(|&k| n[k] == 1.0).unwrap_or(3) as u8;
            let signbits = (0..3).fold(0u8, |s, k| s | (u8::from(n[k] < 0.0) << k));
            CPlane {
                normal: n,
                dist: p.dist,
                kind,
                signbits,
            }
        })
        .collect();
    let materials: Vec<(u32, u32)> = b
        .materials
        .iter()
        .map(|m| (m.surface_flags, m.content_flags))
        .collect();
    let boxes = &b.brush_boxes;
    let brushes = b
        .brushes
        .iter()
        .zip(boxes)
        .map(|(br, bx)| {
            let s = &b.brush_sides[br.first_side as usize..][..br.num_sides as usize];
            let mut axial_material = [[0u16; 3]; 2];
            for (k, side) in s[..6].iter().enumerate() {
                axial_material[k % 2][k / 2] = side.material as u16;
            }
            CBrush {
                mins: Vec3::from_array(bx.mins),
                maxs: Vec3::from_array(bx.maxs),
                contents: bx.contents,
                sides: s[6..]
                    .iter()
                    .map(|side| (side.plane_or_bound, side.material as u16))
                    .collect(),
                axial_material,
            }
        })
        .collect::<Vec<_>>();

    // One shared rewrite of lump 27, as `CMod_LoadBrushRelated` does it.
    let mut leaf_brush_list: Vec<u16> = b.leaf_brushes.iter().map(|&i| i as u16).collect();
    leaf_brush_list.push(0);
    let mut brush_nodes = vec![BrushNode::Leaf {
        first: 0,
        count: 0,
        contents: 0,
    }];
    let mut leafs = Vec::with_capacity(b.leafs.len());
    for (i, l) in b.leafs.iter().enumerate() {
        let (first, count) = (l.first_leaf_brush as usize, l.leaf_brush_count as usize);
        let mut leaf = CLeaf {
            first_aabb: l.first_coll_aabb as u16,
            aabb_count: l.coll_aabb_count as u16,
            brush_contents: 0,
            terrain_contents: 0,
            mins: Vec3::ZERO,
            maxs: Vec3::ZERO,
            brush_node: 0,
        };
        for t in &b.coll_aabb_trees[first_run(l.first_coll_aabb, l.coll_aabb_count)] {
            leaf.terrain_contents |= materials[t.material as usize].1;
        }
        if count > 0 {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(-f32::MAX));
            for &br in &leaf_brush_list[first..first + count] {
                let bx = &brushes[br as usize];
                leaf.brush_contents |= bx.contents;
                for k in 0..3 {
                    if bx.mins[k] < lo[k] {
                        lo[k] = bx.mins[k];
                    }
                    if hi[k] < bx.maxs[k] {
                        hi[k] = bx.maxs[k];
                    }
                }
            }
            leaf.mins = lo - Vec3::splat(0.125);
            leaf.maxs = hi + Vec3::splat(0.125);
            let tree = partition_brushes(boxes, &mut leaf_brush_list, first, count)
                .map_err(|e| e.context(format!("leaf {i}")))?;
            leaf.brush_node = brush_nodes.len() as u32;
            brush_nodes.extend(tree.nodes);
        }
        leafs.push(leaf);
    }

    Ok(ClipMap {
        planes,
        nodes: b
            .nodes
            .iter()
            .map(|n| CNode {
                plane: n.plane as u32,
                children: n.children,
            })
            .collect(),
        leafs,
        brush_nodes,
        leaf_brush_list,
        brushes,
        materials,
        verts: b.coll_verts.iter().map(|v| v.xyz).collect(),
        edges: b
            .coll_edges
            .iter()
            .map(|e| {
                // The reciprocal goes to the scale helper as a float.
                let inv = (1.0 / f64::from(e.length)) as f32;
                CEdge {
                    origin: e.origin,
                    axis: e.axis,
                    dir_scaled: (e.dir.as_dvec3() * f64::from(inv)).as_vec3(),
                }
            })
            .collect(),
        tris: b.coll_tris.to_vec(),
        borders: b.coll_borders.to_vec(),
        partitions: b.coll_partitions.to_vec(),
        aabbs: b.coll_aabb_trees.to_vec(),
        indexed: None,
        reach: Default::default(),
    })
}

/// The engine's [`CollisionWorld`] for a CoD2 map: the world through
/// `clip`, the submodels and `model_tris` through the shared clip. `world`
/// is the map's [`MapWorld`] and `clip` its [`ClipMap`] (both off one
/// load, `crate::map::LoadedMap`); model 0 is left out of the shared clip,
/// and the submodels' brushes take the loader's contents mask.
pub fn collision_world(
    world: &MapWorld,
    clip: Arc<ClipMap>,
    model_tris: &[ModelTri],
) -> CollisionWorld {
    // What `CollisionWorld::build` reads, model 0's brushes and partitions
    // left out.
    let mut rest = MapWorld {
        materials: world.materials.clone(),
        entities: world.entities.clone(),
        planes: world.planes.clone(),
        nodes: world.nodes.clone(),
        leafs: world.leafs.clone(),
        brush_sides: world.brush_sides.clone(),
        brushes: world.brushes.clone(),
        models: world.models.clone(),
        terrain: world.terrain.clone(),
        collision_verts: world.collision_verts.clone(),
        collision_indices: world.collision_indices.clone(),
        leaf_lists: vec![
            LeafLists {
                brushes: (0, 0),
                partitions: (0, 0),
            };
            world.leaf_lists.len()
        ],
        leaf_partitions: world.leaf_partitions.clone(),
        ..MapWorld::default()
    };
    rest.models[0].num_brushes = 0;
    for m in &mut rest.materials {
        m.content_flags &= BRUSH_CONTENTS_MASK;
    }
    let mut cw = CollisionWorld::build(&rest, model_tris);
    cw.set_world_clip(clip);
    cw
}

/// A `(first, count)` run as a range.
fn first_run(first: i32, count: i32) -> std::ops::Range<usize> {
    first as usize..(first + count) as usize
}
