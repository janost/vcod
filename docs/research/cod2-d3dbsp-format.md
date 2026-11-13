# CoD2 BSP format (`.d3dbsp`, IBSP version 4)

The byte layout of Call of Duty 2 1.3's `maps/mp/*.d3dbsp`, as a diff against CoD 1's IBSP 59 (`bsp-ibsp59-format.md`), enough to write a loader from. All integers and floats are little-endian.

Sources:

- `cod2_lnxded`, the retail Linux 1.3 dedicated server: the oracle for everything the server reads. It has no renderer. Its loader functions are unnamed; the names below are the `Hunk_Alloc` tag strings each passes (`"CMod_LoadLeafs"` and so on), which match the Mac build's symbols.
- The Mac 1.3 MP binary: fully symbolized and carries the renderer, so it is the source for the render lumps. Its addresses are marked `mac`.
- The census: `tools/re/d3dbsp_census.py <map>...` over the 15 stock MP maps (`iw_03.iwd`, `iw_04.iwd`, `iw_15.iwd`): mp_breakout, mp_brecourt, mp_burgundy, mp_carentan, mp_dawnville, mp_decoy, mp_downtown, mp_farmhouse, mp_harbor, mp_leningrad, mp_matmata, mp_railyard, mp_rhine, mp_toujane, mp_trainstation. It prints `FAILURES 0` on all 15. "Census" below means that run.

vcod: `vcod_cod2::d3dbsp` parses the lumps, `vcod_cod2::collision::build` fills a `ClipMap` from the collision lumps, and the trace kernels live in `vcod_common::clipmap` (re-exported by `vcod_cod2::collision`); their module comments carry the addresses below.

## Header

```
char[4] magic        // "IBSP"
u32     version      // 4
39 x { u32 length; u32 offset }   // at offset 8; lump i's length at 8 + 8i, offset at 12 + 8i
```

VERIFIED, `Com_LoadBsp` (`cod2_lnxded` 0x08051e88, mac 0x11154e): the loader refuses a file shorter than 0x140 bytes (the 320-byte header), magic other than `IBSP` (0x50534249) or version other than 4 (`EXE_ERR_WRONG_MAP_VERSION_NUM`), and walks 0x27 = 39 directory entries. The entry order is `(length, offset)`, as in IBSP 59. VERIFIED, the same function: it stores `Com_BlockChecksum` over the whole file, and `CM_LoadMap` (0x080521a2) hands that word back to its caller as the map's checksum. VERIFIED, the renderer (`R_LoadWorldInternal`, mac 0xe4fa0) errors `LoadMap: lump extends past end of file` and `LoadMap: funny lump offset` (offset under 4) on every lump it reads, and `LoadMap: funny lump size` when a length is not a multiple of the record. The collision loaders raise `<Loader>: funny lump size` the same way.

## Lump directory

