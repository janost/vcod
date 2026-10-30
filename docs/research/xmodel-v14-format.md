# CoD 1 xmodel format (version 14)

Byte layouts of the three files that make up a CoD 1 model, verified against the retail kar98k viewmodel (`xmodel/viewmodel_kar98k`) and hands (`xmodel/viewmodel_hands_new`), and cross-checked against the cod-asset-importer project. All six files (descriptor, parts, surfs for gun and hands) parse to exact EOF with these layouts. The parser is `crates/common/src/xmodel.rs`; `crates/common/src/skeleton.rs` grafts several models into one skeleton and builds the inverse binds. Where this document and the parser disagree, the parser is right. Everything here is VERIFIED unless labelled otherwise. All integers are little-endian; `cstr` is a NUL-terminated string.

## The three files

| pk3 entry | Holds |
|---|---|
| `xmodel/<name>` | descriptor: bounds, LOD slots, collision mesh, texture list per LOD |
| `xmodelparts/<lod>` | bone hierarchy and bind pose |
| `xmodelsurfs/<lod>` | surfaces: triangle strips and vertices in bone space |

The descriptor's first present LOD name selects the parts entry, and every present LOD names its own surfs entry (section "LOD selection"). Texture filenames in the descriptor resolve under `skins/` (`skins/viewmodel@woodk98.dds`, `skins/viewhands@default.jpg`); the formats are dds, jpg and tga.

## `xmodel/<name>` (descriptor)

```
u16 version                 // 14
f32 mins[3], maxs[3]        // bounds (kar98k: +-36.2, hands: +-23.7)
3 x { f32 dist; cstr name } // LOD slots; empty name = unused
i32 collision_lod           // -1 when the model has no collision mesh
u32 surf_count
surf_count x {              // collision surfaces, see "Collision block"
  u32 tri_count
  tri_count x { f32 plane[4]; f32 svec[4]; f32 tvec[4] }
  f32 mins[3], maxs[3]      // bone space
  i32 bone; u32 contents; u32 surf_flags
}
per present LOD: u16 tex_count; tex_count x cstr   // texture filenames, one per surface
```

- Surface `i` of a LOD uses texture `i`; the parser refuses a model whose surface count differs from its texture count.
- kar98k: one LOD `viewmodel_kar982`, five textures `viewmodel@woodk98.dds, viewmodel@K98.dds, viewmodel@K98.dds, viewmodel@stockviewk98.dds, metal@k98clip.dds`. Hands: one LOD `viewmodel_hands_new4`, four times `viewhands@default.jpg`.
- The LOD name's last character encodes the model type: `'0'` rigid, `'1'` animated, `'2'` viewmodel, `'3'` playerbody, `'4'` viewhands. The parser reads it off the LOD name it is given.
- `viewhands@default.jpg` is a 4x4 white placeholder; the engine substitutes the real skin per surface at runtime. Surfaces 0 and 1 of the hands are the left and right hand meshes, whose UVs match the `viewhands@hand.dds` atlas; surfaces 2 and 3 are the forearm sleeves, whose UVs match the 512x512 `viewhands@vsleeve_<nation>` textures. vcod substitutes `viewhands@hand.dds` and `viewhands@vsleeve_whermact.tga` when, and only when, a model's materials are exactly the four placeholders.

### Collision block

The layout is CoD2's `XModelCollSurf_s` / `XModelCollTri_s` unchanged. VERIFIED on every descriptor in the stock paks (1018 models; each parses to exact EOF and every reconstructed vertex but 9 of 544k lies inside its surface bounds within 0.5).

