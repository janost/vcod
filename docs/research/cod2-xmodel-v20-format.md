# CoD2 xmodel format (version 20)

The three files of a Call of Duty 2 1.3 model, written as a diff against `xmodel-v14-format.md` (CoD1) and its parser `crates/common/src/xmodel.rs`. A Rust loader for v20 can be written from this document alone; where a section says "as v14" the v14 document has the detail.

Sources. The Linux 1.3 dedicated server `cod2_lnxded` is the oracle; its loader functions are unnamed in the symbol map, so each is given by address with the Mac 1.3 MP binary's symbol beside it. The Mac build is a release build with full symbols and STABS struct layouts (`XModel`, `XModelParts_s`, `XSurface_s`, `XVertexInfo_s`, `XBlendInfo_s`, `XBoneInfo_s`, `XModelCollSurf_s`, `XModelLodInfo`); field names below are its. The layouts were then checked against every model in the CoD2 1.3 install's `main/iw_*.iwd` and `localized_english_iw*.iwd` (later archive wins): 1533 `xmodel`, 1197 `xmodelparts` and 2654 `xmodelsurfs` entries, all parsed to exact EOF with no failure (VERIFIED, census script in my scratch directory, not committed). All integers are little-endian; `cstr` is NUL-terminated.

vcod reads these files with `vcod_cod2::xmodel` (raw records) and bakes them into the neutral `vcod_common::model::Model` with `vcod_cod2::model::V20` (positions through the base pose below, collision bounds grown and contents masked as the loader does), rebuilding a view rig's bind as "Viewmodels and hands" says. It keeps the vertex colour (`model::Vertex::color`, white on v14) and the tangent frame (`tangent`, `binormal`, turned by the base pose as the normal is; zero on v14).

## What changed from v14

| | v14 (CoD1) | v20 (CoD2) |
|---|---|---|
| version | 14 | 20 (`0x14`), all three files |
| descriptor header | bounds | `u8 flags`, then bounds |
| LOD slots | 3 | 4 |
| per-LOD list | texture filenames under `skins/` | material names (`materials/<name>`) |
| bone hit boxes | in `xmodelparts`, after each name | in the descriptor, after the material lists |
| parent | `i8` | `u8` (absolute parent index) |
| quaternion scale | `/32768`, float `w` | `/32767`, integer `w` |
| surface header | `skip 1, nv, nt, skip 2, bone, skip 4` | `u8 tileMode, nv, nt, i16 bone, [u16 blend count]` |
| triangles | packed strips before the vertices | plain `u16` list after the vertices |
| vertex | normal, uv, pos (+ weights in a second pass) | normal, RGBA colour, uv, binormal, tangent, pos, weights inline |
| weights | `f32` influences | primary `u8`/256, extras `u16`/65536 |
| bone names | `bip01 *` | `j_*` (and `tag_*` as before) |

Collision surfaces are unchanged (VERIFIED below). xanim stays v14 (section "xanim").

## `xmodel/<name>` (descriptor)

Readers: Linux 0x080c685c (header and LOD slots), 0x080c69d0 (collision), 0x080c6cea (material lists, parts, bone infos); all three are inline in the Mac `XModelLoad` 0xbb616.

```
u16 version                   // 20
u8  flags                     // XModel.flags (+0x8c)
f32 mins[3], maxs[3]          // XModel.mins/maxs (+0x64/+0x70)
4 x { f32 dist; cstr name }   // LOD slots; empty name = unused
i32 coll_lod                  // stored as a short at +0x7e; -1 = no collision mesh
i32 num_coll_surfs
num_coll_surfs x {            // XModelCollSurf_s, 0x2c bytes in memory
  i32 num_tris
  num_tris x { f32 plane[4]; f32 svec[4]; f32 tvec[4] }   // XModelCollTri_s, 48 bytes
  f32 mins[3], maxs[3]
  i32 bone; i32 contents; i32 surf_flags
}
per LOD slot with a non-empty name: u16 count; count x cstr material
num_bones x { f32 mins[3]; f32 maxs[3] }   // XBoneInfo_s bounds; num_bones from LOD 0's xmodelparts
```

