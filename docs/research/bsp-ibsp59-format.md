# CoD 1 BSP format (IBSP version 59)

Byte layouts of the map lumps vcod reads, verified against the retail `maps/mp/mp_pavlov.bsp` and cross-checked against the CoD-BSP-Decompiler project (CoDEmanX/Daevius). The parser is `crates/common/src/bsp.rs`; `crates/common/src/collision.rs` turns the collision lumps into clip planes. Where this document and the parser disagree, the parser is right; the census numbers below are pinned by its tests (`parses_mp_pavlov`, `parses_collision_lumps_in_mp_pavlov`). Everything here is VERIFIED unless labelled otherwise. All integers are little-endian.

## Header and lump directory

```
char[4] magic           // "IBSP"
u32     version         // 59 (CoD 1 and UO)
33 x { u32 length; u32 offset }   // lump directory at offset 8, 264 bytes
```

The directory entry is `(length, offset)`, the opposite of Q3's `(offset, length)`. The header is 272 bytes; the parser rejects anything shorter, any other magic, and any other version.

## Lumps vcod reads

| Lump | Contents | Record size | mp_pavlov count |
|---|---|---|---|
| 0 | materials | 72 | 191 |
| 1 | lightmaps | 786432 (one 512x512x3 RGB page) | 11 |
| 2 | planes | 16 | 9132 (all unit normals) |
| 3 | brushsides | 8 | 56966 |
| 4 | brushes | 4 | 7609 |
| 6 | triangle soups | 16 | 2625 |
| 7 | draw verts | 44 | 94886 |
| 8 | draw indices | 2 | 183840 |
| 9 | cull groups | 32 | 122 |
| 10 | cull group indices | 4 | 125 |
| 11 | portal vertices | 12 | 568 |
| 16 | AABB tree nodes | 12 | 335 |
| 17 | cells | 52 | 20 |
| 18 | portals | 16 | 124 |
| 20 | BSP nodes | 36 | 1752 |
| 21 | BSP leafs | 36 | 1788 |
| 27 | models | 48 | 35 |
| 29 | entities | text | one block per entity |

Lump 27 bytes 32..40 are each model's run in lump 23 ("The leaf walk", submodel partitions). The remaining lumps are identified below under "The other lumps"; the visibility set (cells, portals, AABB trees, cull groups, nodes, leafs, PVS) is decoded in "Visibility and tree lumps".

## Record layouts

### Lump 0, materials (72 bytes)

```
char[64] name           // NUL-padded
u32      surface_flags
u32      content_flags
```

