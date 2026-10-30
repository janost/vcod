//! The 15 stock MP maps parse, and their record counts are the census's
//! (`docs/research/cod2-d3dbsp-format.md`, "Census"; rerun with
//! `python3 -I tools/re/d3dbsp_census.py`). Needs `COD2_DIR`.

use vcod_cod2::d3dbsp::D3dBsp;
use vcod_common::testing;

/// map, then materials, planes, brushes, sides, surfaces, verts, lightmaps,
/// grid, grid colours, cells, portals, aabb trees, cull groups, nodes,
/// leafs, leaf brushes, coll verts, edges, tris, borders, partitions, coll
/// aabbs, models, clusters: the doc's table column for column.
const CENSUS: [(&str, [usize; 24]); 15] = [
    (
        "mp_breakout",
        [
            304, 13822, 10425, 95559, 5983, 243087, 3, 49239, 13659, 10, 92, 941, 0, 993, 1029,
            15487, 9653, 19206, 16887, 2902, 1723, 3291, 35, 477,
        ],
    ),
    (
        "mp_brecourt",
        [
            121, 11638, 3429, 37416, 1230, 56146, 2, 138664, 5086, 10, 46, 176, 17, 2198, 2280,
            6303, 8023, 14095, 13570, 196, 2156, 3465, 81, 766,
        ],
    ),
    (
        "mp_burgundy",
        [
            235, 6903, 3469, 31978, 2110, 81385, 3, 29591, 7193, 18, 122, 322, 0, 620, 628, 6474,
            4643, 9678, 7997, 1211, 719, 1662, 7, 306,
        ],
    ),
    (
        "mp_carentan",
        [
            260, 9561, 8072, 63875, 3929, 177543, 2, 41305, 14067, 28, 272, 625, 26, 2087, 2097,
            18470, 8657, 18216, 13283, 2521, 1101, 2915, 9, 1129,
        ],
    ),
    (
        "mp_dawnville",
        [
            230, 8771, 6951, 55451, 3531, 147884, 2, 30710, 9652, 27, 196, 545, 5, 1018, 1054,
            12326, 7854, 15329, 11517, 1170, 1139, 2646, 35, 455,
        ],
    ),
    (
        "mp_decoy",
        [
            80, 5400, 1335, 16716, 618, 32536, 1, 37871, 7687, 2, 4, 90, 0, 639, 683, 2351, 6173,
            10983, 11835, 421, 1656, 2621, 43, 276,
        ],
    ),
    (
        "mp_downtown",
        [
            140, 6425, 9863, 72142, 4410, 208080, 3, 53120, 9805, 4, 12, 663, 0, 514, 527, 11909,
            3886, 6500, 6287, 106, 829, 1289, 12, 204,
        ],
    ),
    (
        "mp_farmhouse",
        [
            314, 12929, 6070, 64647, 2278, 101987, 2, 66231, 13518, 25, 156, 314, 0, 1588, 1607,
            9236, 5627, 10635, 9559, 1233, 1110, 2507, 18, 649,
        ],
    ),
    (
        "mp_harbor",
        [
            248, 10204, 8267, 69533, 5748, 231659, 4, 26128, 7083, 18, 154, 892, 40, 3861, 3870,
            18687, 9781, 20615, 14659, 2263, 1100, 3072, 8, 1983,
        ],
    ),
    (
        "mp_leningrad",
        [
            203, 9125, 9818, 79608, 3377, 197303, 3, 32623, 9516, 4, 16, 502, 0, 332, 345, 11851,
            5578, 9551, 8566, 251, 1196, 1651, 12, 150,
        ],
    ),
    (
        "mp_matmata",
        [
            204, 11713, 8898, 79208, 5095, 181322, 3, 47599, 11250, 7, 56, 798, 0, 748, 773, 13213,
            18522, 40185, 26366, 8620, 1519, 2398, 24, 326,
        ],
    ),
    (
        "mp_railyard",
        [
            188, 7746, 11496, 83572, 4283, 207891, 3, 54795, 21367, 23, 112, 679, 22, 1246, 1259,
            16406, 5692, 11213, 8629, 1275, 1001, 1880, 12, 544,
        ],
    ),
    (
        "mp_rhine",
        [
            253, 19316, 13301, 127528, 7167, 289470, 3, 33428, 12006, 30, 216, 1118, 1, 2790, 2801,
            28997, 10184, 18673, 14159, 2309, 1700, 4619, 10, 1625,
        ],
    ),
    (
        "mp_toujane",
        [
            141, 5664, 4567, 44177, 2413, 84986, 1, 40339, 5123, 11, 90, 374, 0, 690, 714, 7836,
            5844, 11432, 8986, 1248, 741, 1698, 23, 388,
        ],
    ),
    (
        "mp_trainstation",
        [
            422, 9594, 11087, 82035, 5344, 175705, 3, 55906, 18592, 58, 338, 855, 0, 3038, 3064,
            19023, 10226, 20829, 15909, 3520, 1891, 4052, 25, 1472,
        ],
    ),
];

