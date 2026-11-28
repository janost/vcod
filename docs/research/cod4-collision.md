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
  walk with its partition stamps and `CM_TraceCapsuleThroughBorder` test
  in CoD2 1.3's order, so CoD4 runs CoD2's kernels, the border and the
  float model excepted (below). INFERRED.
- `CM_Trace`'s set-up keeps more on the x87 stack than CoD2's, which
  stores each value. `cod4_lnxded` 0x811ce5c (the function that compares
  its model with 0xfff at 0x811d21d and ends with the 0.7 walkable test
  at 0x811d259), per axis at 0x811ce9f..0x811cfb8: the box's centre
  offset `(mins + maxs) * 0.5` stays on the stack; the half box (trace
  work +0x5c), the shifted start (+0) and the shifted end (+0xc) are each
  stored once; the sweep (+0x24) is the unrounded shifted end less the
  stored start, rounded once, and the middle (+0x48) is the stored start
  plus the unrounded end, halved. VERIFIED (the instructions). So where
  the shifted end needs more bits than a float holds (a player's z plus
  35), 1.7's sweep is the move's own length, and CoD2's set-up rounds it
  first. `clipmap::trace::Sweep::new` takes 1.7's arithmetic for a map
  with indexed triangles. Before it, a dead body sliding off a curb 0.1
  above mp_crash's terrain met the floor 414 ulps of the fraction early,
  the plain slide stopped one ulp of x short of the step's, so ours took
  the step and kept the falling velocity where retail snapped to the
  ground (cod4-combat.md 15). VERIFIED (the gate, the traced fractions).
  The walk replay's exact rows went from 73 to 410 with it (cod4-pmove.md
  15.1). `mesh_pins`' synthetic sweeps on mp_crash, mp_backlot and
  mp_carentan moved with it, 5 to 34 more hits and 14 to 118 more
  `startsolid` per map in 79k to 440k traces; their digests are retaken
  under 1.7's set-up, which no capture contradicts. VERIFIED (the pins
  under both set-ups).
- The brush kernel keeps its values on the x87 stack too. 1.7 inlines
  `CM_TraceThroughBrush` in the leaf brush walk at 0x811ab08 (the
  function that recurses on itself at 0x811ab63 and offsets each node
  test by 0.125, 0x820e270), its leaf arm at 0x811ad67..0x811b40e.
  VERIFIED (the instructions):
  - Axial sides: `d1 = (start - bound) * sign - half` (the capsule's
    half extents at trace work +0x90..+0x98) and `d2` from the end, both
    left on the stack; `fmin(d1, 0.125)` is `0.125 - d1 < 0` picked by
    `fcmovb` on the stack (0x811afc5..0x811afdf); the fraction `(d1 -
    0.125) * recip * sign` (recip at +0x18..+0x20) is never stored.
  - Other sides: the lift `|sphere_z * normal.z|` is one stored float
    (0x811b13c), the distance `(radius + dist) + lift` and both ends'
    dots less it stay on the stack, and so does the enter fraction `(d1
    - 0.125) / (d1 - d2)` (0x811b1dc).
  - The enter fraction rides the stack across every axial side and
    plane and is stored once, into the trace (0x811b37c). The leave
    fraction is stored at each update (0x811b2e7, 0x811b326), the
    compare after a plane's update taking the unrounded quotient
    (0x811b32b).
  - A sweep with no entering side that stayed inside every one sets
    `allsolid`, a fraction of 0 and surface flags (trace +0x10) of 0
    (0x811b3fd..0x811b407).
  vcod's CoD2 kernel (`cod2_lnxded` 0x08059dc0, cod2-d3dbsp-format.md)
  stores each distance and fraction, as its trace A/B reads them. INFERRED.
  `clipmap::trace::Cx::trace_brush_kept` is 1.7's arm, taken with the
  set-up above. Before it, the walk capture's step onto a 0.78 brush
  slope at 70300 landed a few ulps of x off retail's and the walk replay
  never resynced from there; with it `cod4::wire_ab`'s walk matches all
  789 rows (cod4-pmove.md 13.2, 15.1). VERIFIED (the gates).