- VERIFIED (census): the file ends exactly after the bone-info block in all 1533 descriptors, with `num_bones` the root plus child count of the `xmodelparts` named by LOD 0. So a descriptor cannot be parsed to EOF without its parts.
- VERIFIED (Linux 0x080c685c; Mac 0xbb616): the loop runs four times, and `coll_lod` is read with the integer reader 0x080c79bc after the fourth name.
- INFERRED (Mac 0xbb616, the per-slot loop): the LOD count `XModel.numLods` (+0x7c) counts non-empty names, but material lists are read per slot, skipping empty ones, and the surfs loop stops at the first empty name. VERIFIED (census): no stock descriptor has a gap, so a loader may stop at the first empty name. LOD counts: 614 one, 229 two, 317 three, 373 four.
- VERIFIED (census): `flags` is 0 on 1501 descriptors and 1 on 32, every one a grass or shrub prop (`brush_newgrass_squareclump`, `brush_desertshrubgraytall`, `caen_weeds_01`). INFERRED (Mac `R_LoadEntities` 0xe2b6c, its only reader through `XModelGetFlags` 0xb6796): bit 0 lets a `misc_model` with a `gndLt` key take its lighting from that ground colour (`R_GetStaticModelLightingFromGround`) instead of the light grid. It is client-only.

### LOD distances