A few materials spell the path with backslashes (mp_carride's curtains); the parser normalizes them to `/` so lookups match pk3 entry names. Material name to image file: append `.dds`, `.tga`, `.jpg` in that order and look each up in the pk3 file system, e.g. `textures/normandy/walls/brick@damagedwall1_p4` resolves to `textures/normandy/walls/brick@damagedwall1_p4.dds`. `surface_flags` is parsed but unused; the renderer skips surfaces by material name. `content_flags` drives collision (below). `materials[0]` on mp_pavlov is `textures/common/clip_nosight_metal`.

### Lump 1, lightmaps

Consecutive 512x512 RGB pages, 3 bytes per texel, no header. The lump length is a multiple of 786432.

### Lump 6, triangle soups (16 bytes)

```
u16 material            // index into lump 0
u16 lightmap            // page index into lump 1; 65535 = no lightmap
u32 first_vertex        // into lump 7
u16 vertex_count
u16 index_count         // multiple of 3
u32 first_index         // into lump 8
```

Draw indices are relative to the soup's `first_vertex`, not absolute; the parser test checks every index stays under its soup's `vertex_count`. `lightmap == 65535` (`NO_LIGHTMAP`) marks a vertex-lit surface whose vertex color carries the lighting. Winding: for every triangle on mp_pavlov, `cross(p1 - p0, p2 - p0)` points opposite the stored vertex normal, i.e. front faces wind clockwise seen from the normal side; the renderer's back-face culling relies on this.

### Lump 7, draw verts (44 bytes)

```
f32 pos[3]
f32 uv[2]
f32 lm_uv[2]
f32 normal[3]
u8  rgba[4]
```

### Lump 8, draw indices

Flat `u16` array; see the soup record for the relative-index rule.

### Lump 2, planes (16 bytes)

```
f32 normal[3]
f32 dist
```

Every plane in mp_pavlov has a unit normal (checked to 1e-3).

### Lump 3, brushsides (8 bytes)

```
u32 plane_or_dist       // sides 0..6 of a brush: an f32 bound, bit-cast; sides 6..: plane index into lump 2
u32 material            // index into lump 0: the side's own material
```

The first six sides of every brush are axial and carry an f32 bound coordinate instead of a plane index, in the order 0 = xmin, 1 = xmax, 2 = ymin, 3 = ymax, 4 = zmin, 5 = zmax. Converted to clip planes (`n . p <= d` inside): even side `k` with bound `b` on axis `a = k / 2` gives `normal = -axis_a, dist = -b`; odd side `k` gives `normal = +axis_a, dist = +b`. Sides 6 and up index lump 2; every index on mp_pavlov is in range and every axial pair satisfies `min <= max`. The parser keeps the raw u32 and `collision.rs` decodes it.

A side's `material` is its own, and it differs from the brush's (lump 4) on structural brushes: mp_cassino brush 7930 is `textures/common/caulk` (surface flags 0x400a0, no sound surface) with its +y side `textures/italy/walls/plaster@wall_misc-06` (0x1000000, plaster) and its -x and -y sides concrete (VERIFIED, lumps 0, 3 and 4 of UO's `mp_cassino.bsp`). The brush's material classifies the brush (contents); a trace that stops on a side reports that side's material's surface flags: the brush clip's epilogue (`cod_lnxded` 0x8055565) stores `materials[leadside.material].surfaceFlags` (`+0x40` of the 72-byte record) and the material pointer into the trace (VERIFIED decompile). An axial side's lead carries the short at the brush's `+0x28 + 2 * side` as its material (INFERRED, off the data flow into that store). VERIFIED live: a UO tank shell striking that +y face explodes with `surfType` 16 (`*-uo-tank.bin`, 26150).

UO's `coduo_lnxded` (0x8058f0a's tail) and `CoDMP.exe` (0x4230c0's) store the side's flags the same way (VERIFIED decompile). VERIFIED, live (retail 1.1d, mp_carentan, 2026-10-15, `client-probes/probe_surfside` with a `--save-slope --probe-moves` client): walking the floor brush at x 232..552, y 1096..1408, whose material is `rock@cobbl_patrn1a` (17) and whose top side is wood (21), raises run footstep 22 (wood), never 18. VERIFIED, live (UO 1.51b, the satchel capture, coduo-diff.md 21): the satchel's bounces read 17 off a `concrete@concrete-02` (5) brush's cobblestone top side. VERIFIED (lump census): on mp_cassino (UO) 6358 of 8811 brushes have a side whose sound material differs from the brush's; on the 1.1 maps from 913 of 2363 (mp_brecourt) to 7120 of 7609 (mp_pavlov). VERIFIED (the same census): no side differs from its brush in the ladder bit on any of the thirteen maps counted (mp_cassino and twelve 1.1 maps, carentan to rocket).

### Lump 4, brushes (4 bytes)

```
u16 num_sides           // >= 6
u16 material            // index into lump 0; its content_flags classify the brush
```

There is no first-side field. Sides are stored sequentially: brush N's sides start where brush N-1's ended, and `sum(num_sides)` equals the brushside count exactly (the parser refuses the file otherwise). The parser synthesizes `first_side` while reading.

### Lump 27, models (48 bytes)

```
f32 mins[3], maxs[3]
u32 first_soup, num_soups       // into lump 6
u32 first_partition, num_partitions  // bytes 32..40, into lump 23
u32 first_brush, num_brushes    // into lump 4
```

The six counts are 32-bit; whether they are signed does not matter for any shipped file, and the parser reads them as u32. Model 0 is the world; models 1.. are the inline brushmodels an entity references as `"model" "*N"` (doors, gates, script_brushmodels, trigger volumes). The soup and brush ranges partition their lumps: model 0 takes the leading run and each submodel the slice after it. On mp_pavlov model 0 owns brushes `[0, 7575)` of 7609 and each of the other 34 models owns one brush; all 32 `textures/common/trigger` brushes and 2 stray clip brushes are submodel brushes. Colliding only against model 0's brush range therefore excludes triggers and door-style submodels by construction, which is what `CollisionWorld::build` does.

Submodel vertices are stored relative to the brushmodel's own origin brush, and the entity carries that origin: on mp_stanjel, `*58` is a beam whose entity sits at `(-1704, 472, 873)` while every vertex of model 58 lies within its own bounds and under 128 units of zero. A brushmodel built without an origin brush has origin zero, so its local space is map space. mp_harbor is the smallest stock map whose submodels carry drawable surfaces: the world owns soups `[0, 1492)` and three door brushmodels own the remaining six, two each; its submodel 1 is a map-wide `trigger_hurt` with brushes and no surfaces.

### Lump 29, entities

ASCII text of `{ "key" "value" ... }` blocks, trailing NULs stripped. Coordinates are Z-up, one unit is about one inch. `"angles" "pitch yaw roll"` is in degrees, yaw about Z. Spawn classnames the viewer looks for, in order: `mp_deathmatch_spawn`, `mp_teamdeathmatch_spawn`, `mp_deathmatch_intermission`, `info_player_start`. Spawn origins sit about 10 to 20 units above the floor; the player origin is at the feet (bbox mins z = 0).

## The other lumps

Census over the 14 stock MP maps (`maps/MP/*.bsp` in `pak[0-9].pk3`; note the upper-case `MP`): record size is the gcd of the lump lengths across maps, the role is VERIFIED by decoding unless marked. Counts are mp_pavlov's.

| Lump | Record | Role | mp_pavlov |
|---|---|---|---|
| 5 | | empty in every map | 0 |
| 9 | 32 | cull groups: `f32 mins[3], maxs[3]; i32 first_soup, soup_count` | 122 |
| 10 | 4 | cull group indices (`i32`), referenced by cells | 125 |
| 11 | 12 | portal/occluder vertices (`f32 xyz`), referenced by portals and occluders | 568 |
| 12 | 20 | occluder headers: `u32 first_plane; u16 num_planes, num_edges; u32 first_edge, first_vert; u16 vert_count, pad` | 9 |
| 13 | 4 | occluder plane indices (`u32` into lump 2) | 54 |
| 14 | 4 | occluder edges (`u8 plane0, plane1, vert0, vert1`, all relative to one occluder) | 108 |
| 15 | 2 | per-cell occluder index lists (`u16` into lump 12) | 11 |
| 16 | 12 | AABB tree nodes: `i32 first_soup, soup_count, child_count` | 335 |
| 17 | 52 | cells | 20 |
| 18 | 16 | portals: `i32 plane, cell, first_vert, vert_count` | 124 |
| 19 | 2 | leaf light indices (`i16` into lump 30; a negative first entry marks a leaf that sees the sky), see `cod11-light-grid-and-leaf-lights.md` | 230 |
| 20 | 36 | BSP nodes | 1752 |
| 21 | 36 | BSP leafs | 1788 |
| 22 | 4 | leaf brushes (`i32` into lump 4) | 10714 |
| 23 | 4 | leaf collision surfaces (`i32` into lump 24), see below | 1418 |
| 24 | 16 | terrain collision partitions (`u16 x 8`, layout not decoded) | 619 |
| 25 | 12 | terrain collision vertices (`f32 xyz`) | 6291 |
| 26 | 6 | terrain collision triangles (`u16 x 3` into lump 25); mp_ship has none | 4496 |
| 28 | 8 (+rows) | PVS: `i32 cluster_count, row_bytes` then `cluster_count` bit rows; mp_neuville has none | 666 x 88 |
| 30 | 72 | lights: type, colour, origin, direction, falloff and cone words; `R_LoadLights` (0x4db620) rejects a length not divisible by 72, see `cod11-light-grid-and-leaf-lights.md` | 22 |
| 31 | | empty in every map | 0 |
| 32 | 3145728 | light visibility cache: 8192 hash buckets x 32 slots x 12 bytes, read by the static-model and entity lighting (same doc) | 1 |

Lump 23 is not a list of draw surfaces: on mp_brecourt its indices reach 701 with only 548 soups, and on mp_bocage it references exactly the 626 records of lump 24. Draw-surface visibility never goes through leafs.

## Visibility and tree lumps

The soups of lump 6 are laid out as `[cull-group soups][cell AABB-tree soups][submodel soups]`, and the two front ranges partition model 0 exactly (mp_bocage 377 + 802 = 1179, mp_pavlov 437 + 2188 = 2625, mp_harbor 6 + 1486 + 6 submodel soups = 1498). Every soup of the world is drawn through one of the two structures below; the PVS is not needed to find draw surfaces.

### Lump 17, cells (52 bytes)

```
f32 mins[3], maxs[3]
i32 first_aabb_tree        // into lump 16; the trees of consecutive cells are consecutive
i32 first_portal           // into lump 18
i32 portal_count
i32 first_cull_group_index // into lump 10
i32 cull_group_count
i32 first_occluder         // into lump 15; the per-cell occluder index list
i32 occluder_count
```

VERIFIED by chaining: `first_portal + portal_count` of each cell equals the next cell's `first_portal` and ends at the portal count; the same for cull group indices and for the occluder lists (lump 15's 11 entries are exactly cell 0's 9 plus one each for cells 3 and 12 on mp_pavlov); and walking each cell's AABB tree in preorder from `first_aabb_tree` ends exactly at the next cell's tree (all maps but mp_rocket, where one tree ends early). Cell 0 on every map is the large outdoor cell; mp_pavlov's cell 0 holds 44 of the 124 portals, 114 of the 125 cull group references and all 9 occluders (cells 3 and 12 re-share occluder 0).

### Lumps 12-15, occluders

Decoded from the `CoDMP.exe` 1.1 loaders (`FUN_004dcd00` @ `0x4dcd00` reads lumps 12/13/14; `FUN_004dca00` @ `0x4dca00` the vertices; `FUN_004dcf90` @ `0x4dcf90` the index lists) and the census below. The earlier CoD2-derived guesses (24-byte lump 13, 48-byte lump 14) were wrong; the real consumers want 4-byte plane and edge records.

- **Lump 12**, occluder headers (20 bytes, one per occluder): `u32 first_plane; u16 num_planes; u16 num_edges; u32 first_edge; u32 first_vert; u16 vert_count; u16 pad`. The `first_*` fields index lumps 13, 14 and 11 respectively. mp_pavlov: 9 records, plane/edge sums 54 / 108, matching lumps 13 and 14. The 8-vert slabs all index low lump-11 offsets.
- **Lump 13**, `u32` indices into lump 2 (the brush planes). Resolved at load into runtime planes `{n, d - eps}` with precomputed box-sign bytes and a per-frame front/back flag.
- **Lump 14**, edges, 4 bytes `{u8 plane0, plane1, vert0, vert1}`: the plane indices are relative to one occluder's own plane slice (adjacency). The vertex bytes are the occluder's absolute lump-11 slot **taken modulo 256** - on mp_depot, whose occluders sit past vertex 256 in the pool, the byte reads `first_vert + local - 256`; recover `local ≡ B - first_vert (mod 256)`, unique because `vert_count` stays far below 256.
- **Lump 15**, `u16` per-cell lists of occluder indices, packed contiguously so a cell's `[first_occluder, first_occluder + occluder_count)` is its list. Real lists can repeat an occluder across cells.

The runtime occluder is 36 bytes: `{planes, num_planes, num_edges, edges, vert_count, verts}` with 20-byte planes, 16-byte edge records (`plane0*, plane1*, vert0*, vert1*`) and 12-byte vertex records (`f32 xyz`).

### Lump 16, AABB tree nodes (12 bytes)

```
i32 first_soup, soup_count // into lump 6; the node's own surfaces
i32 child_count            // children follow in preorder
```

Nodes carry no bounds; the engine computes them from the surfaces at load (INFERRED from the record, as in CoD2). A node with `child_count == 0` is a leaf. Leaves cover each cell-tree soup exactly once. The root node of a cell spans the cell's whole soup range.

### Lump 9, cull groups (32 bytes), lump 10, indices

A cull group is a bounding box over a contiguous soup range at the front of lump 6, shared between cells through lump 10 (mp_pavlov: 122 groups, 125 references). Every soup of a group lies inside the group's bounds (checked with a 1-unit margin on every map). Ranges are monotone and disjoint; on mp_powcamp and mp_rocket they are not sorted by cell.

### Lump 18, portals (16 bytes), lump 11, portal vertices (12 bytes)

```
i32 plane                  // into lump 2; every vertex satisfies n·v = d to 1e-3
i32 cell                   // the cell on the far side
i32 first_vert, vert_count // into lump 11, 4 vertices per portal on the stock maps
```

Portals are stored once per side: a cell's `portal_count` portals each name the neighbouring cell.

The plane faces out of the owning cell: for every one of the 2474 stock portals, the polygon centroid moved 8 units against the normal lies inside the owning cell's bounds and moved along it inside the neighbour's (VERIFIED).

### Lump 20, nodes, and lump 21, leafs (36 bytes each)

```
node: i32 plane; i32 children[2]; i32 mins[3], maxs[3]   // child < 0 is leaf -(child+1)
leaf: i32 cluster; i32 area; i32 first_leaf_surface, leaf_surface_count;
      i32 first_leaf_brush, leaf_brush_count; i32 cell;
      i32 first_light_index, light_index_count
```

VERIFIED: every plane, child, brush and lump-24 reference is in range; `cluster` runs to `cluster_count - 1` of lump 28 (665 on mp_pavlov), `cell` to the cell count minus one, -1 for solid leafs. The tree reaches leafs 0..1753 on mp_pavlov; the 34 leafs after them belong to the 34 submodels, as in Q3. `area` is -1 exactly where `cluster` is -1, and -1 or 0 on every stock 1.1 MP map; UO's mp_berlin, mp_ponyri, mp_rhinevalley and mp_uo_stanjel and 1.5's mp_bocage hold a few leafs in areas 1 to 7, as do several 1.1 single-player maps (VERIFIED, lump 21 of every stock map). The snapshot cull reads it (`docs/protocol-1.1.md`, "Which entities a client is sent").

### Lump 28, PVS

`i32 cluster_count; i32 row_bytes;` then `cluster_count` rows of `row_bytes` bytes, bit `c` of row `r` set when cluster `c` is visible from `r` (Q3 `visData`). mp_pavlov: 666 clusters, 88 bytes per row, lump length `8 + 666 * 88` exactly. mp_neuville ships without a PVS (lump length 0), so a consumer needs a fallback. The client renderer never reads it (below); it is the server's snapshot visibility.

### How the retail client draws the world

From the Ghidra decompilation of `CoDMP.exe` 1.1 (image base `0x00400000`); VERIFIED by reading the decompiled functions, inferred where marked. The cvar table sits at `0x4e2be0`'s neighbourhood: `r_showportals`, `r_showaabbtrees`, `r_portalbevels`, `r_lockpvs`, `r_noportals`, `r_singlecell`, `r_cullXModels`, `r_showCullSModels`.

- Camera cell, `0x4e2be0`: walks the BSP from the root, `dot(vieworg, plane) - dist <= eps` takes the back child, and returns the leaf's `cell`. A negative cell is legal (camera outside every cell). Its caller `0x4e4c10` then skips the portal walk entirely and frustum-tests every cell's AABB tree (`0x4e4080`) and every cull group (`0x4e4120`) instead, so outside the map the world is frustum-culled only. `r_singlecell` visits only the camera cell.
- Portal recursion, `0x4e44d0`: per visited cell, build the cell's occluder volumes (`0x4e2860`), walk its AABB tree, mark its cull groups and static models (`0x4e4440`), then for each portal skip it if it is already on the recursion stack, if the eye is behind its plane, if its polygon is outside the current frustum (`0x4e2c60`), or if it lies fully inside an occluder volume (`0x4e2cc0`). Otherwise `0x4e2ee0` clips the portal polygon against the near and fog planes and every plane of the current frustum (Sutherland-Hodgman, `0x4e2d20`, 1024-point cap) and `0x4e19b0` builds the child frustum: one plane per surviving edge through the eye, plus four screen-space bevel planes when `r_portalbevels > 0` (its retail default is `0.7`, so bevels are on). An eye within eps of the portal plane inherits the parent frustum unchanged. There is no depth limit; the stack flag and an empty clipped polygon end the recursion. `r_noportals` only affects mirror surfaces (`0x4d4c30`), not this walk.

Occluder volumes, `0x4e2860`: for each occluder of the cell (its runtime list is the cell's +0x2c/0x30 slice of the lump-15 lists), any vertex in front of the eye plane makes it a candidate; each of its box planes is flagged front/back against the eye position, the front-facing planes are kept as volume planes (their `d` already pulled in by eps at load), and every silhouette edge whose two planes straddle the eye contributes one plane through the eye and the edge's two vertices (`d = eye·n - eps`). The volume planes pool per frame (cap `0x1800`), grouped per occluder (per-occluder plane count at `+0x1c`, active volume cap `0x400`). A portal is hidden when every one of its vertices is on the negative side of every plane of one volume (`0x4e2cc0`): all `n·v <= d`, tested vertex by vertex, volume by volume. The clip-and-frustum path then runs only on portals that survived both frustum and occluder tests.

Bevel planes, `0x4e19b0`: with `r_portalbevels > 0`, the clipped portal polygon's vertices are projected into screen space, the min/max `s`/`t` are clamped to `[-1, 1]`, and four planes are built through the eye, each through one edge of the screen-space box of the projection (normals derived from the projection matrix rows `_DAT_011cc1d8..214`). They hug the portal's silhouette, so a frustum that would sweep wide because the polygon is partially clipped by the near plane cannot pull in cells beside the portal.
- AABB tree, `0x4e3210`: node bounds are tested against the frustum with the plane sign trick (near corner first, then far corner); a node fully inside is marked without further tests, otherwise the leaf pass `0x4e3070` tests each surface's own bounds against the frustum and the occluder volumes before marking it. So the runtime node carries bounds (`mins` at +0, `maxs` at +0xc) and every surface carries bounds; both are computed at load, since the lump has neither (the load-time builder was not located).
- Cull groups, `0x4e4120`: the group's bounds only; every surface in the group is marked without a per-surface test.
- Static models (`misc_model`) hang off a per-cell linked list walked in `0x4e4440` and tested by bounds (`0x4e42b0`, `r_showCullSModels`). Entities (xmodels) are box-walked through the BSP each frame (`0x4e39d0`) and linked into a per-cell reference list (`0x4e3940`, "Max xmodel refs (%i) exceeded", 4096), re-tested when the cell is visited (`0x4e3560`); `r_cullXModels` and a size threshold gate that path. Which lump feeds the static-model list was not located; INFERRED to be the entity lump at load.
- The PVS is not consulted: `r_lockpvs` only freezes the frustum setup in `0x4e4610`, and no code reads lump 28's bit rows.
- Runtime portals hold their vertices as plain `f32 xyz` arrays, matching lump 11's 12-byte records.

## Content flags

From the materials lump, per brush via `brush.material`:

| Flag | Meaning |
|---|---|
| `0x1` | solid |
| `0x4` | seen on terrain (snow) materials, which have no brushes |
| `0x10` | glass: `glass@brokenwindow` 0x2090 and 0x8000010, `dam_window` and the shop windows 0x28000010, `workshopwindow` 0x28030010, and no other brush material (census over the stock MP maps) |
| `0x20` | water |
| `0x800` | sky |
| `0x10000` | playerclip |
| `0x20000` | monsterclip |

The masks are retail's: a player moves with 0x2810011 (`ClientThink_real`'s tracemask, `cod11-mantle.md`), a bullet with 0x2802031 (`Bullet_Fire_Extended`), a blast with 0x2802091 (`CanDamage`), a missile's move with the blast's 0x2802091 (`fire_grenade`'s clipmask, cod11-combat.md 11.1) and its explode trace with 0x11; `collision.rs` names them `MASK_PLAYERSOLID`, `MASK_SHOT`, `MASK_BLAST`, `MASK_MISSILE_MOVE` and `MASK_MISSILE`. Of the bits above 0x20000 only 0x800000 and 0x2000000 are in a mask and no stock material carries either; 0x2000 is in both the shot and the blast mask and 0x80 in the blast's, and both ride the kerb and floor brushes' 0x2080 word, so a bullet stops on a kerb a player walks over as terrain. `collision.rs` enters every brush whose word meets one of those masks or the item masks (0x81, 0x491), so these brushes are in the clip and only the player mask passes them. VERIFIED, `flag_census` over the stock paks: 0x2080 is the only word that meets a mask without SOLID, PLAYERCLIP or GLASS, and no 0x400-only brush exists. The shot mask's 0x20 is water, which vcod keeps as volumes rather than clip brushes, so a bullet does not stop at mp_harbor's water the way retail's does.

Census over all 49 stock maps (SP and MP) in `pak[0-4].pk3` pins two more bits, both VERIFIED by decoding every lump-0 material of every map:

- **Water is `content_flags & 0x20`**, Q3's `CONTENTS_WATER` value unchanged. Every liquid texture carries it (`textures/common/water` 0x28000020 = translucent+detail+water, `textures/sfx/*water*` 0x20000020 or 0x28000020, bare waterfalls 0x20); no other low bit behaves like a fluid, and lava/slime (Q3 0x8/0x10) never appear on a brush-referenced material. Of the stock MP maps only mp_harbor has water brushes.
- **Ladders are `surface_flags & 0x8`** (Q3's `SURF_LADDER`) on `textures/common/ladder`, whose brushes are playerclip (content 0x28010000 = translucent+detail+playerclip). The flag rides the material, so a trace hit can read it from the brush's material word without new lumps. Nine of the fourteen stock MP maps carry ladder brushes: bocage, depot, harbor, powcamp, railyard, rocket, ship, stalingrad, tigertown. Because they are playerclip they are already inside vcod's collision mask - today you walk into them as solid walls; ladder climbing takes over when the forward detection trace sees the flag.

### Soup-side content words

The same census run per soup material (the `flag_census` example, names printed for soup uses) splits the world-model soups into collidable and walk-through classes, all VERIFIED by lump decoding:

| content word | soup use | materials |
|---|---|---|
| `0x1` | ~2900 | every solid wall/floor/terrain-with-brushes surface |
| `0x4` bare | 72 | brushless terrain (`a_grass1a/b`, `dirt_earthbase`, snow, rubble) and in-wall decals |
| `0x10000`/`0x30000` + TRANSLUCENT | 213 | masked metal/wood fences (`ironfence1a`, `barbed_fence1`, `rooflattice`) - PLAYERCLIP/MONSTERCLIP |
| `0x20000004` | 385 | ground/wall decals (`decal@road512`, plugo patches) |
| `0x20000002`/`0x2` | 71 | foliage bushwalls, treelines, background trees |
| `0x8000000` | 14 | autosprite2 masked wire fences |
| `0x20000020`/`0x20` | 21 | sfx water planes |

vcod's rule: a soup collides when its material carries `0x1` or `0x10000`, or is bare `0x4` (brushless terrain normalized to solid), and it does not lie on a glass brush's faces; everything else drops out of the collision build. The pane exception is mp_depot's: its 80 `glass@window2a` brushes (0x8000010, the only glass brushes in the stock MP maps) draw one face with a `glass_nosight@fwindow5` soup whose lump-0 entry is 0x1, and a round goes on through it on retail (`cod11-combat.md` 2.4). So bushes and decals are walk-through, while masked wire/iron fences keep stopping players. Bullets sweep solids only (`shot_trace`), so they pass playerclip-only geometry like Q3/RTCW's MASK_SHOT; not yet checked against `Bullet_Fire_Extended` in the retail binary. The content words and their soup counts above are VERIFIED, read out of the shipped maps. That retail's own collision test reads those bits this way is INFERRED from the bit names.

## Terrain has no brushes

Ground-level spawns on mp_pavlov sit 200 or more units above the first brush below them (bedrock at z = -192). Terrain and patch surfaces have no brushes: the engine collides them through lumps 24-26 (below), and the render soups draw the same surfaces. `CollisionWorld::build` takes the terrain partitions' triangles from those lumps and sweeps them as retail's sphere (`cod11-mantle.md`, "Terrain is a swept sphere, a patch is a facet"), skipping degenerate triangles (cross product under 1e-6) and padding each triangle's AABB by 0.25 units for the BVH, and builds each patch record into retail's facet grid ("Patch collision" below). No render soup collides; the soup-side content-word rule above describes the soups, not the clip. Brushes are swept with the Q3 `CM_TraceThroughBrush` clip against their planes pushed out by a capsule's radius and tested against its nearer sphere: retail's mover traces as a capsule, `trap_TraceCapsule` (`cod11-mantle.md`, "The player is a capsule").

### Lump 24, collision partitions (16 bytes)

VERIFIED from `cod_lnxded`'s loader loop (0x804b010) and the shipped files:

```
u16 material            // into lump 0; its content_flags are the partition's
u8  kind                // 0 = bezier patch, else terrain
u8  pad
// patch:
u16 width, height       // control grid, width x height points
u32 tolerance           // CM_GeneratePatchCollide's subdivision distance
u32 first_vert          // into lump 25
// terrain:
u16 vert_count, index_count
u32 first_vert          // into lump 25
u32 first_index         // into lump 26, indices relative to first_vert
```

Lump 25 is `f32 xyz` per vertex and lump 26 `u16` per index, both shared by the two kinds. mp_carentan carries 568 terrain and 534 patch records, mp_pavlov 619 in all; the patches are almost all 3x3, 3x5, 5x3 and 9x3 grids, and every kerb wall on carentan is a flat 3x3 one. `bsp.rs` parses the three lumps into `MapWorld::terrain`, `patches`, `collision_verts` and `collision_indices`.

### Patch collision

VERIFIED, `cod_lnxded`: `CM_LoadMap`'s loop (0x804b010) copies a patch record's `width * height` control points out of lump 25 and calls 0x804dfb4 with the width, the height, the record's `u32` at +8 and the points, and stores the result in the partition's slot +0x24 and the bounds in +0x0c..+0x20. 0x804dfb4 raises `CM_GeneratePatchFacets: bad parameters` (0x80ccdc0), `... even sizes are invalid for quadratic meshes` (0x80cce00) and `... source is > MAX_GRID_SIZE` (0x80cce60), Q3's `CM_GeneratePatchCollide` strings. INFERRED, from the call structure and the strings (`CM_SetBorderInward: mixed plane sides`, `CM_AddFacetBevels... invalid bevel`, `MAX_FACETS` at 0x80ccda0) read against Q3 1.32's `cm_patch.c`: the functions below are Q3's, with the same `facet_t` (0x140 bytes: surface plane, border count, 26 border planes, 26 inward flags, 26 no-adjust flags), the same 129-point grid (0x60c bytes per column) and the same pools (0x1000 planes, 0x400 facets).

| function | address |
|---|---|
| `CM_TransposeGrid` | 0x804bdf0 |
| `CM_SubdivideGridColumns` | 0x804c040 |
| `CM_RemoveDegenerateColumns` | 0x804c338 |
| `CM_PlaneEqual` | 0x804c454 |
| `CM_FindPlane2` | 0x804c568 |
| `CM_FindPlane` | 0x804c648 |
| `CM_EdgePlaneNum` | 0x804c8b0 |
| `CM_SetBorderInward` | 0x804caf4 |
| `CM_ValidateFacet` | 0x804cd28 |
| `CM_AddFacetBevels` | 0x804ceec |
| `CM_PatchCollideFromGrid` | 0x804d754 |
| `CM_TracePointThroughPatchCollide` | 0x804e334 |
| `CM_TraceThroughPatchCollide` | 0x804e944 |
| `CM_PositionTestInPatchCollide` | 0x804f49c |
| `BaseWindingForPlane` | 0x804fb7c |
| `ChopWindingInPlace` | 0x805046c |

VERIFIED, the epsilons in `.rodata`, Q3's values stored as doubles: the plane compare's 0.0001 on the normal and 0.02 on the distance (0x80ccbb8, 0x80ccbc0); the triangle-plane and border-side ±0.1 (0x80ccbe0, 0x80ccbe8, 0x80cccb0, 0x80cccb8); the wrap and degenerate-column point compare ±0.1 (0x80cce98, 0x80ccea0, 0x80ccba8, 0x80ccbb0); the edge plane's 4 units off the triangle (0x80ccc20, a float); the winding chop's 0.1 (floats at 0x80cccc0 and 0x80ccd70); the 0.5 under which an edge or bevel is degenerate (0x80ccd78); the trace's 0.125 and -1 (floats at 0x80ccebc and 0x80cceb8); the base winding's 131072 (0x80ccf68, with -131072 at 0x80ccf64), where Q3 has 65535.

CoD's differences from Q3, INFERRED from the code at each address:

- **The subdivision tolerance is the record's.** `CM_SubdivideGridColumns` loads the +8 word with `fild` (0x804c11d) and splits a column where `0.25 * |p0 + p2 - 2 p1|` (0.25 at 0x80ccba0) is above it; Q3 compares against the constant 16. VERIFIED, vcod census of the 15636 patch records of the 45 stock maps (2026-10-07): the word runs 1 to 61, 4 on 8181 records and 8 on 5264.
- **A column that needs no split keeps its approximating point.** CoD steps two columns (`add [ebp-0x2c], 2` at 0x804c17b) where Q3 removes the middle column and steps one, so a flat 3x3 patch stays a 3x3 grid of four facets where Q3 collapses it to one. `CM_RemoveDegenerateColumns` removes only a column equal to its neighbour.
- **An edge bevel needs a point behind it.** `CM_AddFacetBevels` keeps an edge-by-axis bevel when no winding point is more than 0.1 in front of it and at least one is more than 0.1 behind it (0x804d490-0x804d4f9). Q3 skips a bevel equal to the surface plane instead, and CoD has no such test for the edge bevels; its axial bevels keep it (0x804d0dc).
- **`CM_ValidateFacet` rejects a facet wider than 131072** (0x80cccc4) rather than 65535.
- **The trace's leave fraction starts at the trace's fraction**, not 1 (0x804e990). A hit needs its enter fraction under the trace's fraction anyway, so the outcome is Q3's.

VERIFIED, the x87 roundings from the disassembly: the cross product (0x80659cc) rounds each component once; `VectorNormalize` (0x8065a38) stores the length as a float and multiplies each component by its reciprocal; `ChopWindingInPlace` stores each distance as a float (0x805050a) after comparing the unrounded one, and computes a split point from an unrounded `dist / (dist - next)` with one rounding per coordinate (0x8050787-0x805080f), a coordinate whose normal component is exactly ±1 taking the plane's distance; the subdivision stores the three new points from unrounded midpoints (0x804c2d4-0x804c2f8). The capsule trace keeps `d1`, `d2` and each fraction unrounded and compares a fraction before storing it (0x804eb87-0x804eb91).

VERIFIED, the dispatch (`cod_lnxded`): `CM_TraceThroughPatchCollide` hands a point trace (`tw+0xc0`) to 0x804e334 (0x804e950); 0x804e334 runs only while `cm_playerCurveClip` is set, registered by `CM_LoadMap` (0x804b426) with the default "1"; the box trace (0x8056310) sends a trace whose start equals its end to the leaf position test (0x8056798), whose patch arm calls 0x804f49c (0x8054648) and on a hit writes fraction 0, `startsolid` and `allsolid`; the leaf trace (0x8055608) clips a patch whose bounds meet the trace's and copies the material's surface flags and contents when the fraction drops. INFERRED: nothing else in the patch code writes `startsolid`, so a moving trace never reads a patch as solid.

VERIFIED, the same vcod census: every stock record builds, 303002 facets in all, none past a pool. The patch materials carry 0x1, 0x2080 (4915 records: rails and kerbs, which stop a shot and not a player), 0x20030000 and 0x20032080 (masked iron fences), 0x20001000 and 0x20003080 (flags), 0x20000001, 0x20000000 and 0x20020000; the trace tests each against its mask unchanged (0x8055608).

vcod: `crates/common/src/patch.rs` ports the generation and the capsule, point and position-test arms with those roundings: `f64` where the x87 keeps a value, `f32` where it stores one. The box arm (`tw->offsets[signbits]`) is not ported, since every vcod trace that reaches a patch is a capsule or a point. VERIFIED, vcod measurement 2026-10-07: a patch record whose rows run along +y and columns along +x faces +z, since the grid is transposed once before its facets are built. A patch hit replaces the trace's only when strictly closer, retail's own test; a sweep that meets a patch is clipped in retail's leaf order ("The leaf walk" below).

### Patch collision on 1.5 and UO

1.5b's `cod_lnxded` and United Offensive's `coduo_lnxded` build the same patch code with another compiler, and it rounds where 1.1d's does not. VERIFIED, the `.comment` strings: 1.1d's binary carries `GCC: (GNU) 2.95.2`, 1.5b's and UO 1.51b's `GCC: (GNU) 3.2.3`. VERIFIED, 1.5b's disassembly: every assignment to a float variable is an `fstps` to the frame and every later read an `flds` of it, while an expression still runs on the x87 stack. INFERRED, off that pattern: an unoptimized build, so each C float variable is rounded to `f32` at every assignment.

The 1.5b functions, matched to 1.1d's by their strings and call order, and what each stores that 1.1d's keeps on the stack. VERIFIED, 1.5b disassembly, each address:

| function | 1.5b | stored as a float |
|---|---|---|
| `VectorNormalize` | 0x8066845 | the squared length (0x8066879), and the reciprocal of the length (0x80668a1) |
| `VectorNormalize2` | 0x8066a90 | the same (0x8066ac5, 0x8066aed) |
| `CM_PlaneFromPoints` | 0x804bf99 | nothing more: the edges, the cross product and the distance, as 1.1d |
| `CM_NeedsSubdivision` | 0x804c083 | the quarter length (0x804c108), a power of two off the stored length |
| `CM_Subdivide` | 0x804c123 | the second half point, which the middle point reads back (0x804c1ab) |
| `CM_FindPlane` | 0x804d342 | each point's distance before the 0.1 test (0x804d448, 0x804d4da, 0x804d56c) |
| `CM_PointOnPlaneSide` | 0x804d677 | the distance before the 0.1 test (0x804d6e0); `CM_SetBorderInward` calls it (0x804e08b) |
| `CM_AddFacetBevels` | 0x804e38b | an edge bevel's distance (0x804e8ab) and each winding point's distance from it (0x804e920) |
| `BaseWindingForPlane` | 0x8051cdf | the dot product (0x8051db9); each corner in two steps, `org -/+ vright` and then `-/+ vup` (0x8051eca..0x8052002) |
| `ChopWindingInPlace` | 0x80528b5 | the dot product, then the distance (0x8052967, 0x8052976); the split fraction (0x8052c79) |
| `CM_TracePointThroughPatchCollide` | 0x804f9b3 | both distances of every plane (0x804faba, 0x804faff) and of the struck one (0x804fdca, 0x804fe0f) |

VERIFIED, Ghidra decompiles of both: UO's `coduo_lnxded` patch functions (0x804bfc4..0x804f85b) and 1.5b's (0x804bf44..0x804f60b) read alike, and UO's terrain point clip 0x8055595 matches 1.5b's 0x8055763 line for line. INFERRED: one build of the collision code serves both.

The two builds put the cracks along facet seams in different places. VERIFIED, retail `bulletTrace` on mp_carentan (`client-probes/probe_patchshot`, 2026-10-16, the same `maps/mp/mp_carentan.bsp` from the same `pak4.pk3` in both installs): along the 1.5b turret round of `turret_ab`'s capture (muzzle 1706.721 1822.8395 28.679152, end -3318.5461 -4644.5234 -140.95471), 1.1d stops at fraction 0.0633128, (1388.56 1413.37 17.94), on patch 136 where it meets patch 133; 1.5b and UO 1.51b both return 0.111953, (1144.12 1098.80 9.69), the wall behind, which is where the captured round's impact lies. The same segment moved 0.1 up stops on 1.5b and passes on 1.1d. VERIFIED, the probe's 24 offsets around the round: 1.5b passes 0.03 above and below it and up to 0.3 to its left (from 0.5 to 2 left it passes patch 136 and stops on the brush behind at x 1376.12), and stops 0.1 and more above or below; 1.1d passes only 0.03 to the right and 0.1 above. INFERRED: each build leaves a thin crack along patch 136's row seam at z 18, in different places.

VERIFIED, the same probe's 790 segments through mp_carentan's patches (`client-probes/probe_patchseams`, fixture `crates/common/tests/fixtures/patch/mp_carentan-seams.txt`): 1.5b answers differently from 1.1d on 583 of them, and UO 1.51b answers as 1.5b on all 790.

VERIFIED, 1.5b's brush clip (0x80590d6, the capsule arm a bullet takes, as 1.1d's 0x8054e90 does) stores what 1.1d's keeps: each side's `d1` and `d2` (0x8059166, 0x8059193 on the axial sides; 0x805947c, 0x80594a0 on the others), `d1 - d2` (0x80591af, 0x80592a4), `d1 - 0.125` (0x80591fa), the enter fraction before the leave test reads it back (0x8059215, 0x8059218) and the leave fraction (0x80592bf); the pushed-out distance (0x805933d) and the shifted start and end (0x8059391..0x80593eb) are floats in both builds.

vcod: `patch::Arith` names the two builds. `PatchCollide::generate_as` and `trace` round every stored variable in the table under `Arith::Stored`, and so does `clip_brush_capsule` at the brush clip's stores. `CollisionWorld::build` builds each patch both ways, keeping the stored one only where its planes or facets differ (400 of mp_carentan's 534 patches). `CollisionWorld::set_arith` picks one for every trace and also routes terrain point traces to `Terrain::point_clip_stored`, the UO build's that 1.5b shares, and terrain capsule traces through `Terrain::clip_capsule`'s stored arm (1.5b 0x8055aab, every variable stored, on the UO build's planes and edge frames; cod15-movement.md 7). The server sets `Arith::Stored` on protocols 6 and 22. A third build, `Arith::Msvc`, is the Windows clients', which store some of the table's variables and keep others (cod15-movement.md 8); `generate_as` and `trace` keep a variable only gcc 3.2.3 stores through `Arith::gcc_var`, and `CollisionWorld::build` keeps that build of a patch too where it differs. The client sets it. Measured against the fixture (`collision::tests::patch_seams_clip_as_each_retail_build`), ours answers all 790 segments as 1.1d does under `Arith::Kept` and as 1.5b does under `Arith::Stored`, together with the leaf walk below.

VERIFIED, 1.5b disassembly: the patch capsule arm (`CM_TraceThroughPatchCollide` 0x805050a, called from the patch dispatch 0x8058ffc) hands each plane to `CM_CheckFacetPlane` (0x805036e), a function of its own where 1.1d inlines it, which stores `d1` (0x80503b3) and `d2` (0x80503ec), the enter fraction before it is clamped to 0 and compared (0x805047a) and the leave fraction before it is clamped to 1 and compared (0x80504cb); the 0.125 is the float at 0x80d6cfc. The sphere's pushed-out distance and the shifted start and end are float arrays in both builds. 1.5b's position test (`CM_PositionTestInPatchCollide` 0x805111e, called from the leaf position test 0x8058494 at 0x80585e7) compares each plane's distance on the x87 stack without a store (0x80512e0..0x8051302, 0x80514fb..0x805151d), as 1.1d's does. The function between the two (0x8050b4f, called from 0x805b822) is the same facet walk returning only whether a facet stops the trace.

VERIFIED, `coduo_lnxded` 1.51b against 1.5b, instruction for instruction with absolute addresses masked: the brush clip 0x8058f0a matches 0x80590d6 (779 instructions), `CM_CheckFacetPlane` 0x8050216 matches 0x805036e, the capsule arm 0x80503b2 matches 0x805050a and the position test 0x8050fc6 matches 0x805111e; UO's 0.125 is the float at 0x80dc4fc.

vcod: `PatchCollide::trace` rounds the capsule arm's `d1`, `d2` and both fractions under `Arith::Stored` (`patch::tests::the_stored_capsule_arm_rounds_where_1_5b_stores`: one slanted sweep lands a bit apart under the two builds); the position test needs no change.

VERIFIED, retail player movement against patches (`client-probes/probe_patchwalk`, `--save-slope --probe-patchwalk`, 8 ms cmds, 2026-10-17; `crates/server/tests/fixtures/playerstate/mp_carentan-dm-slope-8ms-patchwalk-<spot>-<1.1|1.5|uo>.txt`): on 1.1d, 1.5b and UO 1.51b, a player runs into mp_carentan's pole (patch 4), a pillar on terrain (patch 11) and the row of posts round the square (patches 97, 98, 101, 133 and 136), slides along each both ways, runs at it turned 30 degrees, jumps at it, crouch walks and crawls along it and stands up beside it. Ours, rebased on each snapshot under that build's rules and patch arithmetic (`moves_ab.rs`, `the_predictor_replays_walks_against_patches`), matches every snapshot of the nine captures within the gate's tolerance, and the snapshots in contact with a patch to the bit but one: a last-bit x on floor patch 459 in a straight run at 224, the drift its terrain and brush neighbours on the same run show (13 of 372 origins on 1.5b's pole run and 42 on its post row leave the last bit, the rest on terrain or brush floors). VERIFIED, the same replay with 1.5b's runs under `Arith::Kept`: 16 more origins on the pole run, 94 on the pillar and 55 on the posts leave the bit, and one `movementDir` goes wrong, so the stored patch arithmetic is what the 1.5 and UO servers move players by. UO's crawl along the brick wall at (956.9, 600) on the pillar run stops about 0.6 units sooner than 1.1d and 1.5b, which is UO's tighter prone bend bound and not a patch (coduo-movement.md, "The prone check").

### The leaf walk

VERIFIED, `cod_lnxded` 1.1d: `CM_TraceThroughTree` (0x8055fe0) is Q3's: it returns at once when the trace's fraction is at or below the node's entry fraction, sends a leaf to 0x8055608, and otherwise splits the segment at the node's plane with an offset of the box's extent on an axial plane, 0 for a point and 2048 (0x80cd434) for anything else on a slanted one, the near side first and each side pushed 0.125 (0x80cd438) past the plane. 0x8055608 clips the brushes the leaf lists (lump 22), each once by its check count and stopping when the fraction reaches 0, and then, unless the cvar at 0x831b180 is set, the lump-24 partitions it lists (lump 23) the same way. 1.5b's leaf function (0x8059a41) has the same shape. VERIFIED, UO's `coduo_lnxded`: its leaf function (0x8059875) is the same loop, `CM_TraceThroughBrush` (0x8058f0a) per brush and a break once the fraction (`tw+0xc4`) is 0, and its box trace (0x805a9d5) sends a moving sweep on a submodel there with the model's own leaf, so a submodel's or a vehicle collmap's brushes clip in lump order and a contact at fraction 0 ends them: a later brush holding the whole sweep never reads it all solid (`CollisionWorld::clip_model`, coduo-vehicles.md 11.7). INFERRED: a partition that no leaf lists is never clipped, and one is clipped only when the walk enters a leaf that lists it.

VERIFIED, vcod census of mp_carentan's lumps 21-24 (2026-10-16): leaf record bytes 8 and 12 are the leaf's run in lump 23 and bytes 16 and 20 its run in lump 22; patches 69, 109, 283 and 284 (lump-24 records 637, 677, 851 and 852) are in no leaf's list.

VERIFIED, vcod census of every stock map's lumps 21-24 and 27 (2026-10-17, `collision::tests::unlisted_lump_24_on_stock_1_1_maps` pins the 1.1 half), counting model-0 partitions only (a submodel's are never in the tree's leaves): on the twelve stock 1.1 maps the only unlisted ones are mp_carentan's four patches and mp_brecourt's terrain record 385 (vcod's terrain numbering: five points and no index, so no triangle; 1 of its 860). 1.5's mp_bocage, mp_stalingrad and mp_tigertown have none; mp_neuville has six unlisted patches (65, 66, 68, 69, 71, 72). United Offensive has unlisted terrain with triangles on mp_arnhem (1328, 1358, 1381, 1387, 1388, and the 0x2080 kerbs 2061, 2063, 3322), mp_foy (637, 639), mp_kharkov (861, 1147, 1196, 1306), mp_kursk (477, 809, 810, 817, 818, 819, 821, 823, 824, all at x past 90000), mp_peaks (80, 586) and mp_streets (274, 341), triangle-less records on mp_berlin, mp_foy, mp_kharkov, mp_kursk and mp_peaks, and unlisted patches on mp_cassino (205, 412, 418), mp_kharkov (747), mp_ponyri (985..1012), mp_uo_carentan (carentan's four) and mp_uo_stanjel (147). Straight down from a triangle's centre, only mp_kursk's and mp_peaks' 586 leave a spot where no other surface stands at or above the partition's plane; the rest lie on or under other geometry.