- `collision_lod` is -1 on the 624 models without collision, which then have `surf_count` 0. Otherwise it names the LOD the mesh was built from; 308 use LOD 0.
- A triangle is a unit `plane` (`n·p = d`) and two barycentric edge planes: `u = svec·p - svec.w`, `v = tvec·p - tvec.w`, the point is inside when `u >= 0`, `v >= 0`, `u + v <= 1`. The vertices are the solutions of the 3x3 system `{n, svec.xyz, tvec.xyz} · p = {d, u + svec.w, v + tvec.w}` at (u,v) = (0,0), (1,0), (0,1); `coll_tri_verts` in `xmodel.rs` solves it in f64. The stored normal equals `normalize(cross(v2 - v0, v1 - v0))` (181k of 181k non-degenerate triangles), so the parser emits (v0, v2, v1). A crate floor decodes to its two exact halves.
- Coordinates and the per-surface `mins`/`maxs` are in `bone`'s space. Rigid props use bone 0 (1161 of 1449 surfaces); vehicles and artillery spread surfaces across their bones, and only the bone world bind (`pos = bone.rot * p + bone.pos`, as for render vertices) puts a tank's tracks on the ground instead of 23 units under it.
- `contents` values seen: `0x1` (solid, 1418 surfaces), `0x0` (288: tree canopies, hanging signs), `0x10` (112, on 38 models: lamp and headlight glass; meaning not verified). A tree is one solid trunk surface plus canopy surfaces at 0, which is why the player walks through the canopy in retail. vcod collides on the solid bit only.
- `surf_flags` values seen: `0xd00000` (942), `0x1500000` (247, trunks and crates), `0xd04000`, `0x4000` (canopies), `0x900000` (glass), `0x400000`, `0xb00000`, `0x4010`, `0x0`. Not decoded; they look like the BSP material surface flags and are kept on `CollSurf` for whoever needs them.
- The render mesh is far denser: `barrel_high0` has 372 triangles, its collision surface 28.
- Stock MP maps place 7168 `misc_model`s over 235 models; 2700 placements (184 models) carry collision. The 51 placed models without (`bookrow`, `grasstuft`, `bottle_wine`, `doorknobcrystal`, `boxhedge`) are passable in retail too. `spawnflags` on those entities is only ever 0 or 2, mostly on winter trees; its meaning is unknown and vcod ignores it (INFERRED to be collision-unrelated, not verified).

## `xmodelparts/<lod>`

```
u16 version                 // 14
u16 num_bones               // non-root
u16 num_root_bones          // root bones have identity local transform and parent -1
num_bones x { i8 parent; f32 pos[3]; i16 rot[3] }
(num_root + num_bones) x { cstr name; f32 hit_mins[3]; f32 hit_maxs[3] }   // root bones first
(num_root + num_bones) x u8 hit_location
```

- The 24 bytes after each bone name are the bone's hit box in its own frame, and the trailing byte per bone is its hit-location index into the 19 names of `cod11-combat.md` section 3. The engine's locational trace slab-tests those boxes on the posed skeleton; an all-zero box is how the artist turns a bone off. Verified across the corpus: all 725 non-empty `xmodelparts` entries in `pak0-6` parse to exact EOF under this layout and the trailing byte takes only values 0..18.
- `xmodelparts/USAirborne3`, the LOD0 of all 17 `playerbody_*` models: `bip01 l thigh`'s box is `[-2.07 -6.69 -5.04 .. 17.41 7.49 4.97]` at hit location 13 (`left_leg_upper`), and its child `bip01 l calf` sits at local `(17.51, 0, 0)`, so the box runs down `+X` to the next joint. The body's own `bip01 head` and `bip01 neck` boxes are all-zero, as is every `tag_*`; the head boxes live in the attached head model, where `xmodelparts/basehead21` gives `bip01 head` `[-0.29 -4.90 -4.11 .. 9.09 5.63 4.16]` at code 2. `tag_weapon_left` and `tag_weapon_right` carry code 18, `gun`.
- Rotation: `q_xyz = rot / 32768.0`, `q_w = sqrt(max(0, 1 - x^2 - y^2 - z^2))`. The clamp is required: the largest observed `|xyz|^2` in the shipped data is 1.00003.
- Parents always precede children, so world transforms compose in file order: `world_pos = parent.world_pos + parent.world_rot * local_pos`, `world_rot = parent.world_rot * local_rot`. The parser keeps both the local and the composed world transform per bone; `skeleton.rs` inverts the world bind (`Mat4::from_rotation_translation(rot, pos).inverse()`) to skin against.
- kar98k gun: 6 bones, root `tag_weapon`, then `kar98`, `tag_brass`, `tag_flash`, `k98 clip` (with the space, which is also the anim track name), `kar98_bolt`. Hands: 55 bones, root `tag_view`, containing `tag_view > tag_torso > tag_weapon`.
- Viewhands quirk (model type `'4'`): the stored bind positions of hand bones are placeholders the engine re-poses at runtime. Before composing worlds, the parser zeroes the local position (rotation kept) of every bone named `tag_view`, `tag_torso` or `tag_weapon`, starting with `bip01 `, containing `webbing`, or starting with `r cuff` or `r wrist`. That covers all 55 observed hands bones. The gun (type `'2'`) needs no adjustment. Consequence for animation: every shipped `viewmodel_*` model has all-zero bind local positions, see the translation-key gotcha in `xanim-v14-format.md`.