- `iw3mp.exe` 1.7's `CM_Trace` (0x4f4d30) stores each of these as CoD2
  does: the centre offset, the shifted end and the sweep each go through
  a float (0x4f4e1f, 0x4f4e4c, 0x4f4e65). VERIFIED (the instructions).
  The client's prediction keeps the server's set-up all the same
  (cod4-client-view.md 5.1).
- `iw3mp.exe`'s trace walk: 0x4f4d30 calls the tree walk 0x4f4920
  (recursive), whose leaf is 0x4f40b0; that calls 0x4f3fc0 for the
  brushes (into the leaf brush node walk 0x4f3c40, then 0x4f37e0 per
  brush) and 0x4f2250 for the AABB trees (into 0x4f2000, whose partitions
  go to 0x4efce0, 0x4f05c0 and the border arm 0x4f1ae0). VERIFIED (the
  calls). That 0x4f37e0 is `CM_TraceThroughBrush` and 0x4f05c0 the
  capsule against the triangles is INFERRED (their shape; 0x4f05c0 reads
  the radius and the sphere offset, trace work +0x88 and +0x8c).
- The client's walks keep vcod's float model but for one split. The leaf
  arm 0x4f40b0 runs the brushes (0x4f3fc0) while the fraction is not 0,
  then the AABB trees while it stays so (0x4f40f4). 0x4f3fc0 grows the
  leaf's box by the half box, each bound stored (0x4f3fc7..0x4f400a), and
  culls it through `CM_TraceBox` (0x4f6d00), which stores each `d1`, `d2`
  and fraction and moves the enter and leave fractions on the sign of a
  stored difference (0x4f6d3f..0x4f6dfa), as `Cx::trace_box_culls` does.
  The leaf brush walk 0x4f3c40 stores `t1`, `t2` and the offset `(half +
  0.125) - clearance` (0x4f3cb1..0x4f3cd3), takes `fmin`, `fmax` and `1 -
  frac` off stored differences, `1 / |t2 - t1|` and both fractions stored
  (0x4f3dc2..0x4f3e05), and each point of the split stored
  (0x4f3e54..0x4f3eb2, 0x4f3ef8..0x4f3f50), as `Cx::trace_brush_node`
  does. VERIFIED (the instructions).
- The tree walk 0x4f4920 splits as that walk does on an axial plane, but
  on any other it stores the dot with each end before taking `dist` off
  it (0x4f49b2, 0x4f49d5), and its offset there is not CoD2's 2048: 0.125
  (0x700be8) for a point (trace work +0x84, set at 0x4f511c..0x4f5146 when
  the half box sums to 0), else the stored `+0x9c + 0.125` (0x4f49ee),
  where `+0x9c` is `sqrt(stored(|half box|^2))` stored (0x4f4f11..0x4f4f35,
  through 0x401050). VERIFIED (the instructions). `Cx::trace_tree` takes
  both under `TraceFloats::Cod4Client` (`Sweep::box_radius`). The slack
  only prunes the walk; no take reads a difference. VERIFIED (the gates).
- The client's brush kernel (0x4f37e0) runs at the client's 53-bit
  precision (cod4-player-anims.md 11) and stores as CoD2's does: each axial
  `d1`, `d2`, `fmin(d1, 0.125)`'s difference and each fraction is a float
  (0x4f383a..0x4f3978); each side's `dist` (0x4f39fd), `d1` and `d2`
  (0x4f3a27, 0x4f3a48), `0.125 - d1`, `d1 - d2` and `d1 - 0.125`
  (0x4f3a6d, 0x4f3ab7, 0x4f3abd) and the enter and leave fractions
  (0x4f3ada, 0x4f3b2b) are stored. Unlike CoD2's port it stores each
  side's dot with the start or the end (0x4f3a15, 0x4f3a40) before
  taking `dist` off it, and a sweep all inside zeroes the surface flags
  (trace +0x10, 0x4f3b8f). VERIFIED (the instructions).
  `TraceFloats::Cod4Client` takes it; `ClipMap::trace_client` is the
  client's trace, which the physics worlds' sweep and the ragdolls use
  (cod4-gsc.md 3.6.9).