- INFERRED (Mac `XModelGetLodForDist` 0xb6890, its branches): the pick returns the first LOD whose dist is 0 or greater than the distance, -1 past the last, the same rule as CoD1's 0x48bd70 (`xmodel-v14-format.md`, "The pick"). `g_testLods` (`XModelSetTestLods` 0xb6842) overrides the table per slot.
- VERIFIED (census): 531 descriptors have the single table `0`. Common tables: `160 400 1000 0` (141, including 29 of the 32 `playerbody_*` and 36 of the 46 `head_*`), `800 1400 1800 0` (83), `700 0` (56), `256 512 0` (34). A last LOD at 0 never drops.
- VERIFIED (census): 28 of the 1982 coarse LOD names have their own `xmodelparts` entry; the loader never reads them (it reads parts once, from LOD 0's name, Linux 0x080c6c66), so every LOD's vertex bone indices index LOD 0's bones, as in v14.

### Collision block

- VERIFIED (census): byte layout as v14. Every one of 539455 triangles decodes through the v14 three-plane solve, and its stored normal equals `normalize(cross(v2 - v0, v1 - v0))`, as in v14, so `coll_tri_verts` and the (v0, v2, v1) emit carry over unchanged.
- VERIFIED (Linux 0x080c69d0, the float 0.001 at 0x081507a0 loaded at 0x080c6b84; Mac the same 0.001 at 0x326210): each surface's `mins` is lowered and `maxs` raised by 0.001 on load.
- VERIFIED (same two functions): `contents` is stored as `contents & 0xdffffffb`, and `XModel.contents` (+0x5c) is the OR of every surface's stored contents.
- VERIFIED (census): `coll_lod` is -1 on 677 descriptors, exactly those with no collision surface, 0 on 482, 1 on 243, 2 on 128, 3 on 3. Contents in the file: `0x1` (1509 surfaces), `0x2` (181), `0x10` (33), `0x4` (17, stored as 0), `0x20000010` (12, stored as `0x10`), `0x20030200` (7), `0x31680` (2). 904 of 1761 surfaces use bone 0; the rest need the bone's world bind, as in v14.

### Material lists

- VERIFIED (census): every LOD's material count equals its `xmodelsurfs` surface count, and surface `i` uses material `i`. Names are bare (`mtl_us_rangers`, `mtl_weapon_kar98`, `mtl_hands_german`).
- VERIFIED (Mac `R_LoadXSkins` 0xd4248): each name is copied, lowercased and passed to `Material_RegisterHandle`. VERIFIED (Linux 0x080c6cea): the dedicated server reads the lists too, interning each name with `SL_GetString(name, 0, 8)`, and then never registers a material.
- INFERRED (Mac `XModelLoad`, the branch to the "File conflict (between non-iwd and iwd file)" message): a surfs file whose surface count differs from the descriptor's list fails the model.

### Bone infos (the hit boxes)

- VERIFIED (Linux 0x080c6cea; Mac 0xbb616, `XBoneInfo_s` is `bounds[2][3], offset[3], radiusSquared`, 40 bytes): per bone the loader keeps the two corners as read, `offset = (mins + maxs) / 2` and `radiusSquared = |maxs - offset|^2`, into `XModel.boneInfo` (+0x60).
- INFERRED (Linux `DObjTraceline` 0x080bb27e, the `!= 0.0` test on `+0x24`; Mac 0x76600): a bone whose `radiusSquared` is 0 is skipped, so an all-zero box turns a bone off, as in v14. INFERRED (same functions, the sphere test before the slab test): the trace first tests the segment against the sphere at `offset` in the posed bone, then slab-tests the box in the bone's frame, so the boxes are in bone space as in v14.
- VERIFIED (census): 5555 of 14332 bone infos are all-zero.
- What moved: v14 kept these 24 bytes per bone in `xmodelparts`. v20 keeps them in the descriptor, so two descriptors sharing one `xmodelparts` (1533 descriptors over 1197 parts entries) can carry different boxes.
- VERIFIED (census), `playerbody_american_normandy01` (LOD 0 `us_ranger_pvtA_8k3`, 76 bones): `pelvis [-3.90 -6.48 -8.24 .. 2.41 6.48 8.24]` hit location 5, `j_knee_le [-1.78 -3.68 -3.00 .. 16.14 3.68 3.00]` 15, `head [-0.06 -4.39 -3.51 .. 8.71 4.39 3.51]` 2, `neck` 3, `back_mid` and `back_up` 5. `j_head` itself is all-zero; the body's box sits on the helper bone `head` under it. Head models (`head_british_boon`) carry their own `j_head` box at 2, `j_neck` at 3 and a `j_spine4` box at hit location 0.

## `xmodelparts/<lod0 name>`

Readers: Linux 0x080c63c4 (file), 0x080c613c (one quaternion), 0x080c621e (base pose); Mac inline in `XModelLoad` 0xbb616.

```
u16 version                   // 20
i16 num_child                 // non-root bones
i16 num_root                  // root bones: identity rotation, zero translation, no parent
num_child x { u8 parent; f32 trans[3]; i16 quat[3] }    // 19 bytes
(num_root + num_child) x cstr name                       // roots first
(num_root + num_child) x u8 classification              // XModelParts_s.partClassification
```

- INFERRED (Linux 0x080c63c4, Mac 0xbb616, the `< 0x80` branch): the total must be under 128, or the model fails with "has more than %d bones" (127). VERIFIED (census): the largest is 96.
- VERIFIED (census): no hit boxes follow the names; the names run straight into the classification bytes, and the file ends there in all 1197 entries.
- `parent` is the parent's absolute bone index. VERIFIED (Linux 0x080c63c4): the engine stores `bone_index - parent` in its hierarchy array. VERIFIED (census): every parent precedes its child.
- Root counts: 1 on 1527 models, 2 on 4, 5 on 1, 77 on 1 (VERIFIED, census). 1125 descriptors' root bone has an empty name.

### Quaternions

- VERIFIED (Linux 0x080c613c: the integer 0x3fff0001 = 32767^2, the float 0.5 at 0x081505e0, `floor`): `w = 0` when `0x3fff0001 - (x^2 + y^2 + z^2) < 1`, else `w = floor(sqrt(that) + 0.5)` as an `i16`. The four shorts are then scaled by the float 1/32767 (3.0518509e-05, Linux 0x081505e4 in 0x080c621e; Mac 0x32621c). v14's 1/32768 and float `w` do not carry over. VERIFIED (census): 370 child bones have `x^2 + y^2 + z^2 > 32767^2` and so take `w = 0`.

### The base pose

- VERIFIED (Linux 0x080c621e, Mac 0xbb616): each root's `DObjAnimMat` is quaternion (0,0,0,1), translation 0 and `transWeight` 2.0.
- INFERRED (same functions, the per-child loop): children are composed in file order. The world quaternion is `QuatMultiply(local, parent_world)`, which by its body (Mac `QuatMultiply` 0x2b504) is the Hamilton product `parent_world * local`, the same composition v14 uses. It is not normalized: `transWeight = 2 / |q|^2` (2.0 when `|q|^2` is 0), and every later rotation by that quaternion builds the standard matrix with `s = transWeight` in place of 2. The world translation is `parent_world` rotated by that matrix applied to `trans`, plus `parent_world.trans`.
- VERIFIED (census): this pose bakes the stock vertices inside the descriptor bounds (0.5 slack) for all but 8 of the distinct LOD 0 meshes: the two mk2 frag grenades, the colt45 and tokarev viewmodels, the flak viewmodel, one damaged bunker door and two ivy props.

### Viewmodels and hands

- VERIFIED (census): the LOD 0 name's last character still runs `0`..`4` (1125, 328, 40, 32 and 8 descriptors). All 40 type-`2` and all 8 type-`4` models store every child translation as zero, so the CoD1 viewhands problem is the file's here too: blending against the stored bind is wrong. On the `viewmodel_hands_*` meshes (and 51 vertices of `viewmodel_panzerschreck`) an extra weight's bone-local position lands 1 unit or more away from the primary's. The per-weight offsets give the real bind, as in v14 (`recover_viewhands_bind`). INFERRED: the engine reads nothing off that character; its type rule is the exporter's naming, which vcod already reads.
- VERIFIED (`crates/cod2/tests/view_rigs.rs`): `vcod_cod2::model::recover_bind` places each bone that shares a vertex at the offset the per-weight positions give it from a neighbour, as v14's `recover_viewhands_bind` does, and every type `2` and `4` model bakes and skins against that bind. The worst disagreement between a vertex's weights falls from 12.09 units (17.74 on `viewmodel_hands_british_bare`) to under 0.005 on the hands and from 8.49 to 0.006 on the panzerschreck; the other guns read 0 before and after. The server's bone load (`load_bones`) keeps the stored bind.

### Bone names and tags

- VERIFIED (census): every stock skeleton uses `j_*` joints (8502 names) and `tag_*` tags (3663); `bip01` appears in no `xmodelparts` entry and in 2 of 3669 xanims. Player bodies root at `tag_origin > j_mainroot > pelvis`, and carry `j_spine4`, `j_neck > neck > j_head > head`, `j_shoulder_le/ri`, `j_wrist_le/ri`, and `tag_weapon_left/right` under the wrists. Heads and helmets root at `j_spine4`, `j_head` or `j_helmet`. Viewmodel guns root at `j_gun` (`viewmodel_kar98k`: `j_gun j_bolt j_clip j_round j_stripper tag_brass tag_flash`); hands root at `tag_view`, with `tag_torso`, `tag_weapon`, `tag_camera` and `tag_cambone` under it. No stock model has an upper-case bone name or two bones with one name.
- INFERRED (Mac `XModelGetBoneIndex` 0x45a5c, its loop): a tag resolves by its interned string id, scanning from the last bone down, so the highest-numbered bone with that name wins and a miss returns -1.
- vcod's CoD1 code hardcodes `bip01 *` and `tag_weapon` names (combat, poses, viewmodel graft); those need a CoD2 table.

### Hit locations

- VERIFIED (Linux string table at 0x08187bc0, read through its pointers): the 19 names in order are `none helmet head neck torso_upper torso_lower right_arm_upper left_arm_upper right_arm_lower left_arm_lower right_hand left_hand right_leg_upper left_leg_upper right_leg_lower left_leg_lower right_foot left_foot gun`, the same as CoD1's (`cod11-combat.md` section 3). VERIFIED (census): classification bytes take only 0..18.
- INFERRED (Linux `DObjTraceline` 0x080bb27e, Mac 0x76600): the trace indexes its priority map with the classification, and a priority of 1 takes the parent bone's location (a root takes the bone it is attached to), the v14 inheritance `xmodel.rs` already models. A bone at priority 0 is never scored, since the walk starts with a threshold of 2.
- VERIFIED (census): the player body differs from CoD1's: `tag_weapon_left` and `tag_weapon_right` classify as `left_hand` and `right_hand` (11, 10), not `gun`, and their boxes are zero.

## `xmodelsurfs/<lod name>`

Readers: Mac `XModelLoad` 0xbb616 for the header, `XModelReadSurface` 0xc7318 per surface. The Linux server never reads one (section "What the server loads").

```
u16 version                   // 20
u16 num_surfs                 // must equal the LOD's material count
num_surfs x surface
```

Surface:

```
u8  tile_mode                 // XSurface_s.tileMode
u16 vert_count
u16 tri_count
i16 bone                      // -1: skinned; else every vertex rides this bone
if bone == -1: u16 blend_count        // total extra weights in this surface
vert_count x vertex
tri_count x { u16 i0, i1, i2 }
```

Vertex (file order; `XVertexInfo_s` holds it in 64 bytes):

```
f32 normal[3]
u8  color[4]                  // RGBA
f32 u, v                      // texCoordX, texCoordY
f32 binormal[3]
f32 tangent[3]
if bone == -1:
  u8  num_weights             // extra weights, 0..3
  u16 bone                    // primary bone
  f32 offset[3]               // position in the primary bone's space
  if num_weights > 0:
    u8 primary_weight         // /256
    num_weights x { u16 bone; f32 offset[3]; u16 weight }   // weight /65536; XBlendInfo_s
else:
  f32 offset[3]               // position in the surface bone's space
```

- VERIFIED (census): all 2654 entries end exactly after their last surface's triangles; `blend_count` equals the sum of `num_weights` on every skinned surface; every vertex and surface bone is inside LOD 0's bone count, and every triangle index inside `vert_count`.
- VERIFIED (Mac `XModelReadSurface`, the stores to `boneOffset`): a bone index is stored as `bone << 6`, a byte offset into 64-byte skinning matrices, and every bone a surface touches sets its bit in `XModelSurfs_s.partBits`.
- INFERRED (Mac `XModelReadSurface`, the parity branch after the index loop): an odd triangle count is padded in memory with one degenerate triangle repeating the last index. That is a buffer detail; the file holds `tri_count` triangles.

### Weights

- VERIFIED (Mac `R_SkinXSurfaceSkinned` 0xd1b58, the floats 1/256 at 0x32617c and 1/65536 at 0x32640c): the skinned position is the primary bone's transform of `offset` times `primary_weight/256`, plus each extra bone's transform of its own `offset` times `weight/65536`. The normal goes through the primary bone alone. INFERRED (same function, the `num_weights == 0` branch): a vertex with no extra weight takes its primary at 1.
- VERIFIED (census): `primary_weight * 256 + sum(weight) = 65536` exactly on all 474039 vertices with extra weights. Extra-weight counts: 0 on 575922 vertices, 1 on 293848, 2 on 123282, 3 on 56909. So four influences per vertex is exact and needs no renormalizing.
- As v14, each weight carries its own bone-local position. VERIFIED (census): an extra weight's position under the base pose lands within 0.01 of the primary's for 629976 weights, within 1 for 68391 and 1 or more away for 12772, nearly all of those on the viewhands.

### Tangent frame, colour, UVs

- VERIFIED (census): `|normal · tangent| <= 0.001` everywhere. `cross(normal, tangent) · binormal` is positive on 2366079 vertices and negative on 1605478, so the handedness is per vertex and must be read, not derived.
- VERIFIED (census): 7050 distinct colours; (255,255,255,255) on 1962058 vertices, greys with alpha 255 on most of the rest, and alpha 0 on 16492.
- VERIFIED (census): `tile_mode` is 0 on 4187 surfaces, all with UVs inside 0..1; 1 on 247, all slightly outside (-0.05..1.05) on one axis or both; 2 on 1949, mostly well outside (some by more than 1e6, garbage UVs on surfaces whose material presumably does not sample them). That 0 is clamp and 2 is wrap is INFERRED from those ranges; I found no reader in the renderer functions I checked. Sample with wrap, as for v14, until a material says otherwise.
- UVs are used raw, no v-flip, as v14.

### Winding

- VERIFIED (census, rigid and skinned surfaces baked through the base pose): for 3348578 of 3387883 non-degenerate triangles `cross(p1 - p0, p2 - p0)` points against the summed vertex normals. For comparison, the triangles vcod's v14 strip decoder emits point with the normals (367655 of 373063 on the CoD 1.1 install's rigid surfaces). So a v20 loader that hands the same pipeline `(i0, i2, i1)` matches what v14's decoder hands it.