fn counts(b: &D3dBsp) -> [usize; 24] {
    [
        b.materials.len(),
        b.planes.len(),
        b.brushes.len(),
        b.brush_sides.len(),
        b.surfaces.len(),
        b.verts.len(),
        b.lightmaps.len(),
        b.light_grid.len(),
        b.light_grid_colours.len(),
        b.cells.len(),
        b.portals.len(),
        b.aabb_trees.len(),
        b.cull_groups.len(),
        b.nodes.len(),
        b.leafs.len(),
        b.leaf_brushes.len(),
        b.coll_verts.len(),
        b.coll_edges.len(),
        b.coll_tris.len(),
        b.coll_borders.len(),
        b.coll_partitions.len(),
        b.coll_aabb_trees.len(),
        b.models.len(),
        b.visibility
            .as_ref()
            .map_or(0, |v| v.cluster_count as usize),
    ]
}

#[test]
fn stock_maps_match_the_census() {
    let Some(fs) = testing::cod2_fs() else {
        return;
    };
    for (map, want) in CENSUS {
        let data = fs
            .read(&format!("maps/mp/{map}.d3dbsp"))
            .unwrap_or_else(|| panic!("{map} not in the iwds"));
        let b = D3dBsp::parse(&data).unwrap_or_else(|e| panic!("{map}: {e:#}"));
        assert_eq!(counts(&b), want, "{map}");
        assert!(b.entities.contains("worldspawn"), "{map}");

        // Every leaf's tree: built, and on the stock maps each leaf's run
        // stays a permutation of its lump-27 list (no brush listed twice).
        let (list, trees) = b
            .leaf_brush_trees()
            .unwrap_or_else(|e| panic!("{map}: {e:#}"));
        assert_eq!(list.len(), b.leaf_brushes.len() + 1);
        for (l, t) in b.leafs.iter().zip(&trees) {
            let run =
                l.first_leaf_brush as usize..(l.first_leaf_brush + l.leaf_brush_count) as usize;
            let mut got: Vec<u16> = list[run.clone()].to_vec();
            let mut orig: Vec<u16> = b.leaf_brushes[run].iter().map(|&x| x as u16).collect();
            got.sort_unstable();
            orig.sort_unstable();
            assert_eq!(got, orig, "{map}");
            assert_eq!(t.is_some(), l.leaf_brush_count > 0);
        }
        for m in 1..b.models.len() {
            b.model_brush_tree(m)
                .unwrap_or_else(|e| panic!("{map}: {e:#}"));
        }
    }
}

/// mp_toujane's counts the census prints beyond the doc's table.
#[test]
fn toujane_extra_counts() {
    let Some(fs) = testing::cod2_fs() else {
        return;
    };
    let b = D3dBsp::parse(&fs.read("maps/mp/mp_toujane.d3dbsp").unwrap()).unwrap();
    assert_eq!(b.indices.len(), 187710);
    assert_eq!(b.portal_verts.len(), 404);
    assert_eq!(b.cull_group_indices.len(), 0);
    let vis = b.visibility.as_ref().unwrap();
    assert_eq!(vis.row_bytes, 56);
    assert!((0..vis.cluster_count as usize).all(|c| vis.sees(c, c)));
    let lm31 = b.surfaces.iter().filter(|s| s.lightmap == 31).count();
    assert_eq!(lm31, 198);
    let tri_vert_none: usize = b
        .coll_tris
        .iter()
        .map(|t| t.verts.iter().filter(|&&v| v < 0).count())
        .sum();
    let tri_edge_none: usize = b
        .coll_tris
        .iter()
        .map(|t| t.edges.iter().filter(|&&e| e < 0).count())
        .sum();
    assert_eq!((tri_vert_none, tri_edge_none), (263, 6618));
    assert_eq!(
        b.coll_aabb_trees
            .iter()
            .filter(|a| a.child_count == 0)
            .count(),
        1475
    );
    assert_eq!(
        b.leafs[0],
        vcod_cod2::d3dbsp::Leaf {
            cluster: 0,
            area: 0,
            first_coll_aabb: 0,
            coll_aabb_count: 0,
            first_leaf_brush: 0,
            leaf_brush_count: 0,
            cell: 0,
            pad7: 0,
            pad8: 0,
        }
    );
    let m0 = &b.models[0];
    assert_eq!(
        (
            m0.first_surface,
            m0.surface_count,
            m0.first_coll_aabb,
            m0.coll_aabb_count,
            m0.first_brush,
            m0.brush_count
        ),
        (0, 2413, 0, 0, 0, 4515)
    );
    assert_eq!(b.header.lumps[37].length, 71488);
}