`Reader` names who loads the lump in 1.3 MP: `cm` is the collision model (the server's `CM_LoadMapFromBsp`, `cod2_lnxded` 0x08054d70, mac 0x78638, which also runs on the client), `r` the client renderer (`R_LoadWorldInternal` and the `R_Load*` it calls). `CoD1` gives the IBSP 59 lump with the same role.

| Lump | Contents | Record | Reader | CoD1 | Evidence |
|---|---|---|---|---|---|
| 0 | materials | 72 | cm, r | 0 | VERIFIED, `CMod_LoadMaterials` 0x080527e4 divides by 0x48 |
| 1 | lightmaps | 4194304 (one page) | r | 1 (786432) | VERIFIED, mac 0xe4fa0 checks `length % 0x400000` |
| 2 | light grid entries | 8 | r | none | VERIFIED, "lightgrid hash", mac 0xe4fa0 |
| 3 | light grid colours | 24 | r | none | VERIFIED, "lightgrid coefficients", mac 0xe4fa0 |
| 4 | planes | 16 | cm (r through `CM_GetPlaneNum`) | 2 | VERIFIED, `CMod_LoadPlanes` 0x08053d82 |
| 5 | brush sides | 8 | cm | 3 | VERIFIED, `CMod_LoadBrushes` 0x080536bc, `& 7` |
| 6 | brushes | 4 | cm | 4 | VERIFIED, same function, `& 3` |
| 7 | draw surfaces (triangle soups) | 16 | r | 6 | VERIFIED, `R_LoadSurfaces` mac 0xe4672 |
| 8 | draw vertices | 68 | r | 7 (44) | VERIFIED, same function, `/ 0x44` |
| 9 | draw indices | 2 | r | 8 | VERIFIED, same function |
| 10 | cull groups | 32 | r | 9 | VERIFIED, `R_LoadCullGroups` mac 0xe4522 |
| 11 | cull group indices | 4 | r | 10 | VERIFIED, mac 0xe4fa0 |
| 12-16 | unknown | | none | | VERIFIED, read by neither loader and empty in all 15 maps |
| 17 | portal vertices | 12 | r | 11 | VERIFIED, `R_LoadPortalVerts` mac 0xe4408 |
| 18 | occluders | 20 | r | 12 | VERIFIED, `R_LoadOccluders` mac 0xe3f5e |
| 19 | occluder planes | 4 | r | 13 | VERIFIED, same function |
| 20 | occluder edges | 4 | r | 14 | VERIFIED, same function |
| 21 | occluder indices | 2 | r | 15 | VERIFIED, mac 0xe4fa0 |
| 22 | AABB tree nodes (render) | 12 | r | 16 | VERIFIED, `R_LoadAabbTrees` mac 0xe3e10 |
| 23 | cells | 52 | r | 17 | VERIFIED, `R_LoadCells` mac 0xe3c1c |
| 24 | portals | 16 | r | 18 | VERIFIED, `R_LoadPortals` mac 0xe39d6 |
| 25 | BSP nodes | 36 | cm, r | 20 | VERIFIED, `CMod_LoadNodes` 0x08053558 |
| 26 | BSP leafs | 36 | cm, r | 21 | VERIFIED, `CMod_LoadLeafs` 0x08053ac0 |
| 27 | leaf brushes | 4 | cm | 22 | VERIFIED, `CMod_LoadLeafBrushes` 0x08053f38 |
| 28 | leaf surfaces | 4 | cm | 23 | VERIFIED, `CMod_LoadLeafSurfaces` 0x08054012; empty in all 15 maps |
| 29 | collision vertices | 16 | cm | 25 (12) | VERIFIED, `CMod_LoadCollisionVerts` 0x080540c6 |
| 30 | collision edges | 56 | cm | none | VERIFIED, `CMod_LoadCollisionEdges` 0x080541c4 |
| 31 | collision triangles | 72 | cm | 26 (6) | VERIFIED, `CMod_LoadCollisionTriangles` 0x08054424 |
| 32 | collision borders | 28 | cm | none | VERIFIED, `CMod_LoadCollisionBorders` 0x08054690 |
| 33 | collision partitions | 12 | cm | 24 (16) | VERIFIED, `CMod_LoadCollisionPartitions` 0x08054822 |
| 34 | collision AABB trees | 32 | cm | none | VERIFIED, `CMod_LoadCollisionAabbTrees` 0x08054952 |
| 35 | models | 48 | cm, r | 27 | VERIFIED, `CMod_LoadSubmodels` 0x0805296c |
| 36 | visibility (PVS) | 8 + rows | cm | 28 | VERIFIED, `CMod_LoadVisibility` 0x08054b3a |
| 37 | entities | text | cm, r | 29 | VERIFIED, `CMod_LoadEntityString` 0x08054ae0 |
| 38 | unknown | | none | | VERIFIED, read by neither loader; length and offset 0 in all 15 maps |

VERIFIED, census: every lump length divides by the record size above on all 15 maps, lumps 12-16, 18-21, 28 and 38 are empty on all of them, and lumps 10 and 11 are empty on nine (mp_breakout, mp_burgundy, mp_decoy, mp_downtown, mp_farmhouse, mp_leningrad, mp_matmata, mp_toujane, mp_trainstation).

What IBSP 59 had and IBSP 4 drops: the terrain partitions with patch records (CoD1 lump 24), the leaf light indices (19), the lights (30) and the light visibility cache (32). VERIFIED, no CoD2 loader reads a lump of those shapes. Patches and terrain both reach the collision model as triangles (lumps 29-34 below).

## Load order

VERIFIED, `CM_LoadMapFromBsp` 0x08054d70: materials (0), planes (4), then `CMod_LoadBrushRelated` (0x08054c2a), which loads brushes and sides (6, 5), leaf brushes (27), collision AABB trees (34), leafs (26) and models (35) and then builds each leaf's brush tree; then nodes (25), leaf surfaces (28), collision vertices, edges, triangles, borders and partitions (29-33), visibility (36) and entities (37). VERIFIED, `CM_LoadMap` 0x080521a2 calls 0x08052182, which calls `CM_LoadMapFromBsp(name, 1)` and then `CM_LoadStaticModels`. VERIFIED, the string at 0x08054d70's tail: with the second argument 0 a non-empty lump 36 is an error ("In single player, do not compile the bsp with visibility"). INFERRED from that string: the argument is the multiplayer flag, and the server always passes 1.

VERIFIED, `R_LoadWorldInternal` mac 0xe4fa0, by its "Loading %s..." lines: materials, sun settings (from the entity lump), lightmaps, lightgrid coefficients (3), lightgrid hash (2), surfaces (7, 8, 9), cull groups (10), cull group indices (11), portal vertices (17), occluders (18-20), occluder indices (21), AABB trees (22), cells (23), portals (24), nodes and leafs (25, 26), submodels (35), entities (37). The renderer reads planes through `CM_GetPlaneNum`, so the collision model is loaded first.

## Collision lumps

### Lump 0, materials (72 bytes)

```
char[64] name
u32      surface_flags     // +0x40
u32      content_flags     // +0x44
```

Same as IBSP 59. VERIFIED, `CMod_LoadMaterials` 0x080527e4 copies the records whole and byte-swaps +0x40 and +0x44 on a big-endian host; `CM_TraceThroughAabbTree` (mac 0x67ca2) copies +0x40 and +0x44 into the trace and tests +0x44 against the trace's mask. VERIFIED, "Map with no materials" errors on an empty lump. VERIFIED, census: the brush content words are not CoD 1's; the most common over the 15 maps are 0x8000001 (85925 brushes), 0x28031640, 0x2080, 0x1, 0x28030200 and 0x28001002, and the collision AABB tree leaves carry 0x1, 0x2080, 0x10, 0x2082, 0x20 and 0x2.

### Lump 4, planes (16 bytes)

`f32 normal[3]; f32 dist`, as IBSP 59. VERIFIED, `CMod_LoadPlanes` 0x08053d82: "Map with no planes" on an empty lump; the runtime plane is 20 bytes, the record plus a type byte (0, 1 or 2 when that normal component is exactly 1.0, else 3) and a sign-bits byte (bit k set when component k is negative). VERIFIED, census: every normal has unit length to 1e-3.

### Lump 6, brushes (4 bytes), lump 5, brush sides (8 bytes)

```
brush: i16 num_sides; i16 material
side:  u32 plane_or_bound; i32 material
```

Same layout as IBSP 59: no first-side field, sides stored brush after brush, the first six of each brush axial. VERIFIED, `CMod_LoadBrushes` 0x080536bc:

- The side count it allocates is `side_lump / 8 - 6 * brushes` ("bad side count" if negative), so the six axial sides are not kept as sides.
- `num_sides - 6` must not be negative ("brush has less than 6 sides").
- The axial sides come in the order x min, x max, y min, y max, z min, z max. Each one's first word is an `f32`, read into the brush's bounds (mins at runtime +0x00, maxs at +0x10), and its material into a short at runtime `+0x24 + 2 * (3 * is_max + axis)` ("axialMaterialNum exceeded" if it does not fit an `i16`).
- Every later side's first word is a plane index (runtime `planes + 20 * index`), its second a material index.
- Every material index, the brush's included, must be in `[0, materials)` ("bad materialNum: %i").
- The brush's runtime contents (+0x0c) are `materials[material].content_flags & 0xdffffffb`: the 0x4 and 0x20000000 bits are cleared.
- The brush count must fit a `u16` ("cm.numBrushes exceeded"); the loader allocates one brush more, the box brush (0x08054f0c).

VERIFIED, census: `sum(num_sides)` equals the side count on all 15 maps, every axial pair has min <= max, and every plane and material index is in range.

### Lump 25, nodes (36 bytes)

`i32 plane; i32 children[2]; i32 mins[3], maxs[3]`, as IBSP 59; a negative child `c` is leaf `-(c + 1)`. VERIFIED, `CMod_LoadNodes` 0x08053558 keeps the plane pointer and each child as an `i16` ("CMod_LoadNodes: children exceeded"); "Map has no nodes" on an empty lump. VERIFIED, `R_LoadNodesAndLeafs` mac 0xe3728 reads the same three words. VERIFIED, both loaders: the twelve bounds words are read by neither. VERIFIED, census: every plane and child in range, every node's mins <= maxs.

### Lump 26, leafs (36 bytes)

```
i32 cluster                // -1 in solid
i32 area                   // read by neither 1.3 loader
i32 first_coll_aabb        // into lump 34: the leaf's terrain and patch trees
i32 coll_aabb_count
i32 first_leaf_brush       // into lump 27
i32 leaf_brush_count
i32 cell                   // into lump 23; -1 in solid
i32 pad7, pad8             // read by neither loader
```

The shape of IBSP 59's leaf with the lump-23 partition run replaced by a run of collision AABB tree roots, and the light index run (bytes 28..36) unused. VERIFIED, `CMod_LoadLeafs` 0x08053ac0: words 2 and 3 are kept as `u16` ("firstCollAabbIndex exceeded", "collAabbCount exceeded"); word 0 is kept as an `i16` ("cluster exceeded") only on the multiplayer load, which grows the cluster count to the largest cluster plus one; "Map with no leafs" on an empty lump. VERIFIED, `CMod_LoadBrushRelated`'s leaf pass (0x08053c84) reads words 4 and 5. VERIFIED, `R_LoadNodesAndLeafs` mac 0xe3728 reads words 0 and 6.

VERIFIED, census:

- Leaf 0 is all zeros on every map and no node reaches it; the tree reaches leafs 1 up to some last leaf.
- The leafs after the last one the tree reaches are one per submodel, in model order. Each lists exactly that model's brush range (lump 35 words 10, 11) through lump 27 and its collision AABB range (words 8, 9).
- `cluster` and `cell` are -1 on exactly the same leafs.
- `area` is 0 or -1 on 14 maps and 1 on two leafs of one map; words 7 and 8 are 0 everywhere.
- Every reached leaf's brushes lie in model 0's brush range, and its AABB roots in the part of lump 34 before the first submodel's.
- Every node in lump 34 that a leaf lists is a root (no node lists it as a child).

### Lump 27, leaf brushes (4 bytes)

`i32` brush indices. VERIFIED, `CMod_LoadLeafBrushes` 0x08053f38 stores each as a `u16` ("brushIndex exceeded") with one spare slot at the end, which the box brush takes (0x08054f0c).

### Lump 28, leaf surfaces (4 bytes)

`i32`, copied as is by `CMod_LoadLeafSurfaces` 0x08054012 (VERIFIED). Empty in all 15 maps (VERIFIED, census). What reads it was not looked for.

### Lump 29, collision vertices (16 bytes)

```
u32 unused                 // 0 in every record of the 15 maps
f32 xyz[3]
```

VERIFIED, `CMod_LoadCollisionVerts` 0x080540c6 keeps bytes 4..16 as a 12-byte vertex and drops the first word. VERIFIED, census: the first word is 0 everywhere.

### Lump 31, collision triangles (72 bytes)

```
f32 plane[4]               // normal, dist
f32 svec[4]                // s = dot(p, svec.xyz) - svec.w
f32 tvec[4]                // t = dot(p, tvec.xyz) - tvec.w
i32 verts[3]               // into lump 29; -1 = none
i32 edges[3]               // into lump 30; -1 = none
```

VERIFIED, `CMod_LoadCollisionTriangles` 0x08054424 copies all 72 bytes (the runtime record is the file record). VERIFIED, census on every triangle whose three vertices are given: each vertex lies on the plane to 1e-3, and `(s, t)` reads (0, 0) at vertex 0, (1, 0) at vertex 1 and (0, 1) at vertex 2, to 1e-3. So `s` and `t` are barycentric weights of vertices 1 and 2. VERIFIED, census: edge `k` joins the two vertices other than `k`, in either direction. VERIFIED, `CM_TraceCapsuleThroughTriangle` (`cod2_lnxded` 0x08056490, mac 0x66aa8): it tests `edges[k]` and `verts[k]` only when the index is not negative, and each against a per-trace stamp array, so an edge or vertex shared by several triangles is tested once a trace. INFERRED, from that: -1 marks an edge or vertex another triangle owns or one that cannot be met; census counts 263 vertex and 6618 edge slots of -1 on mp_toujane's 8986 triangles. VERIFIED, the point arm of `CM_TraceThroughAabbTree_r` (mac 0x6754e): a triangle is hit only when the start is in front of the plane and the end behind it, and then only for `s >= -0.001`, `t >= -0.001`, `s + t <= 1.001` (mac `.literal4` 0x326228, 0x32622c). INFERRED from that test: a triangle is one-sided, as a CoD 1 patch facet is.

### Lump 30, collision edges (56 bytes)

```
u32 unused                 // 0 in every record of the 15 maps
f32 origin[3]              // one end of the edge
f32 axis[2][3]             // two unit vectors perpendicular to the edge and to each other
f32 dir[3]                 // unit vector along the edge, from origin
f32 length
```

VERIFIED, `CMod_LoadCollisionEdges` 0x080541c4 keeps bytes 4..52 as a 48-byte runtime edge and scales the last three floats by `1 / length` (the reciprocal passed to the scale helper 0x080552e2 as an `f32`), so the runtime edge holds `dir / length`. VERIFIED, census: `axis[0]`, `axis[1]` and `dir` are unit and mutually perpendicular to 1e-4 on every sampled edge, and `origin + dir * length` lands on the triangle vertex at the edge's other end. VERIFIED, `CM_TraceCapsuleThroughTriangle` mac 0x66aa8: the capsule meets the edge in the plane of the two axes and accepts the contact when `|dot(p - origin, dir / length) - 0.5| <= 0.5` (mac 0x326190 is 0.5), i.e. inside the segment.

### Lump 32, collision borders (28 bytes)

```
f32 dist_eq[3]             // a, b, c: a*x + b*y = c, (a, b) unit
f32 z_base
f32 z_slope
f32 start
f32 length
```

VERIFIED, `CMod_LoadCollisionBorders` 0x08054690 copies all 28 bytes. VERIFIED, census: `(a, b)` has unit length and `length > 0` on all 29246 borders. VERIFIED, the capsule arm of `CM_TraceThroughAabbTree_r` (mac 0x6754e) walks a partition's borders only when the trace moves in x or y and the trace word at +0x90 is non-zero: it takes the trace's xy against `(a, b, c)`, measures the position along the border from `start` and accepts it up to `length`, and accepts a height within that +0x90 word of `z_base + z_slope * along`. INFERRED from those reads: the +0x90 word is the capsule's half height, and a border is a vertical wall along the outer edge of a terrain partition, which keeps a sideways capsule from slipping under the edge of the triangles.

### Lump 33, collision partitions (12 bytes)

```
u16 unused                 // 0 in every record of the 15 maps
u8  tri_count
u8  border_count
i32 first_tri              // into lump 31
i32 first_border           // into lump 32
```

VERIFIED, `CMod_LoadCollisionPartitions` 0x08054822 keeps bytes 2 and 3 and turns the two indices into pointers; bytes 0 and 1 are dropped. VERIFIED, census: every partition has at least one triangle, at most 96 triangles and 24 borders on these maps, the partitions' triangle runs and border runs each tile their lump in order with no gap or overlap, and every triangle and border belongs to exactly one partition. CoD 1's partition carried a patch or a terrain mesh; this one carries triangles either way, and there is no patch record.

### Lump 34, collision AABB trees (32 bytes)

```
f32 origin[3]              // box centre
f32 half_size[3]
u16 material               // into lump 0: the triangles' surface and contents
u16 child_count            // 0 for a leaf
i32 child_or_partition     // child_count > 0: first child (into lump 34); 0: partition (into lump 33)
```

VERIFIED, `CMod_LoadCollisionAabbTrees` 0x08054952 copies the record field by field. VERIFIED, `CM_CullBox` mac 0x664b0 tests the trace against `origin ± half_size` widened by the trace's extents, and `CM_TraceThroughAabbTree_r` mac 0x6754e recurses into `child_count` consecutive nodes from `child_or_partition`, or with no children clips the partition `child_or_partition`. VERIFIED, `CM_TraceThroughAabbTree` mac 0x67ca2: a tree whose material's contents miss the trace mask is skipped, and a hit takes that material's surface and content flags. VERIFIED, mac 0x6754e: a partition already clipped in this trace (a per-trace `u16` stamp per partition) is not clipped again.

VERIFIED, census: every child run lies after its parent and inside the lump, every leaf's partition is in range, on every fifth node, a leaf's box covers its triangles' vertices to 0.004 and a parent's box holds its children's to the same margin. Partitions are shared: the 15 maps hold 8244 partitions that more than one tree leaf names.

### Lump 35, models (48 bytes)

```
f32 mins[3], maxs[3]
i32 first_surface, surface_count       // into lump 7
i32 first_coll_aabb, coll_aabb_count   // into lump 34
i32 first_brush, brush_count           // into lump 6
```

IBSP 59's model, with the lump-23 partition run replaced by a collision AABB run. VERIFIED, `CMod_LoadSubmodels` 0x0805296c: "Map with no models" on an empty lump and "MAX_SUBMODELS exceeded" at 0x400 or more; the runtime bounds are `mins - 1` and `maxs + 1`; for every model but 0, words 8 and 9 are kept as `u16`. VERIFIED, `CMod_LoadSubmodelBrushNodes` 0x08053404: a submodel's brush list is `first_brush .. first_brush + brush_count` taken straight from this record, not through a leaf. VERIFIED, mac 0xe4fa0's submodel pass reads `mins`, `maxs` and the surface run (a count of 0 stores first surface -1).

VERIFIED, census: the brush runs and the surface runs of the models tile lumps 6 and 7 in model order. Model 0 owns no collision AABB range (both words 0); the world reaches lump 34 only through its leafs. No submodel on the 15 maps owns a collision AABB tree; on five maps 2 or 6 submodels own surfaces, on the other ten none.

### Lump 36, visibility (8 bytes + rows)

`i32 cluster_count; i32 row_bytes;` then `cluster_count` rows, as IBSP 59. VERIFIED, `CMod_LoadVisibility` 0x08054b3a: an empty lump makes an all-visible row of `(clusters + 31) & ~31` bytes from the leafs' cluster count; otherwise it copies the rows. VERIFIED, census: all 15 maps carry one, `cluster_count` equals the leafs' largest cluster plus one, `row_bytes` is `ceil(cluster_count / 64) * 8`, the length is `8 + cluster_count * row_bytes`, and every cluster sees itself.

### Lump 37, entities

ASCII `{ "key" "value" }` blocks, NUL-terminated. VERIFIED, `CMod_LoadEntityString` 0x08054ae0 copies the whole lump; census: the last byte is 0 on all 15 maps. VERIFIED, mac `CM_LoadStaticModels` 0x77724: the collision model takes static models from the `misc_model` entities (`model`, `origin`, `angles`, `modelscale`, `modelscale_vec`). VERIFIED, mac `R_ParseSunLight` 0xe22e4 reads the sun from the same lump's keys (`sunlight`, `suncolor`, `sundiffusecolor`, `sundirection`, `diffuseFraction`, `ambient`, `_color`), and mac `R_LoadEntities` 0xe2b6c the static models' lighting keys (`gndLt`, `directLightScale`, `indirectLightScale`).

## What the trace reads

The order in which a sweep meets the brushes and triangles decides ties (`bsp-ibsp59-format.md`, "The leaf walk"), so it is part of the format a faithful collision model has to rebuild.

VERIFIED, `CM_TraceThroughTree` mac 0x63f90: a node splits the segment like Q3's (an axial plane offsets by the trace's extent plus 0.125, a slanted one by 2048 for a box and 0.125 for a point; mac 0x32625c, 0x3261c8) and a leaf goes in this order: when the leaf's brush contents meet the mask, its brush tree (`CM_TraceThroughLeafBrushNode` mac 0x63ae2), returning if that left the fraction at 0; then, when its AABB contents meet the mask and the fraction is not 0, each of its `coll_aabb_count` roots in lump order (`CM_TraceThroughAabbTree`), stopping once the fraction reaches 0.