VERIFIED, retail UO 1.51b (`client-probes/probe_terrainleaf`, 2026-10-17, fixture `crates/common/tests/fixtures/patch/terrainleaf-uo.txt`): from 8 units over mp_kursk's unlisted terrain 477, 809, 810 and 818 and mp_peaks' 586, a `bulletTrace` 400 units down reaches its end (fraction 1) and a spawned weapon item falls past the plane (z -3397 on mp_peaks 3 s later), so an unlisted terrain partition clips neither a point nor a capsule, as an unlisted patch does not. Ours answers all 12 lines as retail does (`collision::tests::unlisted_terrain_never_clips`); with the terrain half of the rule turned off the first mp_kursk point stops at 0.0196875 on the partition.

VERIFIED, retail 1.5b (`client-probes/probe_neuvilleleaf`, 2026-10-17, fixture `crates/common/tests/fixtures/patch/neuvilleleaf-15.txt`): mp_neuville's unlisted patches 68 and 69 are upright 3x3 arches in the plane x = -15480 (z 92 to 96, y 4847 to 4873 and 4823 to 4849) with open air east of them. A bulletTrace from (-15440, y, z) 80 units west through either stops at x -15483.88 (fraction 0.548437), on the wall behind; one the patch clipped would stop at -15479.875 (0.498438), where vcod's patch alone stops it. All six spots pass, so 1.5b does not clip an unlisted patch either, and vcod gives the same six lines (`collision::tests::neuvilles_unlisted_patches_never_clip`). The other four (65, 66, 71, 72) lie inside a brush wall, and no trace can tell whether they clip. VERIFIED (measured): the test passes with the build's drop of unlisted partitions turned off as well, since the leaf walk's own per-leaf test keeps the two patches out, so it pins retail's answer rather than one of the two guards.

