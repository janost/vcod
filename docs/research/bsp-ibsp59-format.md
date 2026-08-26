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

Lump 27 bytes 32..40 (the collision-AABB range) are parsed past and never used. The remaining lumps are identified below under "The other lumps"; the visibility set (cells, portals, AABB trees, cull groups, nodes, leafs, PVS) is decoded in "Visibility and tree lumps".

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
u32 material            // parsed, unused
```

The first six sides of every brush are axial and carry an f32 bound coordinate instead of a plane index, in the order 0 = xmin, 1 = xmax, 2 = ymin, 3 = ymax, 4 = zmin, 5 = zmax. Converted to clip planes (`n . p <= d` inside): even side `k` with bound `b` on axis `a = k / 2` gives `normal = -axis_a, dist = -b`; odd side `k` gives `normal = +axis_a, dist = +b`. Sides 6 and up index lump 2; every index on mp_pavlov is in range and every axial pair satisfies `min <= max`. The parser keeps the raw u32 and `collision.rs` decodes it.

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
u32 first_aabb, num_aabbs       // bytes 32..40, not consumed
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
| 19 | 2 | leaf light indices (`u16` into lump 30) | 230 |
| 20 | 36 | BSP nodes | 1752 |
| 21 | 36 | BSP leafs | 1788 |
| 22 | 4 | leaf brushes (`i32` into lump 4) | 10714 |
| 23 | 4 | leaf collision surfaces (`i32` into lump 24), see below | 1418 |
| 24 | 16 | terrain collision partitions (`u16 x 8`, layout not decoded) | 619 |
| 25 | 12 | terrain collision vertices (`f32 xyz`) | 6291 |
| 26 | 6 | terrain collision triangles (`u16 x 3` into lump 25); mp_ship has none | 4496 |
| 28 | 8 (+rows) | PVS: `i32 cluster_count, row_bytes` then `cluster_count` bit rows; mp_neuville has none | 666 x 88 |
| 30 | 48 | lights (one per `r_showLeafLights` entry; retail rejects other lengths, `FUN_004db240`), see `cod11-light-grid-and-leaf-lights.md` | 22 |
| 31 | | empty in every map | 0 |
| 32 | 3145728 | light grid: 262144 (128x128x64) cells x 48 bytes; compiled but never sampled in MP (same doc) | 1 |

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

VERIFIED: every plane, child, brush and lump-24 reference is in range; `cluster` runs to `cluster_count - 1` of lump 28 (665 on mp_pavlov), `cell` to the cell count minus one, -1 for solid leafs. The tree reaches leafs 0..1753 on mp_pavlov; the 34 leafs after them belong to the 34 submodels, as in Q3. `area` is -1 or 0 on every stock map.

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
| `0x20` | water |
| `0x800` | sky |
| `0x10000` | playerclip |
| `0x20000` | monsterclip |

The brush collision mask is `(content_flags & (0x1 | 0x10000)) != 0`. `collision.rs` defines `CONTENTS_SOLID`, `CONTENTS_PLAYERCLIP` and `CONTENTS_SKY` only; monsterclip is not referenced by any code path.

Census over all 49 stock maps (SP and MP) in `pak[0-4].pk3` pins two more bits, both VERIFIED by decoding every lump-0 material of every map:

- **Water is `content_flags & 0x20`**, Q3's `CONTENTS_WATER` value unchanged. Every liquid texture carries it (`textures/common/water` 0x28000020 = translucent+detail+water, `textures/sfx/*water*` 0x20000020 or 0x28000020, bare waterfalls 0x20); no other low bit behaves like a fluid, and lava/slime (Q3 0x8/0x10) never appear on a brush-referenced material. Of the stock MP maps only mp_harbor has water brushes.
- **Ladders are `surface_flags & 0x8`** (Q3's `SURF_LADDER`) on `textures/common/ladder`, whose brushes are playerclip (content 0x28010000 = translucent+detail+playerclip). The flag rides the material, so a trace hit can read it from the brush's material word without new lumps. Nine of the fourteen stock MP maps carry ladder brushes: bocage, depot, harbor, powcamp, railyard, rocket, ship, stalingrad, tigertown. Because they are playerclip they are already inside vcod's collision mask - today you walk into them as solid walls; ladder climbing takes over when the forward detection trace sees the flag.

## Terrain has no brushes

Ground-level spawns on mp_pavlov sit 200 or more units above the first brush below them (bedrock at z = -192). Terrain and patch surfaces exist only as render triangle soups, so brush collision alone drops the player through the ground and a triangle collider is required, not a fallback. `CollisionWorld::build` harvests every triangle of every soup whose material lacks the sky flag, from all models, skipping degenerate triangles (cross product under 1e-6) and padding each triangle's AABB by 0.25 units before building the BVH. Brushes and triangles are then swept with the same Q3 `CM_TraceThroughBrush` clip against plane sets expanded by the box (triangles get face, axis and edge-cross bevels).

## Movement constants and their provenance

`crates/common/src/pmove.rs` holds every tunable in one block. Q3-inherited values are exact copies from the GPL source; CoD-specific values are the community-documented CoD 1 defaults. Source files named below are in the id Software Quake III Arena release (`code/game/...`) and RTCW-MP (`src/game/...`).

| Constant | Value | Origin |
|---|---|---|
| `GRAVITY` | 800 | `bg_public.h` `DEFAULT_GRAVITY` (g_gravity default) |
| `SPEED_RUN` | 190 | CoD 1 `g_speed` default |
| `SCALE_WALK` | 0.4 | CoD slow-walk modifier |
| `SCALE_CROUCH` | 0.65 | CoD |
| `SCALE_PRONE` | 0.15 | CoD |
| `JUMP_HEIGHT_STAND / LOW` | 34 / 24 | retail rodata 0x70BE8/0x70BEC; vz = sqrt(2 * height * gravity), gate forwardmove != 0 (`cod11-mantle.md`, "Jumps") |
| `PM_ACCELERATE` | 9 | retail rodata 0x70844; Q3's is 10, RTCW-MP's 10 too - the community-documented "Q3 exact copy" was wrong |
| `PM_DUCKED_ACCELERATE / PM_PRONE_ACCELERATE` | 12 / 19 | retail rodata; selected in the steep-slope mover @0x2f4b0-0x2f4ca, walk-path application INFERRED (`cod11-mantle.md`) |
| `PM_AIRACCELERATE` | 1 | retail rodata 0x70848, same as Q3 |
| `PM_FRICTION` | 5.5 | retail rodata 0x70854; Q3/RTCW-MP have 6 |
| `PM_STOPSPEED` | 100 | retail rodata 0x70824 (flat); vcod scales it per stance - see below |
| `STEPSIZE` | 18 | `bg_local.h`; retail drops to 10 while PRONE (chooser @0x35045) |
| `OVERCLIP` | 1.001 | `bg_local.h` |
| `MIN_WALK_NORMAL` | 0.7 | `bg_local.h` (steeper than about 45.6 degrees is not ground) |
| `MAX_CLIP_PLANES` | 5 | `bg_slidemove.c` |
| `SURFACE_CLIP_EPSILON` | 0.125 | `qcommon/cm_local.h` |
| `MAX_FRAME_MS` | 66 | `bg_pmove.c` pmove msec clamp |
| `HALF_WIDTH` | 15 | CoD bbox `(-15, -15, 0)..(15, 15, height)` |
| `HEIGHT_STAND / CROUCH / PRONE` | 70 / 50 / 30 | CoD |
| `VIEW_STAND / CROUCH / PRONE` | 60 / 40 / 11 | CoD viewheights; `standViewHeight = 60` is corroborated in `player-model-anim-system.md` |
| `LEAN_MAX` | 28 | RTCW-MP `bg_pmove.c` `LEAN_MAX 28.0f` (eye offset in units; roll is lean / 2 degrees) |
| `LEAN_TIME_TO_MS / FROM_MS` | 280 / 350 | attributed in `pmove.rs` to RTCW-SP `bg_pmove.c`; UNVERIFIED, RTCW-MP has 200 / 300 |

The friction/accelerate/jump rows were re-sourced from the retail binaries
during the water/ladder work: the dedicated server's rodata table
(`cod11-mantle.md`, "Tunables") contradicted the community-documented values
this section used to carry. Two deliberate divergences remain. First,
`PM_STOPSPEED`: retail's flat 100 makes prone unable to accelerate at all -
gain per frame is `19 * dt * 28.5` = 4.33 against a floor loss of
`100 * 5.5 * dt` = 4.40 - so `pmove.rs` keeps the floor scaled by stance
until someone recovers how retail actually compensates. Second, there is no
wading wish clamp: retail references neither wade-scale float anywhere in the
walk mover, so shallow-water slowdown comes from the water friction term alone.
Water and ladder mechanics are documented in `pmove.rs`'s constant blocks with
their sources; the full retail ladder constants live in `cod11-mantle.md`.
The lean code is RTCW's `PM_UpdateLean` with two deliberate differences: CoD
leans while moving, so the `!cmd->forwardmove` gate is dropped, and prone
blocks leaning outright.