The leaf's runtime record (0x2c bytes) is built at load. VERIFIED, `CMod_LoadBrushRelated`'s leaf pass 0x08053c84 and its helper 0x080533a2: brush contents are the OR of its brushes' masked contents and AABB contents the OR of its roots' materials' content flags; the brush box is the union of the brushes' bounds widened by 0.125 (0x080531d8, rodata 0x0813f86c). VERIFIED, `CM_TraceThroughLeafBrushNode` mac 0x63ae2 culls the whole leaf against that box before walking its brushes.

The leaf's brushes are not walked in lump-27 order. VERIFIED, `CMod_PartionLeafBrushes_r` 0x08052d16 (mac 0x77f0a) and its split scorer 0x08052bb2 build a k-d tree over the leaf's brush list, rewriting the list in place:

- Each candidate split is a brush's min or max on an axis, tried for axes x, y, z in turn and, within an axis, brush by brush in list order, min before max.
- A candidate counts the brushes wholly on its high side (`min >= d`) and wholly on its low side (`max <= d`), each count starting at -1, and moves `d` to the middle of the gap, `0.5 * (lowest high-side min + highest low-side max)` (rodata 0x0813f7d4).
- Its score is `min(counts) * min(high-side min - node mins[axis], node maxs[axis] - low-side max)`, 0 when either count is under 1.
- A candidate replaces the best only with a strictly higher score. With no positive score the node is a leaf that holds the brushes in list order.
- Otherwise the brushes that straddle `d` (`min < d < max`) go first into a subtree of their own, then the high side's, then the low side's, each in list order.