VERIFIED, retail UO 1.51b (`client-probes/probe_patchside`, 2026-10-18, fixture `crates/common/tests/fixtures/patch/patchside-uo.txt`): a patch facet stops a round only from in front of its surface. mp_streets' arch corner 391, over the window at (292, -6660), lies in the plane x = 292 facing +x (y -6657 to -6633, z 173 to 201). Its mirror 393 lies on the wall's other face at x = 284, facing -x, over the same span. A bulletTrace 40 units along x from in front of 391 stops at x 292.125 (fraction 0.496875). From behind, it passes 391 and stops on 393 at 283.875 (0.296875). Ours gives all six lines (`collision::tests::a_patch_stops_traces_from_its_front_only`). INFERRED, from the capsule arm's `d1 <= 0 && d2 <= 0` early accept (0x804e944, 1.5b's `CM_CheckFacetPlane` 0x805036e): a capsule behind the surface meets the facet only through a border, so it is one-sided too. A standing capsule swept through the arch at y -6645 is stopped by 391 going -x and by 393 going +x, in ours. The window that closes against a body inside but not outside (uospawns, crouchjump reports) is therefore not a missing facet side; the two arches are mirror images, so it rides where the two walks meet them.

INFERRED, from `CM_TracePointThroughPatchCollide` (0x804e334): a patch facet is taken only when its unpushed crossing (`d1 / (d1 - d2)`) is within the trace's fraction, and it then sets the fraction 0.125 units short of that crossing. A brush tests its pushed fraction instead. So the order the walk meets things in decides some answers: a patch met first stops the round on it, and the same patch met after a brush 0.1 units behind it lets the brush stand.