- Two other differences, INFERRED: `CM_TestBoxInBrush` culls on strict
  bounds where CoD2's accepts a touch (unported for the server; matters
  only for a box exactly touching a brush), and the AABB position walk
  stops at the first child that starts solid (ported in the indexed arm,
  section 3).
- The client's `CM_TestBoxInBrush` (0x4f3140) culls on strict bounds: a
  brush whose maxs is at or under the box's lower bound, or whose mins is
  at or over its upper bound, is passed (0x4f3140..0x4f31b5). Per side it
  stores the dot with the start (0x4f31dc), the lift `|sphere_z n.z|`
  (0x4f31f1, through 0x401000) and `(dist + radius) + lift` (0x4f320f),
  and the box is out when their stored difference is over 0 (0x6ffa44,
  0x4f3223). A box inside every side takes fraction 0, `startsolid`,
  `allsolid`, the brush's contents and surface flags of 0
  (0x4f323e..0x4f3254). VERIFIED (the instructions).
  `Cx::test_brush` takes it under `TraceFloats::Cod4Client`.
- The client's position test (0x4f36c0, `CM_Trace`'s still arm at
  0x4f510f) grows the box by 1, each bound stored (0x4f36da..0x4f3772),
  lists the leaves through 0x4f2c60 (0x400 of them), and per leaf until
  `allsolid` runs the brush walk (0x4f3360: the leaf's box must overlap
  the bounds strictly, 0x4f3373..0x4f339b; the node walk 0x4f3270 as
  `Cx::test_brush_node`), then, unless that left it solid, the AABB trees
  (0x4f22b0). VERIFIED (the instructions). Its triangles: section 3.1.
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
  differently. The client's test is read and ported (section 3.1); the
  server's is not.

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
lands on retail's. VERIFIED (`the_frag_flies_on_ours_as_on_retail`).

The capsule's float model, inlined in the same walk at
0x8116cd1..0x8117aa6. VERIFIED (the disassembly), each item:

- The face. `v0 - v1` is three stored floats; `v0 - v2` stays on the
  stack for the cross product, its stored copy (ebp-0x3c) serving the
  edge tests. `n.x` and `n.y` are one store each; `n.z` is stored and
  kept. The closing speed is `(sweep.y n.y + stored(sweep.x n.x)) +
  sweep.z n.z`, compared unrounded and then stored (0x8116d52..0x8116d85).
  `Vec3NormalizeTo` (0x8192a64) returns the length in `st(0)`, stored as
  the face's scale (0x8116d9d).