The runtime node is 20 bytes: axis byte, `i16` brush count, contents, then either the brush pointer or the split `d` and its clearance, and two `u16` child offsets counted in nodes from this one. VERIFIED, `CM_TraceThroughLeafBrushNode_r` mac 0x62fe0: a positive count clips that many brushes in order; a count of -1 (set on a node that has straddlers, VERIFIED at 0x08052d16) walks the next node first, which is the straddlers' subtree; then the segment is split at `d` against the clearance plus the trace's extent and 0.125, near side first. INFERRED, from that the straddlers' subtree is the next allocation in the temp pool: the walker's `node + 1` is that subtree's root. A submodel's brushes get the same tree (0x08053404).

VERIFIED, 0x08052d16: the high-side and low-side passes are separate tests (`d <= min`, `max <= d`), so a brush flat on the split (`min == max == d`) is written on both sides and the rewrite runs past the leaf's run into the next one's; the scorer's `else` counts such a brush on the high side only. VERIFIED, `crates/cod2/tests/d3dbsp_maps.rs` running the port (`vcod_cod2::d3dbsp::partition_brushes`) over every leaf of the 15 maps: no leaf's rewrite leaves its run, so each run stays a permutation of its lump-27 list.

## Contents and surface flags

VERIFIED, the material compiler's `surfaceparm` table in the Mac 1.3 MP binary (`.data` 0x348820, 20-byte rows `{name, clears SOLID, surface bits, content bits, tool bits}`): the content bits are

| Bit | `surfaceparm` | | Bit | `surfaceparm` |
|---|---|---|---|---|
| 0x1 | (SOLID, cleared by every row marked "clears") | | 0x1000 | `ai_nosight` |
| 0x2 | `foliage` | | 0x2000 | `clipshot` |
| 0x10 | `glass` | | 0x10000 | `playerclip` |
| 0x20 | `water` | | 0x20000 | `monsterclip` |
| 0x40 | `canshootclip` | | 0x1000000 | `mantleOn`, `mantleOver` |
| 0x80 | `clipmissile` | | 0x8000000 | `detail` |
| 0x200 | `vehicleclip` | | 0x10000000 | `structural` |
| 0x400 | `itemclip` | | 0x80000000 | `nodrop` |
| 0x800 | `sky` | | | |

and the surface bits

| Bit | `surfaceparm` | | Bit | `surfaceparm` |
|---|---|---|---|---|
| 0x1 | `nodamage` | | 0x4000 | `nonsolid` |
| 0x2 | `slick` | | 0x10000 | `alphashadow` |
| 0x4 | `sky` | | 0x20000 | `nodlight` |
| 0x8 | `ladder` | | 0x40000 | `nocastshadow` |
| 0x10 | `noimpact` | | bits 20-24 | sound material: `bark` 1 through `asphalt` 0x16, `opaqueglass` reusing `glass`'s 9 |
| 0x20 | `nomarks` | | 0x2000000 | `mantleOn` |
| 0x80 | `nodraw` | | 0x4000000 | `mantleOver` |
| 0x400 | `nolightmap` | | 0x80000000 | `portal` |
| 0x800 | `pointlight` | | | |
| 0x2000 | `nosteps` | | | |

INFERRED from the table: 0x4 and 0x20000000, the two bits `CMod_LoadBrushes` clears off a brush (0x080536bc, "Lump 6"), come from no `surfaceparm` here; three of the census's most common brush words carry 0x20000000 (0x28031640, 0x28030200, 0x28001002; VERIFIED, census).

The masks the server traces with, each VERIFIED as the immediate its caller passes (Mac 1.3 names):

| Mask | Bits | Caller |
|---|---|---|
| 0x2810011 | SOLID, GLASS, PLAYERCLIP, 0x800000, 0x2000000 (BODY) | `ClientThink_real`'s `pm->tracemask` for `pm_type` under 6; `GScr_PlaceSpawnPoint`; `PM_UpdateLean`; `turret_think_client` |
| 0x810011 | the same without BODY | `ClientThink_real` for a dead player |
| 0x2802831 | SOLID, GLASS, WATER, SKY, CLIPSHOT, 0x800000, BODY | `Bullet_Fire_Extended`, `Melee_Trace` (`G_LocationalTrace`) |
| 0x802891 | SOLID, GLASS, CLIPMISSILE, SKY, CLIPSHOT, 0x800000 | `CanDamage`'s blast probes |
| 0x2802801 | SOLID, SKY, CLIPSHOT, 0x800000, BODY | `Player_UpdateLookAtEntity` |

INFERRED, from the masks against the census: a `clipshot` trim (0x2080 after the mask) stops a bullet and not a player, as CoD 1's kerbs do, and BODY is a player's own contents. No material in the table sets 0x800000.

`GScr_PlaceSpawnPoint` (Mac 0x19f4b8), VERIFIED: a player capsule (`playerMins`, `playerMaxs`, mask 0x2810011) is swept 128 up (rodata 0x326384) from the spawn's origin, then 262144 down (0x32641c) from where that stopped, the origin is set to the landing, and a position test there prints `WARNING: Spawn point entity %i is in solid` when it comes back `allsolid`. INFERRED from that order: a spawn whose box starts inside a brush it can rise out of lands on top of that brush. The three are `G_TraceCapsule` (0x0810a528), `SV_Trace` without static models, so an entity in the way (mp_toujane's script_model panzer) is landed on.

## The trace

`cod2_lnxded`'s trace functions, matched to the Mac names by their callers, sizes and constants (Mac addresses after each). VERIFIED for each the structure described. The arithmetic is the Linux build's x87 code: every named float variable stored, every expression in between extended. `vcod_common::clipmap` ports them with the same addresses in its comments.