VERIFIED, the fixture above: of the 27 lines ours answered differently from retail before the walk and the brush stores, patches 69 and 109, which no leaf lists, explained 12, the order explained 8 (patch 469 before patch 4, patch 18 before brush 2775, patch 82 before brush 2537, and 382..386 on 259 and 260), and 1.5b's brush stores the last 7.

VERIFIED, `cod_lnxded` 1.1d `CM_BoxTrace` (0x8056310): a moving trace on model 0 goes to `CM_TraceThroughTree` (0x8055fe0) whether it is a point or a capsule, a submodel's straight to the leaf function (0x8055608) with the model's own leaf, and a trace that does not move on model 0 to the position test 0x8054b1c, which collects the leaves the box touches (0x805416c) and tests each (0x8054648). So model 0's patches meet a capsule, and a position test, only from the leaves that list them, as they meet a point.

VERIFIED, vcod census of the stock 1.1 maps' lump 27 and lump 21 (2026-10-16): lump 27 bytes 32..40 are a model's run in lump 23. Model 0's run holds every list the tree's leaves point at; each submodel's is the list of a leaf of its own appended after the tree's (mp_rocket: leaves 1987..2007, which no node reaches, one per submodel with that model's brushes and partitions). mp_rocket's `*3` and `*4` (exploders) own 21 and 14 patches, mp_ship's `*2` (a dock) 15 and mp_depot's `*1` and `*2` 36 and 3; no 1.1 submodel owns terrain. United Offensive's do: mp_arnhem's `dom` flag pieces (`*17` and on) own terrain, as do models on mp_berlin, mp_foy, mp_ponyri, mp_rhinevalley and mp_uo_stanjel.

VERIFIED, retail 1.1d (`client-probes/probe_leafgate`, 2026-10-16, fixture `crates/common/tests/fixtures/patch/leafgate.txt`): straight down 400 units from spots over patches, a `bulletTrace` and a script-spawned weapon item, whose `(-1,-1,-1)..(1,1,1)` sweep is a capsule. Over mp_railyard's patch 120, where the leaves under the spots do not list it, both fall through (the item to z -2514); over mp_depot's 220, 234 and 238 both stop on the brush floor 70 units below the patch; over mp_harbor's 53 the point falls through. Over mp_rocket's `*4`, which dm keeps (its `script_brushmodel` is the one left, entity 77, no targetname, `script_exploder` 1), the round stops at 0.0714907 and reports entity 77, and the item rests on the patch; over mp_ship's dock, which `mp_ship.gsc` deletes outside `re`, both fall through. Three of the item spots start inside a brush and are left out of the fixture.

vcod: `leafwalk.rs` holds model 0's tree and each leaf's brushes and partitions as the collision world's indices. `CollisionWorld::build` leaves out a model-0 partition no leaf lists (four patches on mp_carentan, and mp_brecourt's terrain record 385, which has no triangles), and gives every partition its owning model from lump 27 (`WorldPatch::model`, `terrain_model`), placed at the entity's origin. A submodel's partitions clip as its brushes do, only while it is linked and at its spawn place, and report its entity (`CollisionWorld::entity_num`); a posed submodel's partitions are not clipped at all. The BVH walk runs as before; any sweep, point or capsule, that meets a model-0 patch's box is clipped again from scratch in retail's order (`CollisionWorld::clip_leaf_walk`): the leaves the walk enters, skipping one entered at or past the fraction, their brushes and then their partitions, each once, and then the submodels' brushes and partitions and the static models through the BVH. INFERRED, not measured: retail clips the submodels and the static models after the world, as entities. Each listing carries its BVH prim's box, and a listing whose box misses the sweep cut at the trace's fraction (padded a unit) is passed over before its check count, as the BVH passes it over: it can neither start the sweep solid nor cut it shorter, then or in a later leaf, since the fraction only falls. The walk after it enters only the BVH subtrees that hold a prim outside model 0 (`BvhNode::rest`). VERIFIED (measured 2026-10-16, `nav_build`'s graph digest): neither changes a graph on the twelve stock 1.1 maps or on UO's mp_cassino, mp_peaks, mp_arnhem and mp_uo_carentan; bot-navigation.md ("Build times") has the cost. Which sweeps take the leaf walk is the BVH pass's to say: a patch counts as met when its box meets the sweep cut at the fraction the brushes and terrain before it in that pass left. VERIFIED (measured 2026-10-16, the same digest): deciding earlier, at the fraction the brush walk had reached, moves the graphs of mp_carentan, mp_dawnville and UO's mp_uo_carentan, so the pass stays, and only the replay after the first met patch is skipped. Measured against the fixture (`collision::tests::lump_24_clips_from_its_leaves_or_as_its_submodel`), ours answers all 23 lines as retail does; before, 10 were off: the railyard, depot and ship items and the two rocket rounds.

VERIFIED, retail footsteps (the patchwalk pillar captures above, 1.1d ct 25184, 1.5b 25637, UO 26141 and 26593): a player crossing (817, 694) on mp_carentan, where floor brushes 3880 (surface 21) and 3882 (surface 4) meet at z 8.125, steps with `EV_FOOTSTEP_RUN` + 4 on every build. INFERRED, from 1.1d's `CM_TraceThroughBrush` capsule arm (0x8054e90, the leave fraction starts at the trace's): a capsule that enters a second brush at exactly the trace's fraction leaves the trace alone, so of two brushes a resting body touches at one height the first the leaf walk clips names the surface. vcod: `clip_brush_capsule` notes such a tie (`note_contact`), and `walk_tree` replays a trace whose contact ties another through the leaf walk, as it replays one that meets a patch (`collision::tests::a_capsule_on_two_floor_brushes_reads_the_first_in_leaf_order`). Before, our BVH met 3880 first and stepped with surface 21 there.

