//! Values out of walked stock zones, with the bulk data kept: mp_crash's
//! render world, collision map and entity string, code_post_gfx_mp's
//! materials. `--nocapture` prints them.

use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, Walked, walk};
use vcod_cod4::zone::Zone;

fn walk_zone(name: &str) -> Option<Walked> {
    let path = cod4_install()?.zone("english", name);
    let zone = Zone::open_path(&path).unwrap();
    Some(walk(zone, WalkOptions { keep_bulk: true }).unwrap())
}

#[test]
fn mp_crash_world_and_collision() {
    let Some(z) = walk_zone("mp_crash") else {
        return;
    };
    assert!(z.stats.unresolved.is_empty(), "{:?}", z.stats.unresolved);
    println!("{} offsets into earlier arrays", z.stats.into_arrays);
    let a = &z.assets;

    let [g] = &a.gfx_worlds[..] else {
        panic!("{} gfx worlds", a.gfx_worlds.len())
    };
    println!(
        "gfx_map {} ({:?}): {} vertices, {} indices, {} surfaces, {} cells, {} lightmaps, {} static models, {} materials",
        g.name,
        g.base_name,
        g.vertex_count,
        g.index_count,
        g.surface_count,
        g.cell_count,
        g.lightmap_count,
        g.static_models.len(),
        g.materials.len()
    );
    assert_eq!(g.name, "maps/mp/mp_crash.d3dbsp");
    let verts = g.vertices.as_ref().unwrap();
    assert_eq!(verts.len(), 0x2C * g.vertex_count as usize);
    assert_eq!(
        g.indices.as_ref().unwrap().len(),
        2 * g.index_count as usize
    );
    assert_eq!(g.surfaces.len(), g.surface_count as usize);
    // Every surface draws with a material, and its triangles stay inside
    // the index buffer.
    for (m, s) in &g.surfaces {
        assert!(m.is_some());
        let tri_count = u16::from_le_bytes([s[10], s[11]]) as u32;
        let base_index = u32::from_le_bytes(s[12..16].try_into().unwrap());
        assert!(base_index + 3 * tri_count <= g.index_count);
    }
    let first = &g.surfaces[0];
    let m = &a.materials[first.0.unwrap().index()];
    println!(
        "surface 0: material {} techset {:?}, {} textures",
        m.name,
        m.techset.map(|t| &a.techsets[t.index()].name),
        m.textures.len()
    );

    let [c] = &a.clip_maps_pvs[..] else {
        panic!("{} clip maps", a.clip_maps_pvs.len())
    };
    println!(
        "col_map_mp {}: {} planes, {} brushes, {} brush sides, {} nodes, {} leafs, {} submodels, {} static models, {} partitions, {} clusters",
        c.name,
        c.plane_count,
        c.brush_count,
        c.brush_side_count,
        c.node_count,
        c.leaf_count,
        c.submodel_count,
        c.static_model_count,
        c.partition_count,
        c.cluster_count
    );
    assert_eq!(c.name, "maps/mp/mp_crash.d3dbsp");
    assert_eq!(
        c.brushes.as_ref().unwrap().len(),
        0x50 * c.brush_count as usize
    );
    let ents = &a.map_ents[c.map_ents.unwrap().index()];
    let text = String::from_utf8_lossy(&ents.entity_string);
    let count = text.matches('{').count();
    println!(
        "map_ents {}: {} bytes, {count} entities",
        ents.name,
        ents.entity_string.len()
    );
    assert!(text.trim_start().starts_with('{'));
    assert!(text.contains("\"classname\" \"worldspawn\""));
}

#[test]
fn code_post_gfx_mp_materials_resolve() {
    let Some(z) = walk_zone("code_post_gfx_mp") else {
        return;
    };
    assert!(z.stats.unresolved.is_empty(), "{:?}", z.stats.unresolved);
    let a = &z.assets;
    // Materials name technique sets, every one with a name.
    let bare: Vec<_> = a
        .materials
        .iter()
        .filter(|m| m.techset.is_none())
        .map(|m| m.name.as_str())
        .collect();
    println!("materials without a techset: {bare:?}");
    for t in a.materials.iter().filter_map(|m| m.techset) {
        assert!(!a.techsets[t.index()].name.is_empty());
    }
    let shaders = a
        .techniques
        .iter()
        .flat_map(|t| &t.passes)
        .filter(|p| {
            p.vertex_shader
                .as_ref()
                .is_some_and(|s| s.program.is_some())
        })
        .count();
    println!(
        "{} materials, {} techsets, {} techniques, {} passes with vertex bytecode, {} images",
        a.materials.len(),
        a.techsets.len(),
        a.techniques.len(),
        shaders,
        a.images.len()
    );
    assert!(shaders > 0);
}