## `xmodelsurfs/<lod>`

```
u16 version                 // 14
u16 surface_count
per surface:
  skip 1
  u16 vertex_count; u16 triangle_count
  skip 2
  u16 default_bone          // 65535 = rigged (per-vertex bones), then skip 4
  packed triangle strips until triangle_count triangles are emitted
  vertex_count x {
    f32 normal[3]; f32 uv[2]
    if rigged: u16 weight_count; u16 bone
    f32 pos[3]              // in that bone's space
    if weight_count != 0: f32 primary_influence
  }
  per vertex, weight_count x { u16 bone; skip 12; f32 influence }   // extra weights, second pass over all vertices
```

- Triangle strips: each packet is `u8 count` followed by `count` u16 indices. The first three form a triangle emitted as `(i3, i2, i1)`; each further index forms a triangle with the two before it, alternating winding, and any triangle with a repeated index is dropped. `decode_tri_strip` in `xmodel.rs` is the exact port; decoding stops once `triangle_count` triangles are out.
- Unrigged surfaces (`default_bone != 65535`) have no per-vertex bone or weight fields; every vertex belongs to `default_bone`.
- The 12 bytes inside each extra weight are the vertex's position in that weight's bone space. On `xmodelparts/USAirborne3`, whose bind is real, `bone.rot * pos + bone.pos` through each extra weight lands within 1e-4 (median, 0.18 worst over 732 weights) of the same point through the primary bone. Influences across primary and extras sum to 1.0 (max deviation 3e-8). Observed `weight_count` histogram on the hands: 0 for most vertices, 1 for many, 2 for a few, so four weight slots suffice. The parser keeps the four largest influences per vertex, renormalized.
- Viewhands blend against a bind the file does not store. With the zeroed placeholder positions (see the viewhands quirk above) the weights of one `viewmodel_hands_new4` vertex disagree by 1.8 units at the median and 12.1 at worst (`bip01 r upperarm` against `bip01 r forearm`), so skinning against that bind drags every blended vertex toward its secondary bone's origin: the sleeves read as a broad slab reaching toward screen centre and the fingers split off the hands. The per-weight positions do imply one: over every vertex that pairs two bones, `primary.rot * primary_pos - other.rot * other_pos`, which is the offset between the two bones' bind positions, is constant per pair to 0.003 (standard deviation, all 1014 extra weights). `recover_viewhands_bind` in `xmodel.rs` places each bone that shares a vertex at the offset the mesh gives it from a neighbour, and the mesh is baked and skinned against that bind; the animation still starts from the zeroed locals. That retail skins the hands this way is INFERRED: its hands draw with narrow sleeves and whole fingers, which only a skin that honours the per-weight positions produces.
- Bake: `pos = bone.world_rot * pos + bone.world_pos`, `normal = bone.world_rot * normal` through the vertex's primary bone.
- UVs are used raw, with no v-flip: the engine's v-down convention matches the BSP pipeline, which already samples unflipped. The reference importer flips for Blender's v-up; do not copy that. Gun UVs legitimately run outside 0..1 (range -0.99..1.98), so sample with wrap.
- Totals: gun 5 surfaces, 1125 vertices, 1001 triangles; hands 4 surfaces, 1914 vertices, 2924 triangles. Every baked vertex lies inside the descriptor bounds within 0.5.

## LOD selection

All CoDMP.exe 1.1. Each claim carries its own label.

### Loading