- `CM_Trace` 0x0805b414 (mac 0x64426): the box is shifted to its centre, `(mins + maxs) * 0.5` (rodata 0x8140270), the start and end each moved by it; the half size goes to `size`, the capsule's radius is the smaller of `size.z` and `size.x` and its sphere offset `size.z - radius`. `invDelta` is `1 / (start - end)` per axis, 0 where they meet (0x0805d86c). The trace's bounds are the swept span grown by the radius in x and y and by radius plus offset in z, and a sweep whose bounds' volume is under 16 times `size.x * size.y * size.z * |delta|` (rodata 0x814026c) culls the AABB trees by their axes alone (0x0805b39c). `start == end` (0x0805d78c) is a position test; otherwise a trace with a zero box is a point. Model 0 walks the tree from node 0 with the start at fraction 0 and the end at the trace's fraction.
- `CM_TraceThroughTree` 0x0805b058 (mac 0x63f90), "What the trace reads" above. Front when the smaller distance is at least the offset, back when the larger is at most minus it; both are the engine's `fsel` min and max (0x0805d5f2, 0x0805d5ce), which take the difference as a float. A crossing returns once the trace's fraction is no further than the segment's start; a segment within 4.7684e-7 (rodata 0x8140268) of parallel walks the front whole.
- The leaf, 0x0805a6ae: nothing once the fraction is 0; its brush tree when the brush contents meet the mask, returning when that left the fraction at 0; then its AABB roots in lump order while the fraction is not 0.
- `CM_TraceThroughLeafBrushNode` 0x0805a5a8 (mac 0x63ae2) culls the leaf's brush box grown by `size` through `CM_TraceBox` 0x0805d8e0, a slab test of the segment that also refuses a box met no nearer than the trace's fraction. `CM_TraceThroughLeafBrushNode_r` 0x0805a1e0 (mac 0x62fe0) splits on `0.125 + size[axis] - clearance` (rodata 0x8140250): a segment wholly in the gap returns, the straddlers' subtree (`count` -1) is walked before the split.
- `CM_TraceThroughBrush` 0x08059dc0 (inlined on the Mac): the six axial sides first, mins then maxs, x, y, z, each against the capsule's box (`radius, radius, radius + offset`); then every other side, its distance pushed out by `radius + |normal.z * offset|`. Enter starts at 0 and leave at the trace's fraction; a side the start is in front of by less than 0.125 that the end does not cross ends the brush (`min(d1, 0.125) <= d2`). The lead side is the latest entering one, or the first met while none is; with no lead the trace is `startsolid`, and `allsolid` at fraction 0 when no side was left. A hit writes the fraction, the side's normal (an axial side's is the unit axis), the side material's surface flags and the brush's contents. An axial side's material is the brush's `axialMaterialNum`.
- `CM_TraceThroughAabbTree` 0x08055e78 (mac 0x67ca2) and `_r` 0x08055af0 (mac 0x6754e): `CM_CullBox` 0x08055404 (mac 0x664b0) tests the sweep's midpoint against the node's box grown by `size` and the half delta on the three axes, then, unless the sweep culls by axes alone, on the three cross axes of the half delta. A leaf's partition is clipped once a trace (a stamp per partition). A point trace clips each triangle with 0x0805631a; a capsule clips each with `CM_TraceCapsuleThroughTriangle` 0x08056490 at the sphere offset below the centre, and again at the offset above when the triangle's normal points down; then, when the sweep moves in x or y and the offset is not 0, the partition's borders with 0x08057046 (`CM_TraceCapsuleThroughBorder`, inlined on the Mac).
- 0x0805631a, the point triangle: the end behind the plane and the start in front, fraction `max((d1 - 0.125) / (d1 - d2), 0)` if nearer than the trace's, the crossing point `d1 / (d1 - d2)` along the segment tested at `s, t >= -0.001`, `s + t <= 1.001`.
- 0x08056490, the capsule triangle, with `reach = 0.125 + radius` (rodata 0x8140008): nothing unless the end sphere is within `reach` in front and the sweep closes on the plane by more than the float 1e-4 (0x814000c); that closing distance `d1 - d2` is stored (0x0805654d) and is the divisor of the face fraction below. A start more than `reach` behind is `startsolid` at fraction 0 only if the other sphere's centre is within `reach` in front of the face inside the triangle. Otherwise the contact point (the start when it is within `reach`) is tested against the triangle: inside, the fraction is `(d1 - reach) / (d1 - d2)`, the divisor the stored float (0x08056782), (0 within reach) and the normal the face's, `startsolid` when the start is nearer than the radius. Outside, each corner region is tested: vertex `k` where the outcode bit `k` (bit 0 `s + t > 1`, bit 1 `s < 0`, bit 2 `t < 0`) is clear, edge `k` where it is set, each once a trace (stamps per vertex and per edge). A vertex is a sphere of radius `reach`, an edge a cylinder in its two axes accepted at `|along - 0.5| <= 0.5`; a start inside either is fraction 0, `startsolid` when within the radius. A vertex root takes `disc >= 0` and an edge root `disc > 0`; every root here and in the border is 0x080575a0, libm's `sqrt` of the widened float stored back to a float, and a vertex's inside normal multiplies by `1 / sqrt(c0)` stored. `Vec3Normalize` 0x080a4424 (an edge's inside normal) stores the squared length, its root (0x080aaebe, the same helper) and the reciprocal, then multiplies each component once. A contact normal under 0.7 up (0x8140014) on a face at least that steep, while the sweep falls, takes the face's normal.
- 0x08057046, the border: met only by a sweep closing on it in xy, at `(reach - dist) / approach`, accepted when no more than `reach` behind along the sweep; past either end of the border the sweep meets the end's vertical line as a circle of radius `reach`, and the contact height must be within the sphere offset of `z_base + along * z_slope`. The normal is `(a, b, 0)`.
- The position test, 0x08059c4c: the leaves the box grown by 1 touches, from `CM_BoxLeafnums_r` 0x08058cc2 (front child first, `BoxOnPlaneSide` 0x080aa17e with the axial fast path, at most 0x400 leaves), until one makes the trace `allsolid`. A leaf (0x08059878) tests its brushes when its brush box overlaps the trace's bounds strictly (0x080597ce) and, unless they held the capsule, its AABB roots (`CM_MeshTestInLeaf` 0x08055f50), whose first hit writes its material's flags. `CM_TestInLeafBrushNode_r` 0x08059636 descends the high side when the split is under the bounds' min, both when it is within them. `CM_TestBoxInBrush` 0x080594f6 is the capsule's box against the brush bounds, touching included, and its pushed-out spheres against the other sides. `CM_PositionTestInAabbTree_r` 0x08055d4c and `CM_PositionTestCapsuleInTriangle` 0x080555b6 are the sweep's tests with the sphere nearer the face held still, the edge and vertex tests at the plain radius.
- The dot product helpers 0x0805d758 and 0x080562b2 sum `x*x' + y*y'` and then add `z*z'`, extended.
- `CM_PointContents` 0x08058f8c (mac 0x69190) walks to the point's leaf (front on `d >= 0`, 0x08058b90), needs the point strictly inside the leaf's brush box and ORs the contents of every brush of its tree that holds it (0x08058ddc, high side when the point is past the split). Triangles add nothing.

VERIFIED, `CMod_LoadBrushRelated` 0x08054c2a: the brush trees' pool is addressed from one node before its first allocation, so tree node indices start at 1 and a leaf with no brushes keeps 0, which `CM_PointContents` reads as none.

VERIFIED, 0x08055af0 and 0x08055d4c: the partition stamp is a `u16` compared with the trace count sign-extended from 16 bits, so while the count's low half is 0x8000 or more the stamp never matches and a partition shared by several tree leaves is clipped again; once the count wraps, a stale stamp can match and skip one. INFERRED: the second clip of a partition finds nothing nearer, so only the wrap can change a trace; `vcod_common::clipmap` (`stamp_partition`) stamps with the full count.

VERIFIED, `crates/cod2/tests/collision_maps.rs` against the port: point traces from the spawns agree with a clip over every model-0 brush and every triangle under a leaf's trees to 1e-4 on all 15 maps (800 segments each). Every stock spawn, placed by `GScr_PlaceSpawnPoint`'s three traces, is not `startsolid` for the standing player. Before placement 12 of the 930 spawns, on 6 maps, start solid where the map puts them: mp_carentan 1 (an `mp_dm_spawn` at (0, 696, 160) whose box reaches 14 units into a 26-sided `clip_nosight` brush), mp_harbor 1, mp_railyard 2, mp_rhine 4, mp_toujane 2 (one, at (536, 1585, 39), under a plaster block from z 100 that the placement then lands it on top of), mp_trainstation 2. VERIFIED, retail (`crates/cod2/tests/trace_ab.rs`, "The trace A/B" below): every stock spawn of the 15 maps, 1902 placements counting each gametype's class, lands where the port puts it to the bit, and retail warns `in solid` about none.

## The trace A/B

`crates/cod2/tests/trace_ab.rs` replays `tests/fixtures/traces/<map>.txt` through `ClipMap::trace` and diffs bit for bit. Each fixture is what `cod2_lnxded` 1.3 returned to a gsc gametype written by `cargo run -p vcod-cod2 --example trace_probe -- <map>`. On mp_toujane and mp_carentan the fixture holds `bulletTrace(start, end, false, undefined)` and `physicsTrace(start, end)` over 1729 and 1688 segments (the c4 doorway and window rays, the window swept every 2 units up its height, a 24 by 24 grid of vertical rays over the spawns' area, axial and slanted rays grazing brush box edges at 0, 1/64, 1/8 and 1/8 + 1/64 off, rays down onto collision triangles' middles, corners and edges and just outside an edge, glancing rays along triangles, rays from spawn eyes), `placeSpawnPoint()` on 258 and 240 script_origins around the spawns, and on each spawn entity. On the other 13 maps (`--spawns-only`) it is `placeSpawnPoint()` on each spawn entity of every gametype class. A run boots the server in an `unshare -rn` namespace with the probe installed as a loose gametype (`maps/mp/gametypes/<name>.gsc` and its `.txt`, `sv_pure 0`), reads the `logPrint` lines from `games_mp.log` and pairs each console `WARNING: Spawn point entity %i is in solid` with its placement by entity number and rounded origin. A map's run takes about 6 s.

What the probe rests on:
- VERIFIED, `Scr_BulletTrace` 0x08113818: it returns `fraction`, `position` (`Vec3Lerp` 0x080e129e, `a + (b - a) * f`), `entity`, `normal` and `surfacetype` (`Com_SurfaceTypeToName` 0x080b948a over the 20-byte rows at 0x08182dec, index `surface_flags >> 20 & 0x1f`, `default` for 0 and past 22, `none` at fraction 1, where the normal is the segment's direction). `hitchars` false clears BODY off the shot mask, leaving 0x802831.
- VERIFIED, `G_LocationalTrace` 0x0810a5cc and `SV_Trace` 0x0809d884: the bullet trace is `SV_Trace` with `locational` set, which clips the static models (0x0805ec2c) after the world and before the entities.
- VERIFIED, `Scr_PhysicsTrace` 0x08113c0e: a point `G_TraceCapsule` under 0x820011 (SOLID, GLASS, MONSTERCLIP, 0x800000) that returns the lerped end only; `G_TraceCapsule` 0x0810a528 passes `locational` 0, so no static models.
- VERIFIED, 0x08079e74: gsc turns a float into a string with `%g` (`SL_GetStringForVector` 0x08079f00 a vector with `(%g, %g, %g)`).
- VERIFIED, live probe: gsc's `==` on floats has a tolerance, `0.00000001 == 0` true and `0.000001 == 0` false. The probe therefore prints each float's IEEE bits: scaled by 2^100 to read the sign, halved or doubled into [2^23, 2^24), all exact, and `int()` of the mantissa; a zero of either sign prints `0`.

VERIFIED, the gate, records matching bit for bit (fraction, normal, surface type and position for a `bulletTrace`, the end for a `physicsTrace`, the placed origin and the warning for a placement), before and after the fixes below:

| Map | Records | Matching before | Matching after | Skipped |
|---|---|---|---|---|
| mp_toujane | 3844 | 3777 | 3805 | 8 entity, 30 static model, 1 gap |
| mp_carentan | 3744 | 3689 | 3724 | 2 entity, 18 static model |
| 13 other maps | 1646 placements | 1646 | 1646 | |

VERIFIED, the same gate: before the fixes every divergence was a placement landing a few float steps off retail (mp_toujane's `mp_ctf_spawn_axis` 0 at z 70.55673 against 70.55674), and every `bulletTrace` and `physicsTrace` the world decides already matched. Retail warns `in solid` about 126 of the 498 script_origin placements and none of the 1902 spawn placements; the port agrees on each.

The fixes, each VERIFIED in the disassembly:
- 0x0805654d, 0x08056782: the capsule face fraction divides by the stored closing distance, which is compared against the float 1e-4; 62 placements.
- 0x080575a0, 0x080a4424: every capsule root is a float `sqrt`, and `Vec3Normalize` stores its length and reciprocal; mp_toujane's `mp_ctf_spawn_axis` 0, a 262144-unit drop onto a terrain vertex.
- 0x0805d758: the dot helpers sum x, y, then z, where the port summed z first; no record here turned on it.

What the gate skips, and counts, because the clip map does not model it:
- An entity: a `bulletTrace` that returned one, and the `physicsTrace` of the same segment (script_model vehicles on both maps).
- A static model: a `bulletTrace` that stopped nearer than the clip map with its hit inside the bounding sphere of a `misc_model` whose xmodel has collision surfaces, at its `modelscale`. 48 records; VERIFIED, the gate: the `physicsTrace` of each of those segments matches.
- `GAPS`: mp_toujane placement `near` 105, landed on the script_model panzer at (1047, 2751, 44). It starts at (1080.6719, 2643.8594, 131.125) and retail lands it at z 99.75969, on the hull. VERIFIED, the fixture's bits. The entity is the bombzone's intact `script_model` `xmodel/vehicle_panzer_ii_static` at angles (0.666642, 270, -0.0750181), one of three at that origin (its destroyed twin and an `xmodel/fx`). VERIFIED, the map's entity lump. `G_TraceCapsule` clips it through the xmodel's collision surfaces; the port has only the world's clip map, and vcod-server clips a script model's collision for a ray only (`builtins::combat::bullet_hit`), so closing the record needs a capsule trace against an xmodel's `CollSurf`s posed at the entity's angles. INFERRED, the trace's entity pass; which of the model's surfaces the capsule meets was not read.

## Render lumps

### Lump 1, lightmaps (4 MiB pages)

VERIFIED, mac 0xe4fa0: the page count is `length / 0x400000`, and a non-empty lump whose page count is not the surfaces' largest lightmap index plus one is a `funny lump size`. VERIFIED, the same function, by the sizes it copies: a page is four images back to back,

| Offset | Image | Size | Bytes per texel | Format passed to `Image_Generate2D` |
|---|---|---|---|---|
| 0x000000 | 0 | 512 x 512 | 4 | 0x15 |
| 0x100000 | 1 | 512 x 512 | 4 | 0x15 |
| 0x200000 | 2 | 512 x 512 | 4 | 0x15 |
| 0x300000 | 3 | 1024 x 1024 | 1 | 0x32 |

each allocated as `*lightmap%i_%i` (page, image). The formats are the D3D numbers of A8R8G8B8 (0x15) and L8 (0x32). VERIFIED, census: all four channels of images 0-2 hold varied values on mp_toujane. VERIFIED, the stock shader `materials/shaders/lib/lightmap.hlsl` (`iw_07.iwd`) samples `lightmapSamplerR`, `lightmapSamplerG` and `lightmapSamplerB` at the lightmap UV, dots each with a weight vector looked up by the normal map, and multiplies sunlight by `lightmapSamplerSun`. VERIFIED, the fixed-function path at mac 0xe4fa0 (`r_rendererInUse` 2) builds one RGBA image per page from byte 0 of images 0, 1 and 2 plus the sun colour times image 3 sampled at every second texel and row, `floor((c + sun * v) * 0.5 + 0.5)`. INFERRED, from those two: images 0, 1 and 2 are the red, green and blue lightmap coefficients (four basis weights each) and image 3 the sun visibility at twice the resolution. Which basis each channel holds was not worked out.

VERIFIED, mac 0xe4fa0, the same fixed-function loop: the output texel's byte 0 is 0xff and bytes 1, 2, 3 take images 0, 1, 2 in that order, each read at the texel's own byte 0, and the sun image is read at `(2x, 2y)` of its 1024-wide rows. The three sun factors are the sun light's colour at `GfxLight` +0x14 (DAT_0130d5e8..f0). VERIFIED, `R_InterpretSunLightParseParamsIntoLights` mac 0xe25de: that colour is `(sunlight - ambient) * (1 - diffuseFraction) * suncolor`, `suncolor` scaled to a largest component of 1 by `ColorNormalize` (mac 0x2c7a6). VERIFIED, `R_ParseSunLight` mac 0xe22e4: the keys compare without case (`I_stricmp`; stock maps spell `diffusefraction`), `diffuseFraction` defaults to 0.5 (0x3f000000) and the others to 0, and an `ambient` over 2.0 (mac 0x3261e4) is rescaled by 4/255 (0x326424). `vcod_cod2::world::lightmap_rgb` builds the CoD1 renderer's RGB pages by this path.

VERIFIED, mac 0xe4fa0: the loader may pack several pages into one larger texture ("%i merged lightmaps from %i original lightmaps", `r_optimizeLightmaps`) and rescales each surface's lightmap UV by its page's scale and offset in `R_LoadSurfaces`; this is a load-time choice, not file data.

### Lump 2, light grid entries (8 bytes), lump 3, light grid colours (24 bytes)

```
entry:  u32 key            // (x >> 2) << 21 | (y >> 2) << 10 | (z >> 2)
        u8  low            // (x & 3) << 6 | (y & 3) << 4 | (z & 3) << 2 | flag (bit 0)
        u8  occlusion_mask // bit c: test corner c of the query cell with a sight trace
        u16 colour         // into lump 3
colour: u8  rgb[3][8]      // channel-major, scaled by 1/255
```

The lookup, `RB_GetLightingAtPoint` (mac 0xeb5e4), takes a point and returns 24 floats and a scalar:

- VERIFIED: the grid's cells are 32 units in x and y and 64 in z with corner `(x, y, z)` at `(32x - 131072, 32y - 131072, 64z - 131072)` (mac 0x326240 = 131072, 0x326430 = 1/32, 0x326348 = 1/64). The cell index on each axis is `(int)(p + 131072) >> 5` (z: `>> 6`) and the fraction `(p + 131072) / 32` minus that index.
- VERIFIED: the query visits the cell's eight corners `c` = 0..7, corner `c` at `(x + (c & 1), y + (c >> 1 & 1), z + (c >> 2 & 1))`, finds each by binary search on `(key, low & 0xfe)`, and weights it by the product of the three axis fractions (`1 - f` on the low side, `f` on the high side): trilinear.
- VERIFIED: a found corner whose `occlusion_mask` has bit `c` set is used only when `CM_BoxSightTrace` (a point, mask 0x2001) from the query point to the corner moved 0.01 (mac 0x3262f0) toward the point along their unit difference is clear.
- VERIFIED: each used corner adds its 24 colour bytes times its weight times 1/255 (mac 0x326184) into the 24 floats, in record order, and its weight into a total; a corner with bit 0 of `low` set also adds its weight into the scalar.
- VERIFIED: a total of 0.98 (mac 0x3262c8) or more is returned as summed. A smaller non-zero total divides the 24 floats and the scalar by it. With a total of 0 the nearest corner (each axis rounded up past a fraction of 0.5) is searched and its record taken at weight 1, the scalar 1 or 0 by its bit 0; a missed search reads the slot the search ended on (INFERRED, the index the loop leaves). With an empty grid, or under `r_showMissingLightGrid`, the result is a fixed table: value `j` of channel `k` is bit `k` of `j`, scalar 0.

What the values mean:

- The 24 floats are three channels of eight, `R0..R7, G0..G7, B0..B7`. VERIFIED, `RB_SetupLighting` mac 0xf6b86: they are copied in order from `GfxEntityLighting` +8 to `backEnd` +0x120..+0x17c, and the sun's colour with the scalar as alpha goes through `RB_SetCodeConstant(0x85, ...)`, constant 133, `lightColor0`. INFERRED, from the path without an entity writing the same sun colour straight to `backEnd` +0x50: code constant `k` lives at `backEnd + 16 (k - 128)`, so +0x120 is constant 146, and the floats fill `lightGridColorsR0` (values 0-3 of red), `R1` (4-7), `G0`, `G1`, `B0`, `B1` (146..151, cod2-materials.md 5). Value `j` is the light arriving along corner `j` of the probe basis' cube, bit 0 of `j` its x, bit 1 its y, bit 2 its z (cod2-materials.md 9.2).
- The scalar is the sun's visibility: the share of the weight on corners whose bit 0 of `low` is set, so bit 0 marks a grid point that sees the sun (INFERRED, from its use as `lightColor0.a`, which scales the sun's diffuse in `lib/lightprobe.hlsl`).
- An entity's sample point is its lighting origin when `renderFxFlags` has 0x80, else its origin 4 (mac 0x3261c0) higher (VERIFIED, `RB_SetupEntityLighting` mac 0xec126, `GfxEntity` fields from STABS: `renderFxFlags` +4, `lighting` +8, `origin` +0x3c). `CG_Player` (mac 0x1e6834) and `CG_Corpse` (0x1e78ea) set 0x80 with the lighting origin at `lerpOrigin` raised by `fTorsoHeight` plus 12 when `eFlags` has 0x8, 20 when it has 0x4, else 32 (mac 0x326354, 0x32624c, 0x3263e8) (VERIFIED, the stores; the `centity_s` and `entityState_s` offsets from STABS). `CG_General` (mac 0x164354) sets it only under `eFlags` 0x400, to the first `lerpOrigin` it saw, kept in `centity_s.lightingOrigin` (VERIFIED).
- A static model samples its bounds' centre (VERIFIED, `R_GetStaticModelLightingFromGrid` mac 0x1069cc averages the instance's +0x14 and +0x20 vectors; that they are its world bounds is INFERRED from the pairing).

VERIFIED, census over the 15 stock maps: every colour index is in range, entries are sorted ascending by `(key, low & 0xfe)`, bit 1 of `low` is always 0, and runs of identical consecutive entries are common (1449 to 3122 per map). INFERRED: a duplicate is harmless, since the search takes any match. VERIFIED, census: bit 0 of `low` is set on 25% (mp_downtown) to 95% (mp_brecourt) of entries and `occlusion_mask` is non-zero on 5% (mp_brecourt) to 39% (mp_harbor). Averaged over a map's colour records, value 0 (straight down) is the darkest and value 7 (straight up) the brightest of the eight on 13 maps; on mp_brecourt and mp_decoy value 7 is the darkest.

VERIFIED, census (`crates/cod2/tests/lightgrid_maps.rs`, with the sight traces through the clip map): at every spawn's standing lighting origin on all 15 maps the lookup returns values in [0, 1], and the mean sun visibility of spawns under open sky beats that of roofed spawns on every map but mp_downtown (0.10 against 0.14).

### Lump 7, draw surfaces (16 bytes)

```
i16 material               // into lump 0
i16 lightmap               // page into lump 1; 31 = none
i32 first_vertex           // into lump 8
u16 vertex_count
u16 index_count
i32 first_index            // into lump 9
```

IBSP 59's soup with the no-lightmap value 31 instead of 65535. VERIFIED, `R_LoadSurfaces` mac 0xe4672: "no surfaces" on an empty lump, the fields at the offsets above, indices copied from `first_index` and offset by `first_vertex`. VERIFIED, mac 0xe4fa0: lightmap 31 (0x1f) is skipped when the pages are counted and is given the identity UV transform. VERIFIED, mac 0xe4672: a map may have one sky material only ("map has at least two different skies"). VERIFIED, census: every material, vertex run and index run in range, every index under its surface's `vertex_count`, `index_count` a multiple of 3, and the largest lightmap other than 31 plus one equals the page count on all 15 maps.

VERIFIED, census, as IBSP 59 has it: the surfaces of model 0 are covered exactly once by the cull groups' runs at the front of the lump and the render AABB trees' leaf runs after them, and each cell's AABB tree, walked in preorder, ends where the next cell's begins.

### Lump 8, draw vertices (68 bytes)

```
f32 xyz[3]                 // +0x00
f32 normal[3]              // +0x0c
u8  color[4]               // +0x18
f32 uv[2]                  // +0x1c
f32 lm_uv[2]               // +0x24
f32 tangent[3]             // +0x2c
f32 binormal[3]            // +0x38
```

VERIFIED, `R_LoadSurfaces` mac 0xe4672 copies the fields at these offsets into the 68-byte runtime vertex (the tangent to +0x38 and the binormal to +0x2c) and packs the colour as `b0 << 24 | b1 << 16 | b2 << 8 | b3` for the colour converter. INFERRED from that packing: the bytes are R, G, B, A. VERIFIED, census on mp_toujane: the vector at +0x2c follows dP/du (dot over 0.9 on 3719 of 3824 sampled triangles) and the one at +0x38 dP/dv (3775 of 3824); both are unit and perpendicular to the normal on every sampled vertex of all 15 maps; the handedness `cross(n, tangent) · binormal` is positive and negative about equally often. VERIFIED, census on mp_toujane: every sampled triangle winds clockwise seen from its vertex normal, as IBSP 59's do.

### Lump 9, draw indices

Flat `u16`, relative to the surface's `first_vertex`, as IBSP 59 (VERIFIED, mac 0xe4672 adds `first_vertex`).

### Lumps 10-11, 17-24, cells, portals, cull groups, occluders, render AABB trees

The same records as IBSP 59's lumps 9-18 (`bsp-ibsp59-format.md`, "Visibility and tree lumps"), renumbered:

| CoD2 | CoD1 | Layout | Evidence |
|---|---|---|---|
| 10 | 9 | `f32 mins[3], maxs[3]; i32 first_surface, surface_count` | VERIFIED, `R_LoadCullGroups` mac 0xe4522 (a count of 0 stores first surface -1) |
| 11 | 10 | `i32` cull group index | VERIFIED, mac 0xe4fa0 |
| 17 | 11 | `f32 xyz[3]` | VERIFIED, `R_LoadPortalVerts` mac 0xe4408 |
| 18 | 12 | `u32 first_plane; u16 num_planes, num_edges; u32 first_edge, first_vert; u16 vert_count, pad` | VERIFIED, `R_LoadOccluders` mac 0xe3f5e |
| 19 | 13 | `u32` plane index into lump 4 | VERIFIED, same function |
| 20 | 14 | `u8 plane0, plane1, vert0, vert1` | VERIFIED, same function |
| 21 | 15 | `i16` occluder index | VERIFIED, mac 0xe4fa0 |
| 22 | 16 | `i32 first_surface, surface_count, child_count` | VERIFIED, `R_LoadAabbTrees` mac 0xe3e10 |
| 23 | 17 | `f32 mins[3], maxs[3]; i32 first_aabb_tree, first_portal, portal_count, first_cull_group_index, cull_group_count, first_occluder, occluder_count` | VERIFIED, `R_LoadCells` mac 0xe3c1c |
| 24 | 18 | `i32 plane, cell, first_vert, vert_count` | VERIFIED, `R_LoadPortals` mac 0xe39d6 |

VERIFIED, mac 0xe39d6: a portal's runtime plane is `{n, -dist}` and its vertex count is stored in a byte. VERIFIED, census: every portal's vertices lie on its plane to 1e-2, the plane faces out of the cell that lists the portal (its centroid moved 8 units back lies in that cell's bounds and 8 units forward in the neighbour's, all 1882 portals), portals have 3 to 8 vertices, and each cell's portal run begins where the previous one's ends. The occluder lumps 18-21 are empty in all 15 maps (VERIFIED, census).

The renderer walks the cells through these portals with a nearest-first queue (`R_VisitPortals`, mac 0xf14c0). VERIFIED (decompile). cod2-client-perf.md 9 has the walk and vcod's port of it.

### Surface bounds and the render AABB trees

`R_LoadSurfaces` (mac 0xe4672) bounds each surface over the vertices its
indices reach: per index, `_AddPointToBounds` of vertex `first_vertex +
index`. VERIFIED. A surface's vertex run is not its own: 3785 of
mp_carentan's 3929 surfaces span a run more than 1500 units across, so a
bound over the run culls nothing. VERIFIED, census. `vis::soup_bounds`
takes the indexed bound for both games; on the 45 CoD1 1.1 maps it equals
the run's bound for every soup. VERIFIED, census.

`R_FinishLoadingAabbTrees_r` (mac 0xe21f6) places a node's children as one
contiguous block starting at the running index, their subtrees after the
block, and bounds a childless node over its surfaces' bounds and any other
node over its children's. VERIFIED. `R_AddAabbTreeSurfaces_r` (mac
0xef1ec) adds a node's own surface range only when the node is a leaf or
lies wholly inside the frustum. VERIFIED. vcod's walk reads the run as
preorder (`vis::walk_bounds`); it covers the same node range and each
node's bounds hold every node it skips, so it culls conservatively, not
as tightly as retail's tree. INFERRED.