- The sphere's z (the start's, less or plus the end sphere's offset by
  the stored normal's z sign, `fcmovnb` at 0x8116dd5) stays on the stack
  and is stored for the edge kernel's argument. The lead point `start -
  r n - v0` and its gap along `n` are kept. The fraction is `(scale *
  -stored(gap - 0.125)) / closing`, one product and one quotient off the
  stored scale and closing speed, rounded once (0x8116e67..0x8116e8c).
  vcod computed `-((gap - 0.125) / closing) * scale` in `f32`, which
  left mp_crash's terrain z a bit off.
- A face contact past the plane already (gap under 0) measures from
  its foot on the plane, kept; a centre within a radius gives fraction 0
  and `startsolid` (0x811729f..0x8117324).
- The edge coordinates. `s = v01 . m` (`m = sweep x foot`, kept) stores
  each partial sum (0x8116ee7..0x8116f0e) and is compared unrounded;
  `t = v02 . m` off the stored `v02` is kept. `m` goes through three
  floats across the first edge's call (0x811754a..0x8117571), `t`
  through one across the second's (0x8116f6c..0x8116f79), and the third
  test is `closing > t - stored(s)` (0x8117226..0x8117236).
- The edge kernel (`CM_TraceSphereThroughEdge`, 0x81150ae) returns 0 for
  a strike written into the trace, 1 for none, 2 and 3 for past the
  first and the second end. The sphere's offset from the edge's start is
  three stored floats; `along x sweep` is kept, its squared length
  stored after each partial sum; the reach `0.125 + r` is kept and
  stored; the half chord `sqrt(room * len2) / across2` is stored
  (0x81151f1); the entry, the edge's squared length (stored) and the
  point's offset from the line are kept; the normal is that offset times
  `1 / stored reach`, x off the kept offset, y and z off their stores
  (0x81152e3..0x811530b). From inside the cylinder the normal is
  `Vec3NormalizeTo` of the stored offset and `startsolid` is `len2 *
  (r^2 across2 - stored(off^2)) > stored(mid)^2` (0x8115405..0x8115440).
- The corner (0x8117354..0x81176f9, 0x8117a06..0x8117aa6). The offset,
  closing speed, squared distance and discriminant are kept; the graze
  test is `b^2 * 0.001 > disc` (0.001 at 0x8209640). The fraction is
  `(-sqrt(disc) - b) / |sweep|^2` (trace work +0x34), kept. The normal's
  three components are each stored, times `1 / (0.125 + r)` kept, then
  times the Newton step's `(3 - |k|^2) * 0.5` (3.0 at 0x8209658, 0.5 at
  0x8209628), x off its kept product and y, z off their stores
  (0x81176c2..0x8117707). A sphere within reach takes fraction 0 and
  the offset times `1 / sqrt(dist2)`.

`mesh4::capsule_tri4`, `sweep_edge` and `vertex4` take this model.
Before it, `cod4_moves_ab`'s walk had its z a bit off on the terrain on
24 rows and a zero vz's sign on 11, the mantle capture's walk-off z on 7,
and `placeSpawnPoint`'s drop stopped a bit under retail's; with it all
of them match retail. VERIFIED (the gates). The border's own corner arm
(0x811633c on) and the position test (`segment_tri_dist2`) are still
plain `f32`.

The client's capsule (`iw3mp.exe` 0x4f05c0) runs the same tests in the
same order but stores nearly every value, at its 53-bit precision.
VERIFIED (the instructions), each item:

- `v0 - v1` and `v0 - v2` are six stored floats (0x4f05f6..0x4f062c);
  `n`'s three components are one store each (0x4f0650, 0x4f066a,
  0x4f0674). The closing speed `(sweep.y n.y + n.x sweep.x) + sweep.z
  n.z` is stored, then compared (0x4f0691..0x4f06a0).
- The normal is 0x55f6e0's: the squared length `(x^2 + y^2) + z^2`
  stored, its root through 0x401050 stored, `1 / len` stored, each
  component one store; the stored length is the face's scale (0x4f06b3).
- The sphere's z is stored (0x4f06ee); the lead point `sphere - r n` is
  three stores (0x4f0714..0x4f0734), less `v0` three more
  (0x4f073f..0x4f0755); the gap `(n.y l.y + n.x l.x) + l.z n.z` is
  stored (0x4f0779). The fraction is `-(((gap - 0.125) scale) /
  closing)`, rounded once (0x4f085a..0x4f0871).
- Past the plane: `sphere - v0` stored, the distance stored (0x4f07d9),
  the test `stored(r^2) > d^2` (0x4f07e1..0x4f07fc), the foot three
  stores (0x4f082a..0x4f0842).
- `m = sweep x foot` is three stores (0x4f08ad, 0x4f08bf, 0x4f08cd);
  `s = (m.y e01.y + m.x e01.x) + m.z e01.z` and `t` likewise off `v0 -
  v2` are stored before their compares (0x4f08ed, 0x4f098c); the third
  test is `closing > t - s` (0x4f0a30..0x4f0a43). The face contact's
  fraction goes through 0x4093c0 against 0 (0x4f0b18).
- The edge kernel 0x4eff70 and the corner 0x4f0370 get the stored
  sphere (calls at 0x4f0919, 0x4f09bf, 0x4f0a81, 0x4f0b4d).

`mesh4::capsule_tri_client` takes this model. With the server's model
instead, the shipment take's step 25 sweep put a bottle a z ulp off
retail's and the watermelon's step 27 sweep a chunk an ulp of x; with it
both match retail on every step. VERIFIED (`phys_ab`, cod4-gsc.md
3.6.9). Those takes ran with the server's edge and corner kernels, before
the client's below were ported, and match with either. VERIFIED (the
runs, ours).

The client's edge kernel (0x4eff70) runs the server's tests in the same
order and returns the same codes (0 struck, 1 none, 2 and 3 past an end),
storing nearly every value. VERIFIED (the instructions), each item:

- The sphere's offset from the edge's start is three stores
  (0x4eff7e..0x4eff9b), `across = sweep x along` three more
  (0x4effad..0x4effcf); the offset along it `(across.y o.y + across.x o.x)
  + across.z o.z` and `|across|^2` are one store each (0x4efff7,
  0x4f0007). The reach `r + 0.125` is stored (0x4f0017), `off^2` kept as
  a double (0x4f0025), `reach^2` stored and the room `reach^2 across2 -
  off^2` stored (0x4f002f..0x4f003d), none when it is at most 0.
- `|along|^2` is stored (0x4f0071), `room len2` stored into 0x401050's
  root (`sqrt` of the float, stored), the half chord `root / across2`
  stored (0x4f0096). `w = offset x along` is three stores (0x4f00b4,
  0x4f00cb, 0x4f00da), `mid_scaled = across . w` one (0x4f00fa); `mid =
  mid_scaled / across2`, `half + mid` before its test and the entry `mid -
  half` (0x4f014c) are each stored.
- From inside (an entry under 0): `s = -(offset . along)` is stored
  (0x4f01a6), `k = s / len2` stored, the foot `offset + k along` through
  0x403ae0 (each component `along k + offset`, one store); the foot's dot
  with the sweep is stored before its test against 0 (0x4f0220); the
  normal is 0x55f6e0's of the foot (0x4f0240), and `startsolid` is `len2
  stored(r^2 across2 - off^2) > mid_scaled^2` (0x4f0250..0x4f0270).
- From outside: the entry point `offset + enter sweep` is three stores
  (0x4f02a1..0x4f02bb), `s` stored (0x4f02e1), `k` stored, the foot
  through 0x403ae0, `1 / reach` stored (0x4f0336); the normal is its
  three products with the foot, one store each (0x4f0344..0x4f0354), the
  fraction the stored entry (0x4f035b).

The client's corner kernel (0x4f0370; the triangle's and the border's,
`walkable` its argument) stores as the edge's does. VERIFIED (the
instructions), each item:

- The offset from the corner is three stores (0x4f037c..0x4f0390), the
  closing speed and the squared distance one each (0x4f03bd, 0x4f03e8);
  the reach `r + 0.125` is kept and the gap `dist2 - reach^2` stored
  (0x4f0406).
- Within reach: 0x401050's root of the distance, `1 / root` stored
  (0x4f042d), the normal three stored products, fraction 0, `walkable`
  (0x4f0452), and `startsolid` when `stored(r^2) > dist2`
  (0x4f0469..0x4f047e).
- Else `b^2` is stored (0x4f0496) and the discriminant `b^2 - gap
  |sweep|^2` (0x4f04aa); the graze test is `b^2 0.001 > disc` (0.001 a
  float widened, 0x70af98); the fraction `(-root - b) / |sweep|^2` is
  stored (0x4f04dd). The normal: `offset + f sweep` three stores
  (0x4f04fd..0x4f0515), times the stored `1 / reach` (0x4f0528) three
  more, its squared length stored and `(3 - len2) 0.5` stored (0x4f0577;
  3.0 at 0x70add8, 0.5 at 0x70ace8), times it three more.

The client's point sweep against a triangle (0x4efce0, which the AABB
walk 0x4f2000 calls for a point at 0x4f20c0) stores what the server
keeps: `v0 - v1` and `v0 - v2` are six stores (0x4efd09..0x4efd43), `n`
three (0x4efd67, 0x4efd81, 0x4efd91), the closing speed one (0x4efdbe),
`v0 - start` three (0x4efdd7..0x4efdeb), the depth one (0x4efe11), `m =
sweep x (v0 - start)` three (0x4efe45..0x4efe65), `s` and `t` one each
(0x4efe8d, 0x4efecd); the third test is `s - t < closing` off the stored
two (0x4efee0..0x4efeeb), the normal 0x55f6e0's of `n` (0x4efef8) and the
fraction the stored depth over the stored closing speed (0x4efeff..0x4eff09).
VERIFIED (the instructions).

The client's border arm (0x4f1ae0, per border at 0x4f2153 when the sweep
moves sideways and the capsule has a length, 0x4f2118..0x4f2139) runs
`CM_TraceCapsuleThroughBorder`'s tests with 1.7's end spheres (section
2). Where it parts from vcod's CoD2 kernel, VERIFIED (the instructions):

- The distance off the line stores the dot `start.x a + start.y b`, then
  its difference with `c` (0x4f1b32, 0x4f1b3d); the CoD2 kernel keeps
  the dot.
- A corner within reach clears `walkable` (0x4f1ca8, 0x4f1e5e) and sets
  `startsolid` when `stored(r^2) > c0` (0x4f1cbc..0x4f1cc4).
- The far end's distance `start + length` is kept (0x4f1d6c), and its
  height less the start's, `start.z - stored(slope length + base)`,
  stored (0x4f1e0a..0x4f1e1e).
- The wall top's height over the hit, `(slope along + base) - hit.z`, is
  one store (0x4f1f13..0x4f1f21); the CoD2 kernel stores the height
  first. A top within `sphere_z` of it is met by the straight part, which
  clears `walkable` (0x4f1fb4); above by less than `stored(sphere_z + r)`
  (0x4f1f48) the upper sphere, below by less than `stored(-sphere_z - r)`
  (0x4f1f8a) the lower, each through 0x4f19f0 (0x4f1fa4).

0x4f19f0 builds the top's two ends, each component rounded once off kept
products and the far end's kept `start + length` (0x4f1a0d..0x4f1a55),
sweeps the sphere at the edge between them through 0x4eff70 (0x4f1a99),
then, past an end, at that end through 0x4f0370 with `walkable` clear
(0x4f1aaa, 0x4f1ac7). VERIFIED (the instructions).

The client's `CM_CullBox` (0x4efb10, called at 0x4f2013 by the AABB
trace walk and at 0x4f2193) stores each axial bound `half + half_sweep`
(0x4efb64, 0x4efb90, 0x4efbb6) and each cross bound (0x4efc06, 0x4efc4d,
0x4efc94) before its compare; vcod's CoD2 kernel keeps them. VERIFIED
(the instructions).

`mesh4`'s `point_tri_client`, `sweep_edge_client`, `vertex_client`,
`capsule_border_client` and `border_sphere_client`, and `Cx::cull_box`
under `TraceFloats::Cod4Client`, take this model; `mesh4`'s tests pin an
input per arm where the two models part. VERIFIED (the tests, ours). The
edges take (cod4-gsc.md 3.6.9) tells the edge kernel apart: the sweep
that catches each of its two shards on a terrain ridge ends on 0x4eff70,
and with the server's edge kernel (`sweep_edge`) in the client's trace
both part from retail's, by an ulp of the fraction and the normal's low
bits. VERIFIED (the run, ours, the swap on a lab build). No take tells
the other four apart: the phys gates (bit-exact) and the ragdoll gates
pass with them, and the sweeps takes read retail's with or without them.
The fall take's contacts are brushes; the roof take's 126 hits are all
triangles, and there the client's capsule kernel (0x4f05c0) gives
retail's where the server's parts on 94. VERIFIED (the runs, ours).

`tools/re/cod4_kernel_vectors.py` runs the five in `tools/re/x87emu.py`:
the edge 0x4eff70, the corner 0x4f0370, the point 0x4efce0, the border
0x4f1ae0 and the border sphere 0x4f19f0, with the CRT root 0x68aec0 taken
as one 53-bit `fsqrt` (0x68aefc). It keeps one generated input per path
through their compares: 12, 6, 9, 92 and 28, the border's and the border
sphere's through the edge and corner calls they make. 40 contacts each
follow. `mesh4::vectors` reads every case back from the client's port to
the bit (`crates/cod4/tests/fixtures/clip/kernels-1.7-client.txt`): none
parts. The server's kernel in each one's place parts on 42, 36, 38, 59
and 49 of 52, 46, 49, 132 and 68 cases. Every compare in the five goes
both ways in the cases but the border corner's `0 < f` (0x4f1d2f,
0x4f1ed3), false only for a start exactly at the reach, and 0x55f6e0's
zero length. VERIFIED (the vectors).

### 3.1 The client's position test

`iw3mp.exe`'s AABB position walk (0x4f2180, from 0x4f22b0 per tree root
whose material meets the mask) culls each node through its `CM_CullBox`
(0x4efb10), recurses on the children until one sets `startsolid` (trace
+0x29, 0x4f21d3), and at a leaf stamps the partition (0x4f21fc) and runs
0x4f1930 per triangle until one does (0x4f2239). 0x4f22b0 then takes the
root's material flags once `allsolid` (+0x28) is set (0x4f2300).
VERIFIED (the instructions).

0x4f1930 picks the test by the sphere offset (trace work +0x8c, compared
with 0 at 0x4f1935): VERIFIED (the instructions), each item:

- With an offset, the capsule's axis from `stored(sphere_z + start.z)` to
  `stored(start.z - sphere_z)` goes to 0x4f1400 with `stored(r^2)`
  (0x4f1942..0x4f1994).
- With none (a cube's box), the squared distance 0x4f0f90 returns is
  compared with `stored(r^2)`, solid when under it (0x4f19b0..0x4f19d6).
- Solid is fraction 0 and `allsolid`, `startsolid` set (0x4f19a0..0x4f19a9).

0x4f0f90 takes `e1 = v1 - v0`, `e2 = v2 - v0` stored, their three dots
stored, calls 0x4f0b70 for a point, and returns the stored `(x^2 + y^2) +
z^2` of that point less the start, each component stored
(0x4f0fb8..0x4f10cf). VERIFIED (the instructions).

0x4f0b70 is the barycentric region search of a closest point on a
triangle, with each dot, `det`, `s` and `t` stored and the region sums
kept (0x4f0b77..0x4f0c1b), but its offset is `p - v0` (0x4f0b77..0x4f0b8e)
where the regions it then picks need `v0 - p`: it returns the point
nearest `2 v0 - p`. VERIFIED (the instructions, run on chosen inputs by
`tools/re/x87emu.py`: 10 over the middle of a 10-unit triangle with v0 at
the origin, `(2, 3, 5)`, it returns v0, squared distance 38). So a
sphere's position test is solid only near v0, or where the reflection
lands near the triangle. Which bodies retail tests this way, and that a
take shows it, is open (section 6).

0x4f1400, the capsule's axis `top..bottom` against the triangle,
VERIFIED (the instructions), each item:

- `n = e1 x e2`, each component stored; the plane's `v0 . n`, `top . n`
  and `bottom . n` stored and the two distances off them kept
  (0x4f142a..0x4f1540).
- Their product under 0 (0x4f1548, 0.0 at 0x70adf0; 0 itself is not a
  crossing) is a crossing: the point at `stored(dt / (dt - db))` along
  the axis, each component stored (0x4f1606..0x4f1645), goes to 0x4f0ec0,
  and inside the triangle is a hit (0x4f166d).
- Otherwise, or past the triangle, an end whose squared distance is
  under `stored(|n|^2) r^2` is near (0x4f1673..0x4f16d9,
  0x4f16e6..0x4f174f); with no crossing and no near end there is no hit
  (0x4f1758). A near end inside the triangle is a hit, the top first
  (0x4f17ec..0x4f1866).
- Then the axis against the edges `v0 e1`, `v0 e2` and `v1 (v2 - v1)`,
  through 0x4f10e0, each a hit when `stored(r^2)` is over the distance
  (0x4f1874..0x4f191a).

0x4f0ec0 takes `v0 - q` stored and its dots with the edges stored, `s =
d2 e12 - d1 e22` and `t = e12 d1 - d2 e11` kept against the kept `det =
e22 e11 - e12^2` 0x4f1400 passes: inside when `s >= 0`, `t >= 0` and
`s + t <= det` (0x4f0f3d..0x4f0f7a). VERIFIED (the instructions).

0x4f10e0 is the squared distance between two segments by the region of
its `s, t` square: `a, b, c, d, e, f` stored (0x4f10e7..0x4f11a1), `det`
and both numerators kept and tested with no parallel case
(0x4f11c9..0x4f130e), each region's quadratic kept and stored once.
VERIFIED (the instructions).

`clipmap::pos4` is this test, and `Cx::position_indexed_r` takes it under
`TraceFloats::Cod4Client`. `tools/re/cod4_postest_vectors.py` runs the
four kernels 0x4f0f90, 0x4f10e0, 0x4f1400 and 0x4f1930 in
`tools/re/x87emu.py` (an i386 and x87 interpreter at 53-bit precision over
objdump's listing) on 469 generated inputs, keeping one per path through
their compares; `pos4`'s test reads each answer back from the port to
the bit (`crates/cod4/tests/fixtures/clip/postest-1.7-client.txt`).
VERIFIED (the vectors).

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

- 1.7 addresses for the INFERRED facts above that still carry none
  (`CM_Trace` is 0x811ce5c, section 2).
- The float model of the border arm (`CM_TraceSphereThroughBorder`,
  0x811626a: its ends, then its own corner tests, `fsqrt` at 0x8116423,
  0x811653a, 0x8116640, 0x81166d3) and of the position test's walk
  (0x811546a). The trace's AABB tree walk is 0x811673a (recursive,
  0x20-byte nodes, `childCount` at +0x1a; the `triIndices` and `verts`
  globals 0x889dea4 and 0x889de9c). VERIFIED (the disassembly). No gate
  shows a difference there yet.
- `CM_TestBoxInBrush`'s strict bounds (section 2).
- The server's tree walk split on a non-axial plane: whether 1.7 keeps
  CoD2's 2048 or takes the client's box radius (section 2) is unread.
- The client's sphere position test (section 3.1) meets the point
  nearest `2 v0 - p`. `Clip::sweep` reaches the position test for a phys
  body that has not moved while its state is 0 or 1, with a cube's box
  when the body is thin (`ode::phys`, 0x5a6930); the fall take's ragdoll
  boxes are all cubes. VERIFIED (the port's sweep, the take). No take
  has such a test over a triangle away from its v0, so the
  retail-visible effect is not measured.
- A take whose sweep ends on the corner, the border or the border sphere,
  or a point sweep (`Ragdoll_BoneTrace`) on a triangle. A 0.001 box
  dropped straight over a terrain vertex meets a face at its corner
  (0x4f05c0's edge coordinates read 0 there, so the contact is inside),
  and 4000 port drops of a turned shard over mp_crash's ridges and peaks
  met no corner: the sphere has to pass just outside the faces' outline
  along the sweep. A cube's capsule has no length, so the border arm
  never runs for a phys or ragdoll sweep (0x4f2118..0x4f2139). VERIFIED
  (the runs, ours).
- The server's pmove and script traces run over this clip map
  (cod4-pmove.md 15).
