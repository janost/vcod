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
| 27 | models | 48 | 35 |
| 29 | entities | text | one block per entity |

The other lump indices are not decoded. Lump 27 bytes 32..40 (the collision-AABB range) are parsed past and never used.

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

## Content flags

From the materials lump, per brush via `brush.material`:

| Flag | Meaning |
|---|---|
| `0x1` | solid |
| `0x4` | seen on terrain (snow) materials, which have no brushes |
| `0x800` | sky |
| `0x10000` | playerclip |
| `0x20000` | monsterclip |

The brush collision mask is `(content_flags & (0x1 | 0x10000)) != 0`. `collision.rs` defines `CONTENTS_SOLID`, `CONTENTS_PLAYERCLIP` and `CONTENTS_SKY` only; monsterclip is not referenced by any code path.

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
| `JUMP_VELOCITY` | 250 | `sqrt(2 * 800 * 39)` for CoD's 39-unit jump apex; Q3 `bg_local.h` has 270 |
| `PM_ACCELERATE` | 10 | `bg_pmove.c` `pm_accelerate` |
| `PM_AIRACCELERATE` | 1 | `bg_pmove.c` `pm_airaccelerate` |
| `PM_FRICTION` | 6 | `bg_pmove.c` `pm_friction` |
| `PM_STOPSPEED` | 60 | Q3 `bg_pmove.c` `pm_stopspeed` is 100; see below |
| `STEPSIZE` | 18 | `bg_local.h` |
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

`PM_STOPSPEED` is the one value I changed from Q3 on purpose. Q3's 100 is 31% of its 320 u/s run speed; 31% of 190 is about 59, so 60 keeps the stop feel by ratio. The friction floor makes any wish speed under `PM_STOPSPEED * PM_FRICTION / PM_ACCELERATE` unreachable, which at 100 is 60 u/s and at 60 unscaled is 36 u/s, still above prone's 28.5, so `pmove.rs` scales the floor by the stance as well (stand 36, crouch 23, prone 5.4). The lean code is RTCW's `PM_UpdateLean` with two deliberate differences: CoD leans while moving, so the `!cmd->forwardmove` gate is dropped, and prone blocks leaning outright.
