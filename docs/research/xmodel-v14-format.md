# CoD 1 xmodel format (version 14)

Byte layouts of the three files that make up a CoD 1 model, verified against the retail kar98k viewmodel (`xmodel/viewmodel_kar98k`) and hands (`xmodel/viewmodel_hands_new`), and cross-checked against the cod-asset-importer project. All six files (descriptor, parts, surfs for gun and hands) parse to exact EOF with these layouts. The parser is `crates/common/src/xmodel.rs`; `crates/common/src/skeleton.rs` grafts several models into one skeleton and builds the inverse binds. Where this document and the parser disagree, the parser is right. Everything here is VERIFIED unless labelled otherwise. All integers are little-endian; `cstr` is a NUL-terminated string.

## The three files

| pk3 entry | Holds |
|---|---|
| `xmodel/<name>` | descriptor: bounds, LOD slots, collision data, texture list per LOD |
| `xmodelparts/<lod>` | bone hierarchy and bind pose |
| `xmodelsurfs/<lod>` | surfaces: triangle strips and vertices in bone space |

The descriptor's first present LOD name selects the parts and surfs entries. Texture filenames in the descriptor resolve under `skins/` (`skins/viewmodel@woodk98.dds`, `skins/viewhands@default.jpg`); the formats are dds, jpg and tga.

## `xmodel/<name>` (descriptor)

```
u16 version                 // 14
f32 mins[3], maxs[3]        // bounds (kar98k: +-36.2, hands: +-23.7)
3 x { f32 dist; cstr name } // LOD slots; empty name = unused
i32 collision_lod           // skipped
u32 pad_count
pad_count x { u32 sub; skip sub * 48 + 36 }   // collision data, skipped
per present LOD: u16 tex_count; tex_count x cstr   // texture filenames, one per surface
```

- Surface `i` of a LOD uses texture `i`; the parser refuses a model whose surface count differs from its texture count.
- kar98k: one LOD `viewmodel_kar982`, five textures `viewmodel@woodk98.dds, viewmodel@K98.dds, viewmodel@K98.dds, viewmodel@stockviewk98.dds, metal@k98clip.dds`. Hands: one LOD `viewmodel_hands_new4`, four times `viewhands@default.jpg`.
- The LOD name's last character encodes the model type: `'0'` rigid, `'1'` animated, `'2'` viewmodel, `'3'` playerbody, `'4'` viewhands. The parser reads it off the LOD name it is given.
- `viewhands@default.jpg` is a 4x4 white placeholder; the engine substitutes the real skin per surface at runtime. Surfaces 0 and 1 of the hands are the left and right hand meshes, whose UVs match the `viewhands@hand.dds` atlas; surfaces 2 and 3 are the forearm sleeves, whose UVs match the 512x512 `viewhands@vsleeve_<nation>` textures. vcod substitutes `viewhands@hand.dds` and `viewhands@vsleeve_whermact.tga` when, and only when, a model's materials are exactly the four placeholders.

## `xmodelparts/<lod>`

```
u16 version                 // 14
u16 num_bones               // non-root
u16 num_root_bones          // root bones have identity local transform and parent -1
num_bones x { i8 parent; f32 pos[3]; i16 rot[3] }
(num_root + num_bones) x { cstr name; skip 24 }   // root bones first
skip (num_root + num_bones)                       // per-bone u8 table (part group)
```

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
- The 12 skipped bytes inside each extra weight are a per-weight bone-local position, unused. Influences across primary and extras sum to 1.0 (max deviation 3e-8). Observed `weight_count` histogram on the hands: 0 for most vertices, 1 for many, 2 for a few, so four weight slots suffice. The parser keeps the four largest influences per vertex, renormalized; an earlier reading that discarded the extras and skipped the primary influence gave the same bind pose because the primary bone matches the reference importer's output, and blending only matters once the model animates.
- Bake: `pos = bone.world_rot * pos + bone.world_pos`, `normal = bone.world_rot * normal` through the vertex's primary bone.
- UVs are used raw, with no v-flip: the engine's v-down convention matches the BSP pipeline, which already samples unflipped. The reference importer flips for Blender's v-up; do not copy that. Gun UVs legitimately run outside 0..1 (range -0.99..1.98), so sample with wrap.
- Totals: gun 5 surfaces, 1125 vertices, 1001 triangles; hands 4 surfaces, 1914 vertices, 2924 triangles. Every baked vertex lies inside the descriptor bounds within 0.5.

## Weapon files

`weapons/mp/<name>` is `\key\value\key\value...` text. The leading backslash is optional, and the shipped files open with a bare `WEAPONFILE` token before the first key, which would shift every pair by one if kept. `kar98k_mp` names `gunModel viewmodel_kar98k` and `handModel viewmodel_hands_new`.

## Model space and the view basis

Viewmodels are authored around `tag_view` at the eye with X forward, Y left, Z up; tags in general point +X forward, which is why the muzzle direction is `tag_flash`'s rotation applied to `+X`. View space is X right, Y up, -Z forward, so the remap in `crates/client/src/viewmodel.rs` sends model +X to view -Z, model +Y to view -X and model +Z to view +Y. That matrix has determinant +1: it is a rotation, not a reflection, so the models' one-sided winding survives it.

The gun is grafted onto the hands by root bone name: `skeleton.rs` aliases the gun's root `tag_weapon` to the hands' `tag_weapon`, which is how a `tag_torso`-only ADS anim moves the gun. Attachments given an explicit tag graft there instead; a root name the base skeleton lacks becomes a new root with a warning.

## Viewmodel depth pass

The viewmodel is drawn after the world in the same render pass with its own projection (65 degree FOV, near 1, far 500) and the viewport depth range set to `0.0..0.3` (`VM_DEPTH_RANGE` in `crates/client/src/renderer.rs`), then the viewport is restored to `0.0..1.0`. Compressing the viewmodel into the front 30% of the depth range keeps world geometry from clipping it.