- VERIFIED, the descriptor reader 0x48b020: the three LOD slots are `{f32 dist; cstr name}` pairs copied raw into a parse buffer of `0x404`-byte records (name, then dist at `+0x400`).
- VERIFIED, the loader 0x48b290: per slot it copies the name, stores the dist at `xmodel+4 + i*0x14` and the name pointer at `+8 + i*0x14`, and increments the LOD count at `xmodel+0x64` (a short). INFERRED, off the `je 0x48b53c` on an empty name at 0x48b414: the count stops at the first empty slot. The stock paks have no gap (1012 descriptors, VERIFIED by a census).
- VERIFIED: 0x48b290 loads `xmodelparts` once, from LOD 0's name (0x48ac80 with `xmodel+8`, the string `xmodelparts/%s` at 0x48ac80's body), then `xmodelsurfs` once per present LOD (0x48aac0 into `+0xc + i*0x14`). INFERRED, off the `goto` to the free path: a LOD whose surfs fail to load fails the whole model.
- VERIFIED (asset census, `pak0`-`pak6`): 710 of the 719 coarse LODs ship no `xmodelparts` entry at all (`USAirborneA_LOD13`, `Blake0`, `lowpoly0`, `k98_lod_med0`), so a coarse LOD's vertex bone ids index LOD 0's bones. vcod bakes every LOD against LOD 0's bind (`model::Model::lods`).
- VERIFIED (census): 546 of 1012 descriptors have every dist 0, each with a single LOD, and so always draw LOD 0; 41 of the 43 `*viewmodel*` descriptors are among them (`viewmodel_ammo` has one LOD at 5000, `vehicle_tank_t34_viewmodel` `1000 3000`). Common tables: heads `400 800 3000` (134), player bodies `800 1200 0` (70), world weapons `150 500 1000` (18), `crate_misc1a` `512 0`.

### The per-frame inputs

- VERIFIED, `RE_RenderScene` (0x4e9cb0), at 0x4e9f6b..0x4ea02d: the view's `lodBias` is `r_lodbias->value` (cvar pointer 0x16c3750) and its `lodScale` is `tan(min(fov_x, fov_y) * pi/360) / tan(0.6981317)` times `r_lodscale->value` (0x16c37a4). The constants are the float pi/360 at 0x56904c and the double 0.6981317 (40 degrees) at 0x569190; `fov_x`/`fov_y` are the refdef's (0x16c5648, 0x16c564c). The min is an `fcomp` of the two with `test ah,0x41; jp` picking `fov_y` only when `fov_x` is larger. The pair lands at viewParms `+0x140`/`+0x144`, which `R_RenderView` (0x4d5310) copies to `tr.viewParms` at 0x16c5350, so 0x16c5490 / 0x16c5494.
- So at `cg_fov 80` across 4:3 (64.4 tall) a table is read at 0.75 of the true distance, a scope's zoom reads it nearer still, and `r_lodscale 0` reads every model at distance `r_lodbias`.
- VERIFIED, registration (0x4b3300): `r_lodbias` "0" and `r_lodscale` "1", flags 1. Model Detail writes them (`ui/options_performance.menu`, `cod11-front-end.md`): Aggressive 4 / -200, Low 2 / -100, Normal 1 / 0, High 0.5 / 0, Off 0 / 0.

### The pick