## What the server loads

- VERIFIED (Linux): `XModelPrecache` (0x080c5840, Mac 0x458b0) calls the loader 0x080c7586 (Mac `XModelLoad`), which loads the descriptor and its parts through 0x080c6cea and calls 0x080c742c (Mac `XModelAllowLoadMesh`). 0x080c742c returns 0, and the surfs reader 0x080c7436 is a stub returning 0. INFERRED (0x080c7586's branch): the surfs loader 0x080c74ea runs only when 0x080c742c returns non-zero, so on the dedicated server it never runs. So the server reads the header, LOD names and distances, collision surfaces, material names (interned only), parts, base pose and bone infos, and never a vertex.
- VERIFIED (Linux 0x080c5840): a model that fails to load prints "Cannot find xmodel" and falls back to 0x080c5714, which allocates an empty 0x90-byte `XModel` under that name, rather than failing the caller.
- So the server's hit rig is the parts (hierarchy, base pose, classification) plus the descriptor's bone infos, and its prop collision is the descriptor's collision surfaces. vcod-server can load CoD2 models without decoding a surfs file.

## xanim

- VERIFIED (Linux `XAnimLoadFile` 0x080c3f8a, the "out of date (version %d, expecting %d)" call passes 0xe; Mac 0xba204): CoD2 xanims are version 14. vcod's `xanim::parse` decodes all 3669 stock CoD2 xanims to exact EOF (multi-game design, 2026-10-21).
- VERIFIED (census): 3480 of them name `j_*` bones and 3605 `tag_*`. Tracks bind to model bones by name, so a CoD2 clip on a CoD2 skeleton needs no remap; a CoD1 clip on a CoD2 skeleton would bind nothing.
- VERIFIED (Linux 0x080c3e62, the notetrack reader, and the string `end` at 0x08150448): each notetrack's `u16` frame is stored as `frame / (n - 1)`, where `n` is the header's frame count plus 1 for a looping clip (flag bit 0) and the result is 0 when `n - 1` is 0, names are interned with `SL_GetString(name, 0, 3)`, and the reader appends a notetrack `end` at 1.0 after the file's own. vcod parses notetracks for EOF only; a consumer has to add the `end`.
