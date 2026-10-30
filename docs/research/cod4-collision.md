# CoD4 1.7 collision: the clip map as the shared trace kernels read it

What `vcod_cod4::collision::ClipMap` builds out of a map zone's
`clipMap_t`, how the CoD2-family kernels in `vcod_common::clipmap` trace
it, and what is new over CoD2 (`cod2-d3dbsp-format.md`, "What the trace
reads" and "The trace"). The zone walk and the record sizes are in
`cod4-fastfile.md` ("col_map_mp / col_map_sp") and `cod4-world.md`
section 6.

Sources:

- The 21 stock MP map zones of the 1.7 client install
  (`zone/english/mp_*.ff`, `_load` zones aside), read by the gate
  `crates/cod4/tests/clip_census.rs` (`COD4_DIR`). A fact read out of
  them is VERIFIED.
- `cod4_lnxded` 1.7, stripped; its trace functions carry no strings. A
  claim labelled INFERRED that names no 1.7 address is not yet confirmed
  on 1.7.

## 1. Records

Offsets inside each record. Every pointer is stored as an offset token
into the clip map's own arrays and becomes an index as `cod4-world.md`
section 6 says.

| Record | Size | Fields | Label |
|---|---|---|---|
| `cplane_s` | 0x14 | normal 0, dist 12, type byte 16, sign bits 17 | see below |
| `cNode_t` | 8 | plane pointer 0, children i16 at 4 and 6 (negative: `-1 - leaf`) | layout INFERRED |
| `cLeaf_t` | 0x2C | first AABB tree u16 0, AABB count u16 2, brush contents 4, terrain contents 8, mins 12, maxs 24, leaf brush node 36, cluster i16 40 (CoD2's runtime leaf, stored) | layout INFERRED |
| `cLeafBrushNode_s` | 0x14 | axis byte 0, `leafBrushCount` i16 2, contents 4; leaf: brush list pointer 8; split: `dist` 8, `range` 12, `childOffset` u16 16 and 18 | layout INFERRED |
| `cbrush_t` | 0x50 | mins 0, contents 12, maxs 16, `numsides` 28, sides pointer 32, `axialMaterialNum` i16 `[2][3]` 36 (`[0]` the min sides), `baseAdjacentSide` 48, `firstAdjacentSideOffsets` i16 `[2][3]` 52, `edgeCount` u8 `[2][3]` 64 | layout INFERRED |
| `cbrushside_t` | 0xC | plane pointer 0, material u32 4, `firstAdjacentSideOffset` i16 8, `edgeCount` byte 10 | layout INFERRED |
| `CollisionPartition` | 0xC | triangle count byte 0, border count byte 1, first triangle i32 4, borders pointer 8 | layout INFERRED |
| `CollisionAabbTree` | 0x20 | CoD2's lump 34, field for field | layout INFERRED |
| `CollisionBorder` | 0x1C | CoD2's lump 32, field for field | layout INFERRED |
| `cmodel_t` | 0x48 | mins 0, maxs 12, radius 24, its `cLeaf_t` at 28 | layout INFERRED |
| `cStaticModel_s` | 0x50 | `nextModelInWorldSector` u16 0, xmodel 4, origin 8, `invScaledAxis` 3x3 at 20, absmin 56, absmax 68 | layout INFERRED |
| `XModelCollSurf_s` | 0x2C | triangles pointer 0, count 4, mins 8, maxs 20, bone 32, contents 36, surface flags 40; each triangle 0x30: plane, `svec`, `tvec` (four floats each), CoD2's lump 31 without vertex and edge indices | layout INFERRED |

- Plane type is 0..2 where that normal component is exactly 1, else 3;
  the sign bits are the negative components, as CoD2's
  `CMod_LoadPlanes` computes them. 0 planes of the 21 maps differ.
  VERIFIED (the census's "plane rule misses").
- Every node child, partition and AABB index is in range. VERIFIED
  (`cod4-world.md` 6).
- The census's point contents read every tested brush through the leaf
  layout (section 4). VERIFIED.
- Stored brush contents are the runtime ones: CoD2's `CMod_LoadBrushes`
  mask is already applied, and the census reads each through point
  contents. VERIFIED (section 4).
- The census's static model rays hit through the collision surface
  layout (section 4). VERIFIED.

## 2. What the kernels share with CoD2

- The leaf brush trees come prebuilt, in the node shape CoD2's
  `CMod_PartionLeafBrushes_r` builds at load: a positive count is a leaf
  of that many brushes, a negative one walks the next node first (CoD2's
  `straddlers`), 0 only the children; `childOffset[0]` is the side above
  `dist`, `[1]` the side below, `range` CoD2's clearance; node 0 is
  never a root. INFERRED (`CM_TraceThroughLeafBrushNode_r`,
  `CM_TestInLeafBrushNode_r`, `CM_PointContentsLeafBrushNode_r`).
  `vcod_cod4::collision` maps each node onto `clipmap::BrushNode` one for
  one and copies the leaf lists into one run.
- `CM_Trace`'s set-up, `CM_TraceThroughTree`, `CM_TraceThroughLeaf`,
  `CM_TraceThroughBrush` (axial sides against the capsule's box, other
  sides against the nearer sphere), `CM_PositionTest`, `CM_TestInLeaf`,
  `CM_TestBoxInBrush`, `CM_PointContents`, `CM_CullBox`, the AABB tree
  walk with its partition stamps and `CM_TraceCapsuleThroughBorder` read
  as CoD2 1.3's do, so CoD4 runs CoD2's kernels unchanged, the border
  excepted (below). INFERRED.
- Two other differences, INFERRED: `CM_TestBoxInBrush` culls on strict
  bounds where CoD2's accepts a touch (unported; matters only for a box
  exactly touching a brush), and the AABB position walk stops at the
  first child that starts solid (ported in the indexed arm, section 3).
- 1.7's capsule border is not CoD2's. Past the end and height tests, the
  tree walk computes the wall top's height at the contact,
  `z_slope * along + z_base` (0x81173c4, the border's +0x10 and +0xc),
  less the capsule centre's, and compares it with the capsule's half
  offset (trace work +0x8c) and that plus the radius (+0x88); two calls
  to 0x811626a follow (0x8117406, 0x81174c8). 0x811626a reads the
  border's start, length, base and slope (+0x14, +0x18, +0xc, +0x10) and
  calls 0x81150ae, the sphere-edge kernel the triangle arm calls
  (0x8117458). VERIFIED (the disassembly).
- So a wall top beside the capsule's straight part but within a radius
  of an end sphere's centre is met by that sphere, against the top edge
  from the border's start to its end, then the end it passed
  (`CM_TraceSphereThroughBorder`, 0x811626a). INFERRED (the branches and
  the call's arguments). CoD2's kernel (0x08057046) has no such arm, so
  `capsule_border` takes it only for CoD4's indexed partitions
  (`Cx::border_sphere`).
- Without that arm a capsule run at mp_carentan's 8-unit curb at (881,
  1880, -48), whose top lies 26 under the capsule's centre, swept through
  the terrain and ended 8 units inside it, all solid, while the position
  test there read solid. VERIFIED (the clip map traced,
  `a_sideways_capsule_meets_a_low_terrain_border_with_its_sphere`).
- The brush edges (`brushEdges`, `baseAdjacentSide`, the per-side edge
  counts) are read only by the physics' brush collision (the client's
  dynamic entities), never by the trace. INFERRED. vcod builds no edges.
- The leaf array ends with leafs no BSP node reaches: mp_crash has 2165
  leafs, 34 past the tree beside its 33 submodels, and each submodel's
  brushes sit, in model space, in one of those as well as in their
  `cmodel_t` leaf. VERIFIED (the census's "leafs past the tree", 32 to 77
  per map; mp_crash's leafs 2132.. each list one submodel's brushes). A
  world trace never reaches them; `ClipMap::world_leafs` lists the ones
  it does.
- The last leaf brush node names brush `numBrushes`, the run-time box
  brush (`cod4-world.md` 6); vcod appends a placeholder brush with no
  contents at that index. INFERRED (Q3's `CM_InitBoxHull`).

## 3. What is new: triangles and walkable edges

Triangles are three u16 vertex indices each into `verts` (`triIndices`,
`3 triCount` u16s), with no stored plane, so CoD2's triangle kernels do
not apply. `vcod_common::clipmap` carries them as an `IndexedMesh` beside
the shared records; the AABB tree walk takes its indexed arm
(`clipmap/mesh4.rs`) when one is present. All INFERRED:

- A point sweep (`CM_TracePointThroughTriangle`) meets a triangle it
  crosses front to back inside its edges, at the exact crossing: no
  0.125 clip epsilon, unlike CoD2's and every capsule test. Face normal
  `(v0 - v2) x (v0 - v1)` normalized.
- A capsule (`CM_TraceCapsuleThroughTriangle`) tests the sphere nearer
  the face (the lower one, the upper under a downward face) against the
  face 0.125 out. A contact outside an edge sweeps the sphere against
  that edge's line (`CM_TraceSphereThroughEdge`, radius grown by 0.125);
  past an edge's end, against that vertex (`CM_TraceSphereThroughVertex`).
  The vertex normal takes one Newton step of the reciprocal length
  instead of a root.
- `triEdgeIsWalkable` holds a bit per triangle edge, bit `3 tri + k` for
  the edge opposite vertex `k`, LSB first within each byte, `(3 triCount
  + 31) / 32 * 4` bytes (size VERIFIED, `cod4-fastfile.md`). An edge
  contact sets `trace.walkable` from its bit, a vertex contact from the
  AND of the bits of the edges the sweep passed; a face or brush contact
  clears it. `CM_Trace` closes with `walkable = normal.z >= 0.7` when no
  kernel set it and the trace did not start solid. `ClipTrace::walkable`
  carries it; CoD2 never sets it.
- A position test (`CM_PositionTestCapsuleInTriangle`) is solid when the
  capsule's axis segment comes within its radius of the triangle. vcod
  computes that distance with its own closest-point routine
  (`segment_tri_dist2`), not `CM_DoesCapsuleIntersectTriangle`'s
  arithmetic, so a contact within a float step of the radius can read
  differently.

The point sweep's float model. 1.7 inlines it in its tree walk
(`cod4_lnxded` 0x81169e3..0x81179c8) on the x87 stack, VERIFIED (the
disassembly):

- The edge differences stay on the stack except where the code stores and
  reloads one (`e01.x`, `e01.z` in `n.y`, `e02.y`, `e02.z`).
- The cross product's components are each one stored float (`n`), kept
  on the stack for `proj = delta . n` and `t`.
- `v0 - start`'s x and y are stored floats.
- The normal is `Vec3NormalizeTo` (0x8192a64: squared length and root on
  the stack, each component times the reciprocal, one store) of the
  stored `n`.
- The fraction is the stored `t` over the stored `proj`.

`mesh4::point_tri4` takes those stores. With plain `f32` the walk
capture's frag landed a last bit off at its terrain bounce; with them it
lands on retail's. VERIFIED (`the_frag_flies_on_ours_as_on_retail`). The
capsule kernels' arithmetic is still plain `f32`.

## 4. Submodels and static models

- A submodel traces through its `cmodel_t` leaf alone
  (`CM_TraceThroughLeaf`, `CM_TestInLeaf`, `CM_PointContents` on the
  model's leaf), points in the model's frame. INFERRED (`CM_Trace`,
  `CM_PointContents`). vcod: `ClipMap::trace_model`,
  `point_contents_model`.
- Static models clip point sweeps only (`CM_PointTraceStaticModels`):
  bullets and other point traces, never a player's capsule. Each model
  whose xmodel contents meet the mask and whose absolute box the sweep
  reaches is traced in its own frame (`invScaledAxis`, row vector times
  matrix) through `XModelTraceLine`: per collision surface whose box the
  sweep reaches, a front-to-back crossing 0.125 short with `s` and `t`
  inside `-0.001..1.001`; the normal goes back through the transpose and
  is normalized. An xmodel with `collLod` below 0 clips nothing.
  INFERRED.
- vcod keeps each transposed component on the stack until one store and
  normalizes as `Vec3NormalizeTo` does. INFERRED (the 1.7 function is not
  located). With glam's `f32` normalize the frag's bounce off mp_crash's
  rubble read two ulps off retail; with this it reads retail's. VERIFIED
  (the gate).
- Retail walks the models by its world sector tree, built at load
  (`CM_LinkAllStaticModels`); vcod walks them all, which finds the same
  nearest contact but for exact ties. INFERRED.
- A static model whose xmodel the map zone only names (a leading comma,
  an asset another zone holds) clips with that zone's model: mp_crash's
  clip map places 219 `,fx_me_brick_01` rubble pieces, common_mp's, one
  concrete collision surface of 12 triangles each, and the walk
  capture's frag bounces off one (cod4-combat.md 10.2). VERIFIED (the
  zones, the gate). `ClipMap::from_zones` resolves the name in the zones
  loaded before the map's; every zone walk keeps an xmodel's collision.
- dynEnts are not clipped here (`cod4-world.md`).

## 5. The census

`cod4_clip_census` over the 21 maps (debug build, 15 s). Each spawn is
put where `placeSpawnPoint` puts it: a capsule sweep 128 up, then 262144
down, mask 0x2810011, the player's box (-15,-15,0)..(15,15,70). VERIFIED
(the 1.7 trace calls below).

- 4375 `mp_*_spawn` points. VERIFIED. Every sweep down lands; deepest
  drop 99.9 units (mp_pipeline); mp_creek's and mp_shipment's start
  spawns hover 80-90 over the ground. 13 origins start solid before the
  placement (7 on mp_carentan at z -5 and 10, below the terrain). Every
  placed capsule starts clear but two: mp_strike's `mp_sab_spawn_axis`
  (1312, 1524, 44) and `mp_ctf_spawn_axis` (1312, 1520, 52) overlap the
  playerclip brushes 12874..12877 (x up to 1297.5) by half a unit, which
  the capsule meets as its box on an axial side. The gate names them in
  `STUCK_SPAWNS`. VERIFIED (the census, the brushes read off mp_strike's
  clip map).
- `placeSpawnPoint` (entity method row at 0x80c56c6) calls the trace at
  0x80b408a three times (0x80c5754, 0x80c57ca, 0x80c5855), each with the
  box at 0x820e024 / 0x820e030 ((-15,-15,0)..(15,15,70)), mask 0x2810011
  and the entity's own number; it adds 128 (0x820e278) to the first end's
  z and subtracts 262144 (0x821f528) for the second. VERIFIED (the
  disassembly and the constants).
- The second sweep starts where the first stopped, the third is a
  position test where the second stopped. A set byte at the third
  trace's +0x28 prints "WARNING: Spawn point entity %i is in solid at
  (%i, %i, %i)" (0x821f0e4, channel 0x17) with the origin before the drop
  (`cvttss2si` of +0x13c), then `G_SetOrigin` (0x80ca218) moves the
  entity anyway. INFERRED (instruction order). That +0x28 is `allsolid`
  is INFERRED (`trace_t`).
- So 1.7 finds the same spawn in solid and keeps it there: on mp_strike
  `sab` it printed "WARNING: Spawn point entity 315 is in solid at (1312,
  1524, 44)", on `sd` none. VERIFIED live (`cod4_lnxded` under `unshare
  -rnm`). vcod-server logs the same line for the same entity. VERIFIED
  (`bot_match` with `RUST_LOG=warn`). The sweep once showed it on the
  mp_strike `sd` row because it filed a load's lines under the level
  before (cod4-map-sweep.md). VERIFIED (the sweep's code).
- 273524 brushes, 271875 tested (a point a hair off the bounds' centre,
  three tries; the untested ones cut off by a side plane or with no
  contents): each reads its own contents through `CM_PointContents` on
  the model whose tree lists it. 3 brushes no tree lists (mp_crash,
  mp_crash_snow, mp_crossfire, one each). A point exactly on a BSP node's
  plane reads the front leaf, which need not list a brush that only
  touches the plane: mp_backlot brush 12195's centre at x 24. VERIFIED.
- 8708 sweeps between consecutive spawns 40 up (a point under the shot
  mask, the player's capsule under its mask): all end with a fraction in
  0..=1 and a finite normal, 3215 short of the far spawn. VERIFIED.
- 114539 static models, a point ray down through each box's centre:
  44198 meet the model's collision. VERIFIED.

## 6. Open

- 1.7 addresses for the INFERRED facts above. `CM_Trace` should be
  findable off its `0xfff` box-model compare and the 0.7 walkable
  constant in `cod4_lnxded`.
- The float model of the indexed arm's capsule kernels and of the brush
  kernels on 1.7 (the point sweep's is section 3). The pmove A/B
  (`cod4_moves_ab`) is the first gate that can show a last-bit
  difference. Where to read it, VERIFIED (the disassembly: the
  `triIndices` and `verts` globals 0x889dea4 and 0x889de9c): 1.7's AABB
  tree walk for a trace is 0x811673a (recursive, 0x20-byte nodes,
  `childCount` at +0x1a); the point sweep is its first triangle loop
  (0x81169c0); the capsule's sphere, edge and vertex tests follow inline
  (`fsqrt` at 0x81175f8, 0x8117784, 0x8117898, 0x8117a0c); the position
  test's walk is 0x811546a.
- `CM_TestBoxInBrush`'s strict bounds (section 2).
- The server's pmove and script traces run over this clip map
  (cod4-pmove.md 15). `placeSpawnPoint`'s drop onto mp_crash's terrain
  stops a bit short of retail's: the indexed arm's float model.