- VERIFIED, 0x4ea0c0 (called with the refEntity in `esi`): `lodDist = lodScale * |viewOrigin - ent.origin| + lodBias`, divided by the float at refEntity `+0x40` when that is non-zero. `viewOrigin` is `tr.viewParms`' first vector (0x16c5350); `ent.origin` is refEntity `+0x44`.
- VERIFIED, 0x48bd70: walks the model's LODs in order and returns the first whose dist is 0 or greater than `lodDist` (`fcomp` then `test ah,0x5; jnp`, so strictly greater); past the last it returns -1. There is no hysteresis and no per-entity state.
- VERIFIED, 0x505260 (a single-model refEntity, `reType` 2): returns without drawing on -1. 0x50e3e0 (a DObj, `reType` 1: players, corpses, script models) computes one `lodDist` for the entity and picks per attached model with that model's own table (0x50e448..0x50e46b). INFERRED, off 0x482330's `jl` past a negative pick: a part past its last LOD contributes no surfaces while the rest of the entity draws, so a distant player keeps his body (last dist 0) and loses his head past 3000 and his rifle past 1000.
- VERIFIED: these two are the only callers of 0x4ea0c0 and 0x48bd70, so every xmodel draw goes through the pick. Static models (`misc_model`) reach it as refEntities: the cell walk at 0x50b4c4..0x50b50f pushes each cell's static-model records through `RE_AddRefEntityToScene` (0x4e9a00, which copies 0x9c bytes). INFERRED: the refEntity `+0x40` read as a divisor is the static model's scale, which 0x504a20 sets to the largest `modelscale_vec` axis (0x42dea0) and leaves 0 when all three are 1 (0x504b55). vcod reads entities with no scale.
- Cheat overrides, VERIFIED, not ported: `r_highLodDist` "-1", `r_mediumLodDist` "0", `r_lowLodDist` "0" and `r_lodViewDist` "0" (flags 0x200, 0x4b3300). `R_RenderView` copies them to 0xa9ce18/0xa9ce2c/0xa9ce40 and 0xa9ce10 each frame; a non-negative `r_highLodDist` swaps every model's table for theirs, and a positive `r_lodViewDist` replaces `lodDist`.

### vcod

- `model::LodView` and `model::select_lod` are the two functions above. The renderer picks each entity part's LOD at the start of `Renderer::render` from the instance's origin and draws nothing for one past its last LOD; `vc_shadows`' entity casters are the drawn instances, so they agree. Props bake every LOD into the map buffers (`props::Props::ranges` carries the LOD) and the gather keeps the picked one; the static `vc_shadows` layer casts LOD 0, since it is cached across views.
- The viewmodel is a DObj in retail too. INFERRED: it sits at the eye, so its `lodDist` is about `r_lodbias` and a stock gun or hands model (one LOD at 0, or `viewmodel_ammo` at 5000) never drops; vcod's viewmodel pass draws LOD 0. Model Detail Aggressive's -200 only lowers the distance.
- Collision is untouched: the collision mesh is the descriptor's own, and hit boxes ride LOD 0's `xmodelparts`, shared by every LOD.

## Weapon files

`weapons/mp/<name>` is `\key\value\key\value...` text. The leading backslash is optional, and the shipped files open with a bare `WEAPONFILE` token before the first key, which would shift every pair by one if kept. `kar98k_mp` names `gunModel viewmodel_kar98k` and `handModel viewmodel_hands_new`.

## Model space and the view basis

Viewmodels are authored around `tag_view` at the eye with X forward, Y left, Z up; tags in general point +X forward, which is why the muzzle direction is `tag_flash`'s rotation applied to `+X`. View space is X right, Y up, -Z forward, so the remap in `crates/client/src/viewmodel.rs` sends model +X to view -Z, model +Y to view -X and model +Z to view +Y. That matrix has determinant +1: it is a rotation, not a reflection, so the models' one-sided winding survives it.

The gun hangs off the hands' `tag_weapon` whatever its own root is called. The viewmodel setup in `cgame_mp_x86.dll` (1.1) at `0x30034cf0` builds a two-model list from the weapon def's strings at `+0x14` and then `+0x10`, each prefixed `xmodel/`, and stores the literal `"tag_weapon"` in the second entry beside its model handle (VERIFIED). The module's weapon field table names `+0x14` `handModel` and `+0x10` `gunModel` (VERIFIED, table entries at `0x30075588` and `0x3007557C`). That the literal is the tag the gun is attached at is INFERRED from the slot it sits in. The gun's root name varies: `viewmodel_kar98k`'s is `tag_weapon`, `viewmodel_m1carbine`'s is `tag_origin` (VERIFIED, `xmodelparts`). `crates/client/src/viewmodel.rs` grafts the gun with an explicit `tag_weapon`, which is how a `tag_torso`-only ADS anim moves either gun; grafting by root name left the carbine as a separate root at the eye. Elsewhere a graft without a tag aliases a later model's root to the base bone of the same name, and a root name the base skeleton lacks becomes a new root with a warning.

## The view fov

Retail draws the viewmodel with the world's own fov. vcod did not: it drew the world at a vertical 75 degrees and the gun at a fixed vertical 65, which put the gun at retail's size at the hip of a 4:3 window and left it unzoomed down the sight.