## Census

VERIFIED, census, record counts per map:

| map | materials | planes | brushes | sides | surfaces | verts | lightmaps | grid | grid colours | cells | portals | aabb trees | cull groups | nodes | leafs | leaf brushes | coll verts | edges | tris | borders | partitions | coll aabbs | models | clusters |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| mp_breakout | 304 | 13822 | 10425 | 95559 | 5983 | 243087 | 3 | 49239 | 13659 | 10 | 92 | 941 | 0 | 993 | 1029 | 15487 | 9653 | 19206 | 16887 | 2902 | 1723 | 3291 | 35 | 477 |
| mp_brecourt | 121 | 11638 | 3429 | 37416 | 1230 | 56146 | 2 | 138664 | 5086 | 10 | 46 | 176 | 17 | 2198 | 2280 | 6303 | 8023 | 14095 | 13570 | 196 | 2156 | 3465 | 81 | 766 |
| mp_burgundy | 235 | 6903 | 3469 | 31978 | 2110 | 81385 | 3 | 29591 | 7193 | 18 | 122 | 322 | 0 | 620 | 628 | 6474 | 4643 | 9678 | 7997 | 1211 | 719 | 1662 | 7 | 306 |
| mp_carentan | 260 | 9561 | 8072 | 63875 | 3929 | 177543 | 2 | 41305 | 14067 | 28 | 272 | 625 | 26 | 2087 | 2097 | 18470 | 8657 | 18216 | 13283 | 2521 | 1101 | 2915 | 9 | 1129 |
| mp_dawnville | 230 | 8771 | 6951 | 55451 | 3531 | 147884 | 2 | 30710 | 9652 | 27 | 196 | 545 | 5 | 1018 | 1054 | 12326 | 7854 | 15329 | 11517 | 1170 | 1139 | 2646 | 35 | 455 |
| mp_decoy | 80 | 5400 | 1335 | 16716 | 618 | 32536 | 1 | 37871 | 7687 | 2 | 4 | 90 | 0 | 639 | 683 | 2351 | 6173 | 10983 | 11835 | 421 | 1656 | 2621 | 43 | 276 |
| mp_downtown | 140 | 6425 | 9863 | 72142 | 4410 | 208080 | 3 | 53120 | 9805 | 4 | 12 | 663 | 0 | 514 | 527 | 11909 | 3886 | 6500 | 6287 | 106 | 829 | 1289 | 12 | 204 |
| mp_farmhouse | 314 | 12929 | 6070 | 64647 | 2278 | 101987 | 2 | 66231 | 13518 | 25 | 156 | 314 | 0 | 1588 | 1607 | 9236 | 5627 | 10635 | 9559 | 1233 | 1110 | 2507 | 18 | 649 |
| mp_harbor | 248 | 10204 | 8267 | 69533 | 5748 | 231659 | 4 | 26128 | 7083 | 18 | 154 | 892 | 40 | 3861 | 3870 | 18687 | 9781 | 20615 | 14659 | 2263 | 1100 | 3072 | 8 | 1983 |
| mp_leningrad | 203 | 9125 | 9818 | 79608 | 3377 | 197303 | 3 | 32623 | 9516 | 4 | 16 | 502 | 0 | 332 | 345 | 11851 | 5578 | 9551 | 8566 | 251 | 1196 | 1651 | 12 | 150 |
| mp_matmata | 204 | 11713 | 8898 | 79208 | 5095 | 181322 | 3 | 47599 | 11250 | 7 | 56 | 798 | 0 | 748 | 773 | 13213 | 18522 | 40185 | 26366 | 8620 | 1519 | 2398 | 24 | 326 |
| mp_railyard | 188 | 7746 | 11496 | 83572 | 4283 | 207891 | 3 | 54795 | 21367 | 23 | 112 | 679 | 22 | 1246 | 1259 | 16406 | 5692 | 11213 | 8629 | 1275 | 1001 | 1880 | 12 | 544 |
| mp_rhine | 253 | 19316 | 13301 | 127528 | 7167 | 289470 | 3 | 33428 | 12006 | 30 | 216 | 1118 | 1 | 2790 | 2801 | 28997 | 10184 | 18673 | 14159 | 2309 | 1700 | 4619 | 10 | 1625 |
| mp_toujane | 141 | 5664 | 4567 | 44177 | 2413 | 84986 | 1 | 40339 | 5123 | 11 | 90 | 374 | 0 | 690 | 714 | 7836 | 5844 | 11432 | 8986 | 1248 | 741 | 1698 | 23 | 388 |
| mp_trainstation | 422 | 9594 | 11087 | 82035 | 5344 | 175705 | 3 | 55906 | 18592 | 58 | 338 | 855 | 0 | 3038 | 3064 | 19023 | 10226 | 20829 | 15909 | 3520 | 1891 | 4052 | 25 | 1472 |

## Open questions

- Lumps 12-16 and 38 are read by no 1.3 MP loader and empty in the MP maps. Whether the single-player maps fill them, and with what, was not checked.
- Which basis each of the four lightmap coefficients holds, and which channel order the A8R8G8B8 images take in memory on the PC build.
- The leaf's `area` word: read by neither loader here, so whether anything in 1.3 uses areas for the snapshot cull (CoD 1's does) is open.
- What reads lump 28 at runtime, if anything.
- Which bit 0x800000 is: every player and shot mask carries it and no `surfaceparm` sets it.
- The trace against retail is closed: `trace_ab` compares the port with `cod2_lnxded` record for record ("The trace A/B"). Entities and static models stay outside the clip map and the gate skips them.