INFERRED, from the same arms: exact ties are not the only contacts whose order decides the answer. Each case below follows from the compares in those arms.

- A capsule brush keeps its enter fraction on the stack and compares it with the trace's stored one. A brush that enters a float step under a contact whose stored fraction rounds to the same float replaces it, so of two such brushes the later one wins.
- The terrain arm (0x8052a58) takes a triangle whose raw fraction is at or under the trace's, and stores the fraction `FRACTION_EPS` (1e-5) short of that. So of two contacts within that back-off of each other, terrain against terrain or terrain against a brush, whichever comes first wins.
- The box arm's contacts at fraction 0 depend on which brush the walk meets first, because the walk stops there.

VERIFIED (measured 2026-10-18, `collision::tests::near_ties_read_as_retails_leaf_order`): over random ground traces, short runs and step-downs of a standing player on mp_depot and mp_brecourt, each as a capsule and as a box, about 1 in 80 of mp_depot's answers differed from the leaf walk's before the change. Over twelve stock 1.1 maps (360k traces), every difference fell in one of these classes: brush against brush at fractions a float step or less apart, brush against terrain and terrain against terrain within the back-off, submodel against model-0 brush (retail clips the world first and the entity's equal fraction loses), and box traces stopped at fraction 0.

vcod: the brush and terrain arms note any contact within twice the back-off of the trace's (`note_contact`), kept or rejected. The exception is one that repeats the trace's own fraction, side and surface on a brush of the same model, which reads alike in either order. `walk_tree` replays a noted trace through the leaf walk, and so does a box trace stopped at fraction 0. After that change the twelve-map sample matched the leaf walk except for two capsule traces stopped at 0 on one brush, where the BVH's other contact never got clipped, so nothing noted the tie, and the traces out of the world below.

VERIFIED, retail placeSpawnpoint on all three servers (`client-probes/probe_placebits`, 2026-10-19, fixture `crates/server/tests/fixtures/spawnpoints/placebits-retail.txt`): mp_carentan's entities 128 and 221 at (368, 3360) land at z -23.54245 on 1.1d, 1.5b and UO, ours at -21.253555. The capsule's 262144-unit drop meets the flat face of terrain triangle 2096 at raw fraction 0.000564098 and an edge of triangle 2176 (another partition) at 0.0005728, 1.87e-5 later, inside the tie window. The BVH clipped 2096 first, stored 0.000554 and then culled 2176's partition by the sweep box cut at that fraction, so nothing noted the tie. VERIFIED, vcod measurement: through the leaf walk 2176 comes first, its edge stores 0.0005628 and 2096's raw fraction is then past it, which gives retail's -23.54245 to the bit. vcod: `trace_node` and `clip_leaf` cut the sweep box at the fraction plus the tie window (`TraceInput::reach`), so a prim whose contact lands within it is clipped and notes the tie. That also closed mp_railyard 244 (1.1d, 1.5b), mp_neuville 223 and 319 (1.5b) and mp_arnhem 421 (UO).

VERIFIED, the same fixture: retail 1.5b places mp_neuville's entity 90, at (-14688, 5072, 8), at 2 + 12574720/2^24, which the drop gives only from a start well over the 89.875 ours' ceiling check stopped at. Ours stopped on caulk brush 2647 (lump 4; x -15152..-14624, y 4720..5088, z 160..312, with roof planes (0, 0.7071, 0.7071) 3671.2983 and (0, -0.7071, 0.7071) -3230.0637, so at z 160 and above the brush reaches y 5032 at most). The capsule's top meets the z 160 underside's axial plane pushed out by the box extent while the roof planes, pushed out by the radius from the lower sphere (Q3's sphere pick, 1.5b 0x8059309), still hold it inside: the pushed-out planes overreach past the unbevelled edge, 40 units off the brush's volume. INFERRED, from 0x8055608's loop over the leaf's own list (1.5b 0x8059a41): a brush is clipped only from a leaf that lists it, and the compiler lists a brush only in the leaves its volume reaches, so retail never meets that overreach. VERIFIED, vcod measurement 2026-10-20: no leaf the up sweep enters lists 2647, and with the brush skipped the drop starts at 136 and lands on retail's height to the bit. vcod: `walk_tree` replays a trace stopped on a model-0 brush through the leaf walk when the leaf one unit past the contact does not list it and no leaf the sweep enters short of the contact does (`CollisionWorld::brush_off_the_walk`, `collision::tests::a_brush_no_leaf_on_the_walk_lists_never_clips`). It closed `placespawn_ab`'s last `BITS_GAPS` row.

VERIFIED (measured 2026-10-18, `collision::tests::leaf_order_census`, the same sampler over the twelve maps, 519k traces): the capsules left over stop at 0 on mp_depot (brushes 182 and 193 both at 0 against a wall, leaf order 193) and mp_railyard (two terrain triangles at 0), and a capsule that starts all in solid (mp_depot's brush 5285, mp_harbor's 800 on a step-down) keeps the normal of whatever contact the walk clipped before the solid brush: Q3's allsolid arm sets the fraction and leaves the plane alone. vcod: a capsule stopped at 0 clips each prim at its start box alone (`ties_at_zero`), and one that reaches the tie window and reads otherwise (normal, surface, solid flags, a brush of another model), or a model-0 patch there, sends it through the leaf walk; so does any capsule all in solid. Replaying every capsule stopped at 0 had cost about 22% of mp_depot's walks.

VERIFIED (measured 2026-10-18, same sample): the rest were traces whose walk enters only solid leaves (cluster -1), out of the world, and one under mp_carentan's terrain at the bottom brush (z -151.6 near (1790, 3266)). There the BVH meets a listed terrain partition that no leaf the walk enters lists (mp_powcamp's terrain past its playable walls, mp_carentan's 2214 and 2216). VERIFIED, retail 1.1d (`client-probes/probe_outterrain`, 2026-10-18, fixture `crates/common/tests/fixtures/patch/outterrain.txt`): from 8 units over that terrain at six mp_powcamp spots, a `bulletTrace` 400 units down reaches its end (fraction 1) and a spawned weapon item falls through (z -3218 to -3387 3 s later), where the BVH stopped both on the terrain at about 0.02. So a listed partition clips only from a leaf on the walk that lists it, as an unlisted one never clips. vcod: a trace that stops on a model-0 terrain triangle is checked against the leaves (`terrain_off_the_walk`): a triangle every leaf its box reaches lists passes at once (`tri_listed`, built with the world), then the leaf holding the contact point, then the walk up to the trace's fraction; a partition no leaf there lists sends it through the leaf walk (`collision::tests::terrain_out_of_the_world_never_clips`, all 12 lines). With the three rules the sample matches the leaf walk on every trace, in the world and out of it (`collision::tests::a_capsule_stopped_at_0_reads_the_first_in_leaf_order` pins the depot and railyard cases).

VERIFIED (measured 2026-10-18, `nav_build`, release, user time under a loaded machine): mp_depot's graph build went from about 7.0 s to 7.4-7.9 s and mp_brecourt's from 5.4 s to 5.8-6.0 s. mp_carentan's and mp_brecourt's graph digests moved and mp_depot's did not; `nav_census` passes at the 1.1 and UO installs. VERIFIED (measured 2026-10-18 the same way, two runs each, the machine at a load of 150-260): the zero-fraction and all-solid rules took mp_depot from 6.8-6.9 s to 7.7-8.0 s, mp_carentan from 3.9 s to 4.5 s and mp_brecourt from 5.4-5.6 s to 5.7-5.8 s; the terrain rule adds about 0.2-0.3 s on mp_brecourt and next to nothing on mp_depot. mp_carentan's and mp_brecourt's digests moved again, mp_depot's did not.

## Movement constants and their provenance

`crates/common/src/pmove.rs` holds every tunable in one block. Q3-inherited values are exact copies from the GPL source; CoD-specific values are the community-documented CoD 1 defaults. Source files named below are in the id Software Quake III Arena release (`code/game/...`) and RTCW-MP (`src/game/...`).

| Constant | Value | Origin |
|---|---|---|
| `GRAVITY` | 800 | `bg_public.h` `DEFAULT_GRAVITY` (g_gravity default); pmove reads `ps.gravity`, which `ClientEndFrame` stores from `g_gravity` (`cod15-movement.md`, "ps.gravity") |
| `SPEED_RUN` | 190 | CoD 1 `g_speed` default |
| `SCALE_WALK` | 0.4 | retail's wire `ps.walkSpeedScale`; the ADS walk (`cod11-mantle.md`, "The wish speed") |
| `SCALE_CROUCH` | 0.65 | retail's wire `ps.crouchSpeedScale` |
| `SCALE_PRONE` | 0.15 | retail's wire `ps.proneSpeedScale` |
| `SCALE_BACK / SCALE_STRAFE / SCALE_LEAN` | 0.7 / 0.8 / 0.4 | retail's wire `ps.backSpeedScale`, `strafeSpeedScale`, `leanSpeedScale`, applied by the walk cmd scale at 0x2e690 |
| `JUMP_HEIGHT` | 39 | retail rodata 0x708C8/0x708CC: `PM_CheckJump`'s vz = sqrt(78 * gravity) = 249.8 and `fJumpOriginZ` 39 up (`cod11-mantle.md`, "Jumps") |
| `DIVE_HEIGHT_STAND / LOW` | 34 / 24 | retail rodata 0x70BE8/0x70BEC; the prone dive's vz = sqrt(2 * height * gravity), which vcod's ground jump took until 2026-09-27 (`cod11-mantle.md`, "Going prone") |
| `PM_ACCELERATE` | 9 | retail rodata 0x70844; Q3's is 10, RTCW-MP's 10 too - the community-documented "Q3 exact copy" was wrong |
| `PM_DUCKED_ACCELERATE / PM_PRONE_ACCELERATE` | 12 / 19 | retail rodata; selected in `PM_WalkMove` @0x2f4b0-0x2f4ca (`cod11-mantle.md`, "The walk's accel floor") |
| `PM_AIRACCELERATE` | 1 | retail rodata 0x70848, same as Q3 |
| `PM_FRICTION` | 5.5 | retail rodata 0x70854; Q3/RTCW-MP have 6 |
| `PM_SPECTATOR_FRICTION` | 5 | retail rodata 0x70860; spectators use Q3's `PM_Friction` with this constant instead of the walk one |
| `PM_STOPSPEED` | 100 | retail rodata 0x70824, flat |
| `WALK_ACCEL_FLOOR` | 100 | retail rodata 0x70908: `PM_WalkMove` accelerates at `max(wishspeed, 100)` (`cod11-mantle.md`, "The walk's accel floor") |
| `STEPSIZE` | 18 | `bg_local.h`; retail drops to 10 while PRONE (chooser @0x35045) |
| `OVERCLIP` | 1.001 | `bg_local.h` |
| `MIN_WALK_NORMAL` | 0.7 | `bg_local.h` (steeper than about 45.6 degrees is not ground) |
| `MAX_CLIP_PLANES` | 8 | game.mp 0x34a1c: `PM_SlideMove` clears the velocity once `numplanes > 7`; Q3 has 5, and four bumps from two starting planes never reach 8 |
| `SURFACE_CLIP_EPSILON` | 0.125 | `qcommon/cm_local.h` |
| `MAX_FRAME_MS` | 66 | `bg_pmove.c` pmove msec clamp |
| `HALF_WIDTH` | 15 | CoD bbox `(-15, -15, 0)..(15, 15, height)` |
| `HEIGHT_STAND / CROUCH / PRONE` | 70 / 50 / 30 | CoD |
| `VIEW_STAND / CROUCH / PRONE` | 60 / 40 / 11 | CoD viewheights; `standViewHeight = 60` is corroborated in `player-model-anim-system.md` |
| `LEAN_TIME_TO_MS` | 350 | game.mp rodata 0x70c60, `PM_UpdateLean`'s ramp out ("Lean" below). The live capture of 2026-09-01 read `leanf` climbing 0.1429 a snapshot, which is 50/350; it was read as 340 before the binary was |
| `LEAN_TIME_FROM_MS` | 280 | game.mp rodata 0x70c5c, the ramp back ("Lean" below); the retail server does ease `leanf` back to 0 (the 2026-10-09 `probe_freeze` motion run read -1, 0, 1 across `lean_left`, `center`, `lean_right`) |

The friction/accelerate/jump rows were re-sourced from the retail binaries
during the water/ladder work: the dedicated server's rodata table
(`cod11-mantle.md`, "Tunables") contradicted the community-documented values
this section used to carry. The stance-scaled `PM_STOPSPEED` that stood
here as a deliberate divergence is gone: a crawl's gain of
`19 * dt * 28.5` = 4.33 per frame lost to the flat floor's `100 * 5.5 * dt`
= 4.40 only because the walk's accel floor was missing, and with it the
gain is `19 * dt * 100`. The wading slowdown
this paragraph used to call absent is in the walk cmd scale, not the mover:
`1 - waterlevel / 3 * 0.5` (`cod11-mantle.md`, "The wish speed"), and
`pmove::wish` applies it.
Water and ladder mechanics are documented in `pmove.rs`'s constant blocks with
their sources; the full retail ladder constants live in `cod11-mantle.md`.
The lean is retail's `PM_UpdateLean`, below.

### Lean

VERIFIED, symbols and calls in `game.mp.i386.so` (file addresses):
`PM_UpdateLean` (0x32ac8) is the last call of `PM_UpdateViewAngles`
(0x33377, and 0x32deb in its dead arm), which `PmoveSingle` calls at
0x340fc, ahead of the stance and the ground trace.

- `ps.leanf` (`ps+0x40`) runs -1 (left) to 1 (right). VERIFIED, the netfield
  and the clamps below.
- Input. VERIFIED, 0x32adc-0x32b19: the keys count (`wbuttons` 0x10 left,
  0x20 right, both cancel) only with `pm_flags` 0x4000 clear, `pm_type` 5 or
  below, and `groundEntityNum` not 1023 unless `pm_type` is 1 (linked); then
  `eFlags & 0xc000` (a mounted gun) zeroes them. INFERRED: a player in the
  air, on a gun or dead only eases back.
- The stance scale. VERIFIED, 0x32b1b-0x32b88: 0.25 (rodata 0x70c54) when
  `pm_flags` 1, or `viewHeightLerpTarget` (`ps+0xd8`) equals
  `proneViewHeight`, or the eye is lerping (`viewHeightLerpTime` non-zero)
  up (`viewHeightLerpDown` 0) to `crouchViewHeight`; else 0.5 (0x70c58) when
  `pm_flags` 2 or the eye is lerping to `crouchViewHeight`; else 1.
- The ramp. VERIFIED, the divisors: the step out is `msec / 350 * scale`
  (0x70c60) and the step back `msec / 280 * scale` (0x70c5c). INFERRED, off
  the branches at 0x32bd0-0x32c4c: out stops at `±scale` and back at 0, a
  lean held past the stance's limit (one carried into a crouch or prone)
  snaps to the limit, and with no key it only eases. So prone leans, to a
  quarter; vcod used to block it.
- The wall clamp. VERIFIED, 0x32c63-0x32d50: unless `pm_type` is 1, with
  `leanf` non-zero, a trace through `pm->trace` from the eye
  (`origin + viewHeightCurrent`) to `AddLeanToPosition(eye, yaw, ±1, 16, 20)`
  (rodata 0x70c68, 0x70c64) with mins -8 and maxs 8 (0x70c6c, 0x70c70) and
  mask 0x2810011, then `|leanf|` is held to `UnGetLeanFraction(fraction)`.
- `GetLeanFraction` (0x6ba64) is `(2 - |f|) * f`, `UnGetLeanFraction`
  (0x6ba80) `1 - sqrt(1 - f)`. VERIFIED.
- `AddLeanToPosition(pos, yaw, leanf, roll, dist)` (0x6ba9c). VERIFIED: with
  `f = GetLeanFraction(leanf)` it builds the angles `(0, yaw, f * roll)`,
  takes their right vector and adds it times `f * dist`. Every caller passes
  16 and 20: `G_AddLean` (0x42f3c, rodata 0x73130/0x7312c) for shots,
  swings, throws and the use and look traces; the server's
  `SV_BuildClientSnapshot` for the PVS eye (cod_lnxded 0x808f2a9-0x808f2d4,
  rodata 0x80d6484/0x80d6488); and the cgame's view (below). A full lean
  puts the eye 19.2 units to the side and 5.5 down. RTCW's 28 units straight
  sideways is not CoD's.
- The view. VERIFIED, cgame_mp_x86.dll: `CG_OffsetFirstPersonView`
  (0x30032ae0) ends with `AddLeanToPosition(refdef.vieworg,
  refdefViewAngles[1], predicted leanf, 16, 20)` (0x30032dbe-0x30032dda;
  the cgame's copy is at 0x3003f4e0), and `CG_InterpolatePlayerState` lerps
  `leanf` between snapshots (0x3002936a). VERIFIED: the refdef's roll
  (0x302095d4) is written only by the copies of the predicted
  `viewangles[2]` (0x3003343b, 0x30033527), the add of the offset
  0x30012cb0 returns (0x30032b9c) and the add at 0x30018057: the damage
  kick and the earthquake shake (`cod11-combat.md` 17.1, 17.2). INFERRED:
  **a lean moves the eye and never rolls the view**; the
  roll in `AddLeanToPosition` only tips the offset down. VERIFIED, 0x30033413:
  the intermission arm (`pm_type` 5) copies the view and skips
  `CG_OffsetFirstPersonView`.

vcod: `pmove::update_lean` is the function above, `pmove::aim::add_lean`
is `AddLeanToPosition` with 16 and 20 in each build's rounding
(`cod11-movers.md` 22, "pmove's turns"), and `PlayerState::view`,
`combat::muzzle_point`, `ClientSim::eye_origin` and the playing client's
camera all apply it. The walk mode draws no lean roll. The viewmodel's own
lean offset and roll are `cod11-combat.md` 17.3.