- `cg_fov` defaults to `"80"` (cvar table entry `0x300749e0` in `cgame_mp_x86.dll` 1.1, vmCvar at `0x30298c60`, value at `0x30298c68`).
- The function at `0x30032e20` returns the view fov. Its constants are 90.0 (`0x3006947c`), 80.0 (`0x30069574`), 160.0 (`0x30069480`) and 55.0 (`0x300695a4`), and it reads the held weapon def's `+0x2cc` (`aimDownSight`), `+0x218` (`adsZoomFov`), `+0x21c` (`adsZoomInFrac`) and `+0x220` (`adsZoomOutFrac`) (field table entries `0x30075c48`, `0x30075dc8`, `0x30075dd4`, `0x30075de0`).
- INFERRED, off its branches: 90 when `0x30207160` is 5 (`pm_type` at intermission, also INFERRED), else `cg_fov` clamped to 80..160; for a weapon with `aimDownSight`, `adsZoomFov` at a sight fraction (`0x30207214`) of exactly 1, and between 0 and 1 `t = frac - (1 - X)` with `X` the in-frac when the flag at `0x30209458` is set and the out-frac otherwise, the fov moving `t / X` of the way to `adsZoomFov` once `t > 0`. So the zoom runs over the last `X` of the fraction only, and the carbine (`adsZoomInFrac 0.42`) does not start zooming until the sight is 58% up. Last, 55 whenever `eFlags & 0xc000` (`0x302071dc`, the mounted-gun bits), overriding everything before it. The flag at `0x30209458` is the sight direction the crosshair reads (`cod11-hud-protocol.md`, "Crosshair").
- INFERRED, off the `fptan`/`fpatan` sequence at `0x30032f40`: the refdef keeps that value as `fov_x` (`0x3020958c`) and stores `fov_y = atan2(height, width / tan(fov_x / 2))` (`0x30209590`), from the refdef's integer width `0x30209584` and height `0x30209588`; the two scale constants are pi/360 (`0x3006946c`, 0.0087266) and 360/pi (`0x3006967c`, 114.59). So `cg_fov` is horizontal across the whole view and a wider window loses height: 80 across 4:3 is 64.4 tall, across 16:10 55.3.
- The first-person weapon is added at `0x30036cf0` through trap `0x3d` with renderfx `0x8c` and no fov of its own. INFERRED: `0x8c` is Q3's `RF_FIRST_PERSON | RF_DEPTHHACK | RF_LIGHTING_ORIGIN`.
- `CoDMP.exe` builds the view projection at `0x4d3cb0` from the refdef's `fov_x` (`0x16c5648`) and `fov_y` (`0x16c564c`) into `0x16c5498`, and a second one at `0x16c54d8` with a `rep movs` of those 16 floats; the only differing element is `[14]`, -2 times `r_znear` (default `"4"`) in the first and -2 times `r_znear_depthhack` (default `"0.1"`) in the second (the stores at `0x4d3dbc` and `0x4d3dd2`). INFERRED, off the order: the depth-hack store follows the copy, so the two matrices share their fov. The backend at `0x4d6710` loads the matrix at `0x16d8cd0` as `GL_PROJECTION` for a surface whose entity flags carry `0x8` and sets the depth range to 0..0.2, restoring `0x16d8c90` and 0..1 after (INFERRED from the branch; that `0x16d8cd0` is the backend's copy of `0x16c54d8` is INFERRED from its sitting 0x40 past the main matrix's copy).

`weapon::view_fov_x` in `crates/common/src/weapon.rs` is the rule above, `camera::fov_y` the refdef's derivation, and the renderer draws the viewmodel with the world's fov, near plane 0.1 and far 500, in the viewport depth range 0..0.2 (`VM_DEPTH_RANGE` in `crates/client/src/renderer.rs`), retail's.

vcod's own, not retail's: with `vc_widescreen 1` (the default) a viewport wider than 4:3 widens that `fov_x` to keep the vertical fov a 4:3 view of it would have (`camera::view_fov_x`, Hor+), world and viewmodel alike; the mouse's zoom scale still reads the unwidened fovs. `vc_widescreen 0` keeps `fov_x`, as above (cod11-hud-protocol.md section 10).
