# CoD4 1.7 fastfiles: container, XFile, script strings, asset list, bodies

How a CoD4 1.7 zone (`zone/<language>/*.ff`) is laid out, as
`vcod_cod4::zone` reads it up to its asset bodies (sections 1-8) and
`vcod_cod4::walk` walks every body (sections 9-12).

Sources, each claim naming its own:

- The stock 1.7 zones: the client install's 72 `zone/english/*.ff` and the
  retail dedicated server's 13 `zone/english/*.ff` (byte-identical to the
  client's of the same name). The census gate
  `crates/cod4/tests/zone_census.rs` reads every one (`COD4_DIR`,
  `COD4_LNXDED_DIR`).
- `cod4_lnxded-bin`, the 1.7 Linux dedicated server (md5
  `6b9995fa2e800664644da1c067779414`), stripped. Addresses are its virtual
  addresses (first `LOAD` at file offset 0 maps to 0x08048000).
VERIFIED means read out of a 1.7 zone or the 1.7 server binary; INFERRED
means read off control flow. A claim labelled INFERRED that names no 1.7
address is not yet confirmed on 1.7.

## 1. Container

- Bytes 0..8 are the magic `IWffu100`, bytes 8..12 a u32 LE version, 5 in
  every stock zone. VERIFIED (all 85 zones).
- From byte 12 to the end of the file is one zlib stream (header `78 da`),
  with nothing after its end. VERIFIED (all 85: the census inflates each to
  its end and checks no byte trails).
- `FASTFILE_VERSION` is 5, and the loader refuses any other version with
  "is newer than client executable" or "is out of date". INFERRED.
- The signed magic `IWff0100` passes the magic check but is refused with
  "authenticated file not supported". INFERRED.
  vcod refuses it too.

## 2. XFile

The inflated stream opens with `XFile`, 0x2C bytes, all u32 LE:

| Offset | Field | Meaning |
|---|---|---|
| 0x00 | `size` | inflated bytes after the XFile |
| 0x04 | `externalSize` | bytes loaded from outside the zone (iwd images); a load progress figure |
| 0x08 | `blockSize[9]` | bytes each block takes once loaded |

- Field order and size: INFERRED (`DB_LoadXFileInternal` reads
  `sizeof(file)` then sizes the stream to `file.size`).
- `size` equals the inflated length minus 0x2C. VERIFIED (all 85 zones).
- `externalSize` is read only for the load progress bar. INFERRED.

## 3. Blocks

| Index | Block | Loaded how |
|---|---|---|
| 0 | TEMP | from the stream; its position rewinds on every pop |
| 1 | RUNTIME | zero-filled, never in the stream |
| 2 | LARGE_RUNTIME | deferred: read after every asset body |
| 3 | PHYSICAL_RUNTIME | deferred, as 2 |
| 4 | VIRTUAL | from the stream |
| 5 | LARGE | from the stream |
| 6 | PHYSICAL | from the stream |
| 7 | VERTEX | from the stream |
| 8 | INDEX | from the stream |

- The order and names: INFERRED (`XFileBlock`). The walk
  ending every zone's blocks at their `blockSize` (section 11) is VERIFIED.
- How each block loads: INFERRED (`Load_Stream`:
  block 1 `memset`, blocks 2 and 3 pushed on `g_streamDelayArray`, the
  rest `DB_LoadXFileData`; `Load_DelayStream` reads the
  deferred ones after the asset array).
- `DB_PopStreamPos` restores the position only when it returns to block 0,
  so every other block's position persists across pushes. INFERRED
  (`DB_PopStreamPos`).
- Across all 85 stock zones only blocks 0, 1, 4, 7 and 8 are ever nonzero;
  2, 3, 5 and 6 are 0 everywhere, so no deferred data trails the bodies.
  VERIFIED (the zones' `blockSize`).
- code_post_gfx_mp's `blockSize` is [1092, 0, 0, 0, 473094, 0, 0, 12224,
  1264], ui_mp's [444, 0, 0, 0, 14102419, 0, 0, 0, 0]. VERIFIED.
- Alignment is applied to the block position, `(p + a) & ~a`
  (`DB_AllocStreamPos`), and pads nothing in the
  stream. INFERRED (the stream read is `DB_LoadXFileData` at the aligned
  pointer, `Load_Stream`).

## 4. Pointer tokens

Every pointer slot on disk is a u32 token:

| Token | Meaning |
|---|---|
| 0 | null |
| 0xFFFFFFFF (-1) | the data follows inline, allocated at the block's aligned position |
| 0xFFFFFFFE (-2) | as -1, after reserving a 4-byte alias slot in block 4 (`DB_InsertPointer`) for later tokens to name |
| other `v` | `v - 1` is `block << 28 \| offset`: data already loaded, or (alias kinds) its alias slot |

- Decoding: INFERRED (`DB_ConvertOffsetToPointer` and
  `DB_ConvertOffsetToAlias`: shift 28 and mask 0x0FFFFFFF).
- An offset token can name any block-4 slot a pointer once landed in, not
  only a header slot. INFERRED. Every offset
  token of the 85 zones lands on a slot the walk recorded (section 9),
  VERIFIED.
- The list's two array slots (script strings, assets) are tested only for
  non-null: any other value reads as inline. INFERRED
  (`Load_ScriptStringList`, `DB_LoadXFileInternal`).

## 5. XAssetList and the asset array

After the XFile come 0x10 bytes of `XAssetList`: `stringList.count`,
`stringList.strings` (token), `assetCount`, `assets` (token). INFERRED
(`Load_XAssetListCustom` reads 16 bytes; `DB_LoadXFileInternal` reads them
into 0x1411DFC8, section 9).

Then, all in block 4 (`DB_PushStreamPos(4)` around both):

1. if `strings` is non-null: `count` u32 string tokens, the array aligned
   to 4; then, for each token in order, a -1 token's NUL-terminated string
   inline (alignment 1). A 0 token is a null entry. INFERRED
   (`Load_TempStringArray`, `Load_TempString`, `Load_XStringCustom`).
2. if `assets` is non-null: `assetCount` 8-byte `XAsset {u32 type, u32
   header token}`, aligned to 4, read as one block. INFERRED
   (`Load_XAssetArrayCustom`, a `Load_Stream` of
   `8 * count` before any body).
3. then each asset's body in list order (`Load_XAsset` →
   `Load_XAssetHeader`). INFERRED (same function).

So the list ends, and the first body begins, at
`0x2C + 0x10 + 4 * count + Σ(len + 1) + 8 * assetCount` in the inflated
stream. VERIFIED (all 85 zones: the census checks it, and each zone's first
body opens with a -1 pointer slot: a techset's name in 68 of the client's
72 zones, an xanim's, a localize entry's, a sound's and a comworld's in
one each).

- Script string tokens in stock zones are -1 or 0. VERIFIED (all 85;
  common_mp's first entry is 0, a null string). vcod resolves an offset
  token to the earlier string at that block-4 offset and refuses any other.
- code_post_gfx_mp holds 165 script strings (`tag_origin`, `j_mainroot`,
  ...) and 368 assets; ui_mp 0 and 113. VERIFIED (the census asserts both
  and code_post_gfx_mp's per-type counts below).
- Every header token is -1 or -2, except code_post_gfx_mp's and
  code_post_gfx's one snddriverglobals, whose token is 0x0051C354.
  VERIFIED (all 85).

### Types with no loader

`Load_XAssetHeader` in `cod4_lnxded-bin` (0x08201B18) compares the type
against 1-9, the range 0xA-0xB, 0xC-0x11, 0x13-0x17, 0x19, 0x1A, 0x1F and
0x20, and returns for anything else. VERIFIED (the `cmp edx, imm` chain at
0x08201B18-0x08201C27, which ends `pop ebp; ret`). Types 0 (xmodelpieces),
0x12 (ui_map), 0x18
(snddriverglobals) and 0x1B-0x1E (aitype, mptype, character,
xmodelalias) therefore load no body: their token is never read and no
bytes follow for them. INFERRED (the branch falls through to `ret`).
code_post_gfx_mp's snddriverglobals token being no pointer at all fits.
`AssetType::has_loader` encodes the list.

## 6. XAssetType (1.7)

The engine's name table `g_assetNames` sits at 0x082739E0 in
`cod4_lnxded-bin`: 33 pointers to the strings below, in order. VERIFIED
(the pointers resolved; the strings start at 0x0823E77C with
`xmodelpieces`). The enum column is vcod's
label for each engine name; 1.7 holds no enum names.

| Id | Engine name | Enum |
|---|---|---|
| 0x00 | xmodelpieces | XMODELPIECES |
| 0x01 | physpreset | PHYSPRESET |
| 0x02 | xanim | XANIMPARTS |
| 0x03 | xmodel | XMODEL |
| 0x04 | material | MATERIAL |
| 0x05 | techset | TECHNIQUE_SET |
| 0x06 | image | IMAGE |
| 0x07 | sound | SOUND |
| 0x08 | sndcurve | SOUND_CURVE |
| 0x09 | loaded_sound | LOADED_SOUND |
| 0x0A | col_map_sp | CLIPMAP |
| 0x0B | col_map_mp | CLIPMAP_PVS |
| 0x0C | com_map | COMWORLD |
| 0x0D | game_map_sp | GAMEWORLD_SP |
| 0x0E | game_map_mp | GAMEWORLD_MP |
| 0x0F | map_ents | MAP_ENTS |
| 0x10 | gfx_map | GFXWORLD |
| 0x11 | lightdef | LIGHT_DEF |
| 0x12 | ui_map | UI_MAP |
| 0x13 | font | FONT |
| 0x14 | menufile | MENULIST |
| 0x15 | menu | MENU |
| 0x16 | localize | LOCALIZE_ENTRY |
| 0x17 | weapon | WEAPON |
| 0x18 | snddriverglobals | SNDDRIVER_GLOBALS |
| 0x19 | fx | FX |
| 0x1A | impactfx | IMPACT_FX |
| 0x1B | aitype | AITYPE |
| 0x1C | mptype | MPTYPE |
| 0x1D | character | CHARACTER |
| 0x1E | xmodelalias | XMODELALIAS |
| 0x1F | rawfile | RAWFILE |
| 0x20 | stringtable | STRINGTABLE |

## 7. The census

`cargo test -p vcod-cod4 --test cod4 -- --nocapture zone_census` with
`COD4_DIR` (and optionally `COD4_LNXDED_DIR`) prints every zone's counts.
On the stock 1.7 install, all 85 zones read and walk (section 11).
VERIFIED.

- code_post_gfx_mp: material 200, techset 94, stringtable 30, rawfile 23,
  image 6, xmodel 5, xanim 2, menufile 2, and one each of physpreset,
  sndcurve, snddriverglobals, fx, impactfx, lightdef. VERIFIED.
- ui_mp: material 109, techset 2, menufile 1, rawfile 1. VERIFIED.
- An MP map zone lists one col_map_mp, com_map, game_map_mp and gfx_map,
  no map_ents, one or two materials, 156-238 techsets, 88-371 xmodels,
  146-176 sounds, 123-159 fx, one impactfx. VERIFIED (all 21 MP map zones). An SP map
  lists col_map_sp and game_map_sp instead. VERIFIED.
- Across the 72 client zones: sound 81402, xmodel 14281, localize 10920,
  xanim 10464, techset 9918, fx 6728, rawfile 1987, material 1051, weapon
  615, image 58. VERIFIED.
- No stock zone lists xmodelpieces, ui_map, menu, map_ents, aitype, mptype,
  character or xmodelalias as a top-level asset. VERIFIED.

## 8. What the body walk has to know

- There is no per-asset length. A body is the 32-bit in-memory image of its
  struct, followed by what its pointers inline, in the generated `Load_*`
  order; one wrong struct size desyncs the rest of the zone. INFERRED (the
  `Load_*` functions; section 11's walk ending every zone on its last
  byte bears it out).
- The walk needs block positions, not just a stream offset: alignment
  advances the block position without reading bytes, and an offset token
  names a block position. `Zone::virtual_pos` is block 4's position after
  the list. Block 0's position rewinds on each pop back to it; the others
  persist. INFERRED (section 3).
- Skip the seven loader-less types (section 5) without reading.
- Materials and images are mostly not list entries: a map zone lists one or two
  material and its others ride inline in the xmodels and gfx_map that use
  them (`Load_MaterialHandle` with a -1 or -2 token). VERIFIED for the
  counts (section 7); INFERRED for where they ride.
- Likewise the map's entity string: no zone lists map_ents, so it rides
  inside the clipmap. VERIFIED for the absence; INFERRED for the carrier
  (`clipMap_t.mapEnts`).
- No stock zone has deferred blocks (2, 3) or blocks 5 and 6, so the last
  body ends at the end of the stream (section 3). VERIFIED.

## 9. The body walk

`vcod_cod4::walk::walk` reads every body after the list, keeping all nine
block positions as `DB_SetStreamIndex` does. The 1.7 helpers it mirrors
sit at these addresses in `cod4_lnxded-bin`; the names are
INFERRED (matched by callees and immediates).

| Function | Address |
|---|---|
| `Load_Stream` | 0x082081B4 |
| `DB_PushStreamPos` | 0x08207E34 |
| `DB_PopStreamPos` | 0x08207F0E |
| `DB_AllocStreamPos` | 0x08207E0C |
| `DB_ConvertOffsetToPointer` | 0x08208114 |
| `DB_ConvertOffsetToAlias` | 0x082080EC |
| `Load_XStringCustom` | 0x08208176 |
| `Load_ScriptStringCustom` | 0x08208298 |
| `DB_InsertPointer` (by call pattern only) | 0x08207FCC |
| `DB_InitStreams` | 0x08207D88 |

- `DB_InitStreams` sets every block's position to the block's start.
  VERIFIED (0x08207D88 copies each `XBlock`'s data pointer into the
  position array).
- `DB_LoadXFileInternal` reads the 16-byte `XAssetList` into a static at
  0x1411DFC8, not into a block. VERIFIED (0x081DA451-0x081DA46A pass that
  address and 0x10 to the file read at 0x081DA0F2).
- So the temp block starts at 0, and in every stock zone its `blockSize`
  is 16 bytes past the highest position the walk reaches there. VERIFIED
  (all 85 zones). That the 16 bytes are the linker's own copy of the
  list is INFERRED.
- `Load_Stream` reads into the current block; in block 1 it zero-fills
  and reads nothing. INFERRED (`Load_Stream`). The runtime arrays
  of the map types (cell bits, vis data, dynamic entity poses) take block
  1 space that way. VERIFIED (block 1's `blockSize` in every map zone
  equals the walk's end there).
- A handle (`Load_*Ptr`) pushes block 0; a -1 or -2 token aligns block 0
  to 4, a -2 reserves a 4-byte alias slot in block 4 (aligned 4) before
  the body, and the body loads; an offset token reads the pointer stored
  at its slot. INFERRED (`LoadPtr_*`; `Load_MaterialHandle`
  0x081F845A pushes block 0 and aligns with 3, VERIFIED).
- `stringtable` is the one type whose handle pushes nothing, reserves no
  alias and resolves an offset as a plain pointer. INFERRED
  (`LoadPtr_StringTable`). `Load_StringTablePtr` at 0x081F7202 makes no
  `DB_PushStreamPos` call. VERIFIED.
- Every body's struct lands in block 0 and everything it points to in
  block 4, except where a section below names another block. INFERRED.
  Each type's `Load_*` in section 10 pushes block 4, except
  `Load_StringTable`, whose struct already sits in block 4. VERIFIED (the
  push immediates of those functions).
- A slot the loader only tests for null (most arrays inside a body) reads
  any nonzero token as inline; a slot it also tests for -1 resolves the
  rest as offsets. INFERRED (`if (ptr)` against its
  The pointer type test branches).
- vcod resolves an offset token against: the asset recorded at each slot
  an asset handle landed in (the list's header slots, alias slots, every
  handle field); the struct address of each asset; shared sub-structs by
  address (techniques, shaders, vertex declarations, texture tables,
  water, sound files, speaker maps, rigid vertex lists, the sun light);
  and the start and length of every struct and array loaded. Across all
  85 zones every offset token lands on one of these. VERIFIED (the census
  asserts none is left). On mp_crash, walked with its bulk data kept,
  9579 tokens name data inside another struct's array (a leaf brush
  node's list inside the clipmap's leaf brushes, a cell tree's static
  model list inside an earlier one). VERIFIED. The walk keeps such shared
  data by kind and address (`W::pool`) and copies the named run into the
  struct that points at it; a portal's cell offset becomes an index into
  the cell array, and the clip map's arrays that stored pointers name
  keep their addresses (`ClipAddrs`) so `vcod_cod4::map` can turn those
  pointers into indices (`cod4-world.md`).
- The clip map's `planes` slot is an offset token into the gfx world's
  `dpvsPlanes.planes`, which loads first. VERIFIED (mp_crash).

## 10. Body layouts by type

Sizes are the immediate the type's `Load_*` passes to `Load_Stream` at
the address given. VERIFIED for each one listed. The order of the
children, their alignments (`aN`: `DB_AllocStreamPos(N - 1)`) and the
conditions are INFERRED, item by item, unless
an item says otherwise. That the whole walk with these layouts ends
every stock zone on its last byte, every block at its `blockSize`, is
VERIFIED (section 11).

- **physpreset** 0x2C (`Load_PhysPresetPtr` 0x081ED2E6): name, then
  `sndAliasPrefix` (slot 0x1C).
- **localize** 8 (`Load_LocalizeEntryPtr` 0x081ED0C0): the value string
  (slot 0) loads before the name (slot 4).
- **rawfile** 0xC (`Load_RawFile` 0x081F417A): name; buffer a1 of
  `len + 1` bytes (the text and a NUL).
- **stringtable** 0x10 (`Load_StringTable` 0x081F709A): name;
  `columnCount * rowCount` string slots a4, then each string.
- **lightdef** 0x10 (`Load_GfxLightDef` 0x081F4F04): name; the image
  handle at 4.
- **sndcurve** 0x48 (`Load_SndCurvePtr` 0x081FFDA2): name only; eight
  knots inline.
- **image** 0x24 (`Load_GfxImagePtr` 0x081EC3D6): the name (slot 0x20)
  loads first; then `texture.loadDef` (slot 4) as a handle of its own in
  block 0: a4, -2 reserves an alias slot, then a 0x10-byte head
  (`levelCount`, `flags`, `dimensions[3]`, `format`, `resourceSize`) and
  `resourceSize` pixel bytes. `Load_GfxTextureLoad` at 0x081E63FC loads
  0x10. VERIFIED.
- **loaded_sound** 0x2C (`Load_LoadedSoundPtr` 0x081EC1B0): name; the
  `MssSound` data (slot 0x28) in block 0, a1, -2 aliased, `data_len`
  (0xC) bytes.
- **sound** 0xC (`Load_snd_alias_list_t` 0x08200354): name; `count`
  aliases of 0x5C a4. Each alias (`Load_snd_alias_t` 0x081FFEE6): four
  strings (name, subtitle, secondary, chain); `soundFile` a4, 0xC, whose
  type 1 loads a loaded_sound handle at 4 and otherwise two strings
  (dir, name); the falloff sndcurve handle (slot 0x48); `speakerMap` a4,
  0x198, its name at 4.
- **techset** 0x94 (`Load_MaterialTechniqueSet` 0x081F804E): name; 34
  technique slots at 0xC. A technique: a4, an 8-byte head (name, flags,
  `passCount` at 6) with `passCount` 0x14-byte passes inline after it, so
  one `Load_Stream` of `8 + 0x14 n`; per pass the vertex declaration
  (a4, 0x64), the vertex shader and the pixel shader (a4, 0x10: name,
  then `programSize` (u16 at 0xC) dwords of bytecode a4), and
  `perPrim + perObj + stable` 8-byte arguments a4, of which types 1 and 7
  load a 16-byte literal a4; the technique's name loads after its passes.
  `Load_MaterialPass` at 0x081F796C loads 0x14 and 0x64. VERIFIED.
- **material** 0x50 (`Load_Material` 0x081F81FE): name; the techset
  handle (slot 0x40); `textureCount` (0x3A) 0xC-byte texture defs a4, each an
  image handle at 8, or for semantic 11 a `water_t` a4 of 0x44 (H0 a4
  `8 M N`, wTerm a4 `4 M N`, its image handle at 0x40); `constantCount`
  (0x3B) 0x20-byte constants a16 (`Load_Material`'s align immediate 0xF,
  VERIFIED); `stateBitsCount` (0x3C) 8-byte state bits a4.
- **font** 0x18 (`Load_Font` 0x081F8524): name; material and glow
  material handles; `glyphCount` 0x18-byte glyphs a4.
- **menufile** 0xC (`Load_MenuList` 0x08201484): name; `menuCount`
  menu handle slots a4, each a menu.
- **menu** 0x11C (`Load_menuDef_t` 0x08200E82): window (name, group,
  background material at 0x98), font, onOpen, onClose, onESC, the onKey
  chain (0xC each: key, action, next), visibleExp, allowedBinding,
  soundName, rectXExp, rectYExp, then `itemCount` (0xA4) item slots a4.
  A statement (8: count, slot) loads its entry slots a4 and each entry
  a4 (0xC); an operand entry (type 1) with a string value (type 2)
  loads its string. An item 0x174 (`Load_itemDef_t` 0x08200556): window,
  eleven strings, onKey chain, enableDvar, the focus sound handle, its
  type data, eight statements. Type data by item type (0xB4): listbox
  (6) a4 0x154 (`Load_listBoxDef_ptr` 0x081FF0E2, VERIFIED), with a
  string at 0x120 and a material at 0x150; the edit-field kinds (0, 4, 9,
  10, 11, 14, 16, 17, 18) a4 0x20; multi (12) a4 0x188, 32 + 32 strings
  (`Load_multiDef_t` 0x081F6D20, VERIFIED); dvar enum (13) a string.
- **xanim** 0x58 (`Load_XAnimParts` 0x081F5C52): name; bone names a2
  (`boneCount[9]` u16); notifies a4 (8 each); the delta part a4 (8);
  dataByte a1, dataShort a2, dataInt a4, randomDataShort a2,
  randomDataByte a1, randomDataInt a4; indices a1 bytes, or a2 u16s at
  256 frames or more. The delta part's translation track: a4, a 4-byte
  head (`size` u16, `smallTrans` u8), then for `size == 0` a 12-byte
  frame, else 28 bytes (mins, size, frames slot) and `size + 1` indices
  (u8 or u16 as above), then the frames: a1 3-byte vectors under
  `smallTrans`, else a4 6-byte ones. Its rotation track: a4, a 4-byte
  head, then a 4-byte frame, or a frames slot, `size + 1` indices and a4
  4-byte frames.
- **xmodel** 0xDC (`Load_XModel` 0x081F8D8C): name; bone names a2 (u16
  per bone); parent list a1, quats a2 (8) and trans a4 (16), per non-root
  bone; part classification a1 and base matrices a4 (0x20), per bone; `numsurfs` (6) surfaces a4 (0x38); as many material handle
  slots a4; collision surfaces a4 (0x2C, each with 0x30-byte triangles
  a4); bone info a4 (0x28 per bone); the physpreset handle; the
  `PhysGeomList` a4 (0x2C, `count` 0x44-byte geoms a4, each with a
  `BrushWrapper` a4 of 0x50: sides a4, adjacency bytes a1, planes a4).
  A surface (`Load_XSurface` 0x081EBCD6): blend data a2 (`c0 + 3 c1 + 5
  c2 + 7 c3` u16s); the vertices a16 in block 7 (0x20 each); the rigid
  vertex lists a4 (0xC, each with a collision tree a4 of 0x28: nodes
  a16 of 0x10, leafs a2); the triangles a16 in block 8 (6 bytes each).
  The pushes of blocks 7 and 8 are VERIFIED (that function's push
  immediates).
- **fx** 0x20 (`Load_FxEffectDef` 0x081F9B6A): name; `looping + oneShot
  + emission` elements a4 of 0xFC (`Load_FxElemDef` 0x081F982A). An
  element: velocity samples a4 (0x60, `velIntervalCount + 1`), visual
  samples a4 (0x30, `visStateIntervalCount + 1`), visuals, three effect
  names (impact, death, emitted), a trail a4 (0x1C: vertices a4 0x14,
  indices a2). Visuals by element type (0xB0): decals (9) a4 two
  material handles per visual; otherwise more than one visual loads a4
  slots, one is inline; sprites, tails, trails and clouds (0-4) a
  material, models (5) an xmodel, runners (10) and sounds (8) a string.
- **impactfx** 8 (`Load_FxImpactTable` 0x081F9FF4): name; 12 entries a4
  of 0x84 (one `Load_Stream` of 0x630, VERIFIED), each 29 non-flesh then
  4 flesh fx handles.
- **weapon** 0x878 (`Load_WeaponDef` 0x081FA7D0): 95 slots in a fixed
  order, the table `assets::weapon::OPS` with their offsets: 15 strings,
  `gunModel[16]` and `worldModel[16]`, 33 xanim names, 10 fx, 8
  material and 6 xmodel handles, and 48 `SndAliasCustom` slots (a4, a
  4-byte struct holding the alias name). `bounceSound` (0x208) is a slot to 29
  of them a4; the four accuracy graphs load `knotCount` 8-byte knots a4.
- **com_map** 0x10 (`Load_ComWorld` 0x081F673E): name; primary lights a4
  (0x44, the def name at 0x40).
- **game_map_mp** 4 (`Load_GameWorldMpPtr` 0x082019D4): name only.
- **game_map_sp** 0x2C (`Load_GameWorldSpPtr` 0x081EE12E, which inlines
  `Load_PathData` 0x081EDD6C): name; path nodes a4 (0x80, each with
  `totalLinkCount` 0xC-byte links a4); base nodes a16 in block 1; two u16
  arrays a2; `visBytes` a1; node trees a4 (0x10: a split's two child
  slots a4, recursive, or a leaf's u16 node list a2).
- **map_ents** 0xC (`Load_MapEnts` 0x081F4962): name; `numEntityChars`
  bytes a1 (the NUL included). It rides in the clipmap only.
- **col_map_mp / col_map_sp** 0x11C (`Load_clipMap_t` 0x081FC348):
  name; planes a4 (0x14); static models a4 (0x50, an xmodel handle at
  4); materials a4 (0x48); brush sides a4 (0xC, each plane a4 or an
  offset); brush edges a1; nodes a4 (8, each plane a4 or an offset);
  leafs a4 (0x2C); leaf brushes a2; leaf brush nodes a4 (0x14, a positive
  count's u16 list a2 or an offset); leaf surfaces a4; vertices a4 (12);
  triangle indices a2 (`3 triCount` u16s); walkable bits a1 (`(3 triCount
  + 31) / 32 * 4` bytes); borders a4 (0x1C); partitions a4 (0xC, each
  border a4 or an offset); AABB trees a4 (0x20); submodels a4 (0x48);
  `numBrushes` (u16 at 0x8C) brushes a16 (0x50, sides and adjacency each
  one struct inline or an offset); visibility a1 (`numClusters *
  clusterBytes`); the map_ents handle; the box brush a16; two dynamic
  entity lists a4 (0x60: xmodel, fx, `XModelPieces` a4 of 0xC with 0x10
  model pieces a4, physpreset); then in block 1 the poses (0x20),
  clients (0xC) and collision (0x14) per dynamic entity of each list.
  `Load_cbrush_t` (0x081EA264) loads 0x50, 1, 0xC and 0x14. VERIFIED.
- **gfx_map** 0x2DC (`Load_GfxWorld` 0x081FDB6E): name, base name;
  indices a2; sky start surfaces a4; the sky image; the sun light a4
  (0x40, a lightdef at 0x3C); reflection probes a4 (0x10, an image at
  0xC) and, in block 1, their textures; `GfxWorldDpvsPlanes` (planes a4
  or an offset, nodes a2, cell bits in block 1: `256 cellCount` dwords);
  cells a4 (0x38: AABB trees a4 of 0x2C with a u16 static model list a2
  or an offset, portals a4 of 0x44 with a cell a4 or an offset and
  vertices a4, cull groups a4, probe bytes a1); lightmaps a4 (8: primary
  and secondary images); the light grid (row starts a2 over the row
  axis' span, raw rows a1, entries a4 of 4, colours a4 of 0xA8); in
  block 1 the primary and secondary lightmap textures; brush models a4
  (0x38); material memory a4 (8, a material at 0); vertices a4 (0x2C
  each, `vertexCount`) in block 4, not block 7; the vertex layer data a1;
  the sun flare's two materials; the outdoor image; in block 1 the cell
  caster bits, dynamic models (6 bytes each), dynamic brushes (4), the
  light shadow vis arrays and a byte per dynamic model; shadow geometry
  a4 (0xC, two u16 lists a2); light regions a4 (8: hulls a4 of 0x50 with
  0x14-byte axes a4); `GfxWorldDpvsStatic` (0x68 at 0x244: in block 1
  three model and three surface vis arrays, a1, and the LOD data a128;
  sorted surfaces a2; model instances a4 (0x1C); surfaces a4 (0x30, a
  material at 0x10); cull groups a4 (0x20); model draw instances a4
  (0x4C, an xmodel at 0x38); in block 1 surface materials (8) and the
  sun shadow bits a128); `GfxWorldDpvsDynamic` (in block 1 two cell bit
  arrays and six vis arrays a16).
  - The LOD data is 8 bytes per `smodelVisDataCount`. VERIFIED
    (`shl eax, 3` at 0x081FD6A6 before its `Load_Stream`).
  - A dynamic model (`GfxSceneDynModel`) is 6 bytes in block 1. VERIFIED
    (8 overshot block 1 in the four map zones tried, mp_crash by 0x400; 6
    fits all 85).
  - The world vertices load in block 4. VERIFIED (with them there, block
    7 ends at its `blockSize` in every map zone).

## 11. The walk census

`zone_census` walks every stock zone (`WalkOptions::default()`, bulk data
read and dropped) and checks: the last body ends on the last inflated
byte; every block's end (the temp block's peak plus 16) equals its
`blockSize`; every header slot holds an asset of its listed type; no
offset token is left unresolved. All 85 zones pass, in 3.5 s on four
threads. VERIFIED.

`walk_spots` walks mp_crash and code_post_gfx_mp with the bulk data kept.
VERIFIED:

- mp_crash's gfx_map `maps/mp/mp_crash.d3dbsp` (base name `mp_crash`):
  166418 vertices (0x2C bytes each), 347493 indices, 5558 surfaces, every
  one with a material and its triangles inside the index buffer, 23
  cells, 1 lightmap, 4038 static models, 134 materials. Surface 0 draws
  `wc/me_wire_black` with techset `wc_l_sm_r0c0`.
- mp_crash's col_map_mp: 11400 planes, 10978 brushes, 21774 brush sides,
  2130 nodes, 2165 leafs, 34 submodels, 4038 static models, 1028
  partitions, 1 cluster. Its map_ents: 43694 bytes, 309 entities, the
  first `worldspawn`.
- code_post_gfx_mp: 207 materials (200 listed, 7 inline), 94 techsets
  sharing 181 techniques, 181 passes carrying vertex shader bytecode, 124
  images. One material, `,$levelbriefing`, has a null techset slot.

## 12. Not walked or not decoded yet

- No stock zone carries xmodelpieces, ui_map, aitype, mptype, character
  or xmodelalias bodies, or a menu outside a menufile, so those paths are
  unexercised (section 7). VERIFIED for the absence.
- Data an offset token names inside an array the walk did not keep
  (no `keep_bulk`) is found but not copied (section 9).
- Anim tracks, shader bytecode, sound samples and most collision arrays
  are kept as raw bytes (`WalkOptions::keep_bulk`), not decoded in the
  walk; `vcod_cod4::map` decodes the render world, the cells and the
  collision indices of a map zone, and `vcod_cod4::image` the pixels
  (`cod4-world.md`).
- No rawfile was compared against a reference extraction.

## 13. Mods and usermaps: where a zone loads from, and which asset wins

`vcod_cod4::install::Install::zone_file` and `startup_zones`,
`vcod_cod4::source::Cod4Source::add_zone_flagged`; the census is
`crates/cod4/tests/mod_census.rs` over the install's stock example mod
`Mods/ModWarfare`. Addresses in this section are `iw3mp.exe` 1.7's.

- `DB_BuildOSPath` (0x489f20) takes a mode in `eax`: 0 builds
  `%s\zone\%s\%s.ff` (base path, language, zone; format at 0x6d3848), 1
  `%s\%s\%s.ff` (base path, `fs_game`, zone; 0x6d385c), 2
  `%s\%s\%s\%s.ff` (base path, `usermaps` at 0x6d3870, a directory, zone;
  0x6d387c), the directory being the zone name up to its first `_load`
  (0x6d3868, `strstr` at 0x489f48), else the whole name. VERIFIED (the
  disassembly).
- The zone open (0x48a7a0) compares the name with `mod` (0x6d0ec8): `mod`
  opens mode 1 only. Any other zone opens from mode 0 when that file
  exists (0x48b9b0 with mode 0, at 0x48a834), else mode 2, and the open
  marks the zone 2 (0x48a8d1). INFERRED (read off the branches). So on
  the 1.7 client only `mod.ff` comes from the `fs_game` directory, no other
  referenced zone. `vcod_cod4::install` follows
  the 1.7 rule, the directory names matched case-blind (`mods/ModWarfare`
  on a disk holding `Mods/ModWarfare`, as Windows opens it).
- `DB_ModFileExists` builds mode 1's `mod` path and only answers when `fs_game`
  is set; `DB_AddUserMapDir` adds `usermaps/<map>` to the iwd search
  (`FS_AddUserMapDirIWDs`) only under a set `fs_game` too. INFERRED. The
  client calls the latter (0x48ba90, `fs_game` tested at 0x48ba98) ahead
  of `<map>_load` (`%s_load` at 0x6d0758, 0x46a457). INFERRED.
- The startup zones and their `allocFlags`: `code_post_gfx_mp` 2,
  `localized_code_post_gfx_mp` 0, `ui_mp` 8 (client only),
  `common_mp` 4, `localized_common_mp` 1, then `mod` 16 when
  `DB_ModFileExists`; a map's zone loads with 8 and its `<map>_load` with
  32 (client only, `!com_dedicated`). INFERRED (`CL_InitDedicated`,
  `SV_SpawnServer`, the load screen load). The
  order without the mod is VERIFIED on the retail server's console
  (`cod4-handshake.md` 1).
- An asset whose type and name a loaded zone already holds replaces it
  when the new zone's flags are at least the old one's
  (`DB_OverrideAsset`: `new.flags >= old.flags`); otherwise it goes on the
  old entry's override list and the old one stays. A type without a
  default asset, rawfiles and map entities aside, fails the load with
  "Attempting to override asset". INFERRED (`DB_LinkXAssetEntry`). So the
  mod's assets win over every zone but a `_load`, the
  map's included, though the map loads later. `Cod4Source` keeps its
  zones sorted by flags, ties in load order; `add_zone` takes the last
  zone's flags, so a caller that adds zones in load order without flags
  searches the last first, as before.
- Scripts: under a set `fs_game`, `Scr_ReadFile` opens the file through
  the file system (every iwd and directory of the search path) and falls
  back to the zone's rawfile only when that fails; without `fs_game` it
  reads the rawfile alone. INFERRED.
  `Cod4Source` reads every file by path iwds first under an `fs_game`.
- ModWarfare (VERIFIED, the census): `mod.ff` holds 41 assets. 14
  override stock ones of `ui_mp` and `common_mp`: seven images (`$white`,
  `gradient_top` and kin) and their seven materials. The rest are new:
  `mod.csv`'s three menu files with their three `changeclass_*_mw` menus,
  twenty `ocd_*` popup menus and a rawfile named `mod`. Its
  `z_modwarfare.iwd` holds `images/logo_cod2.iwi` and ten
  `maps/mp/gametypes/*.gsc`; all but `_modwarfare.gsc` replace a file
  `common_mp` or `main/` holds, and each reads back as the mod's bytes.
- Retail under a mod (VERIFIED live, `crates/cod4/tests/fixtures/net/handshake-1.7-mod.txt`):
  the 1.7 Linux server started with `+set fs_game mods/ModWarfare` opens
  `<fs_basepath>/mods/modwarfare/`, lowercased: on a case-sensitive disk
  the directory must be lowercase (a `Mods/ModWarfare` tree needs a
  `mods/modwarfare` link). Its console then reads `Loading fastfile mod`
  after `localized_common_mp` and before `ui_mp` is unloaded and the map
  loads, and lists `z_modwarfare.iwd (15 files)` on the search path.
  `fs_game` itself reads `mods/modwarfare` everywhere: `game\` in
  `infoResponse`, `connectResponse mods/modwarfare`, the first pair of
  `statusResponse` and configstring 0, and a pair sorted among the rest
  of configstring 1 (only an empty `fs_game` trails, `cod4-handshake.md`
  9). `infoResponse` and `statusResponse` carry `mod\1`. Configstring 1
  holds `sv_referencedFFNames` `code_post_gfx_mp mp_crash common_mp
  mods/modwarfare/mod` with `sv_referencedFFCheckSums` `89196 35287381
  13517381 34310` (the four files' byte sizes), `sv_iwdNames` with
  `z_modwarfare` ahead of `iw_13`..`iw_00` and its sum first in
  `sv_iwds`, and two keys a stock server never sends:
  `sv_referencedIwdNames\main/iw_11 mods/modwarfare/z_modwarfare` and
  `sv_referencedIwds` their two sums. `iw_11` holds the `weapons/mp/*`
  files and nothing a stock level reads, so under an `fs_game` the
  server reads the weapon files out of the iwds as it does the scripts.
  INFERRED (the referenced list against the iwd's contents).
- The recipe: the section 7 tree plus `mods/modwarfare` linked to the
  mod's directory, `+set fs_game mods/ModWarfare` before `+map mp_crash`,
  then `tools/cod4_handshake_probe.py` as in section 7.
- Weapon files. `BG_LoadWeaponDef` reads `weapons/mp/<name>` off the
  search path when `fs_game` is set (or the build loads no fastfiles) and
  the zone's `WeaponDef` otherwise; a file that is missing or fails to
  parse takes `defaultweapon_mp`'s under its name
  (`BG_LoadWeaponDef_LoadObj`). INFERRED. The 1.7 Linux
  server reads them off disk, as its `main/iw_11` in
  `sv_referencedIwdNames` shows. The text is `WEAPONFILE` and an info
  string of fewer than 0x2800 bytes; `weaponDefFields` (cod4_lnxded
  0x826a160, 502 rows of name, offset, type; `tools/re/cod4_weapon_fields.py`)
  writes each key's first value into the 0x878-byte struct the zone
  carries: type 4 `atoi`, 5 `atoi != 0`, 6 `(float)atof`, 7 seconds to
  truncated milliseconds, 0 and 8..11 strings, effects, models,
  materials and sound aliases, 12..32 a name looked up in a table
  (0x826b8e8..0x826ba38: weapon types, classes, reticles, penetration,
  impacts, stances, projectile explosions, offhand classes, active
  reticles, guided missiles, stickiness, overlay interfaces, inventory
  and fire types, ammo counter clips, icon ratios; 20 is
  `mp/playeranimtypes.txt`'s list), 23 the bounce sound prefix, 33 the
  hide tags, 34 the notetrack map. VERIFIED (the table and the name
  tables). Which table serves which type, the def init's empty strings
  and the steps after (the inverse sight times off the ads times, the damage
  ranges' 999999 defaults, the five default sound aliases) are
  VERIFIED on 1.7 by `crates/cod4/tests/weapon_files.rs`: each
  of `iw_11`'s stock files reads back field for field as `common_mp`
  compiled it, `defaultweapon_mp`'s default sounds included.
  `vcod_cod4::weapon_file` is the reader.
- Usermaps. `FS_Startup` adds `usermaps/<map>/` between `main/` and the
  `fs_game` directory, each over the last, only when `fs_game` is set;
  its iwds go into the pure and referenced lists as
  `usermaps/<map>/<name>`. INFERRED.
- The loading screen's zone. `<map>_load.ff` holds `$levelbriefing` (the
  map's `loadscreen_<map>`), `$victorybackdrop` and `$defeatbackdrop`
  and a rawfile named after the zone; the images carry no pixels, so they
  read from the iwds (VERIFIED, `mp_crash_load`).
- `vcod-server --game cod4 --set fs_game=mods/<name>` lowercases the cvar,
  loads `mod.ff` after the common zones (its string tables and models
  ahead of the stock ones in the level's tables, its rawfiles and images
  first in the asset source), reads the scripts, `default_mp.cfg` and
  every weapon file iwds first with the mod's over `main/`'s, a usermap's
  between them, and writes configstrings 0 and 1 as retail does:
  `cod4::mod_tests` matches every key of configstring 1, the referenced
  iwds included. A usermap's zone goes into `sv_referencedFFNames` as
  `usermaps/<map>` and, with a `_load`, `usermaps/<map>_load`, both sizes
  under its slot (`DB_ReferencedFastFiles`, INFERRED).
- The client (`vcod --game cod4`) loads a map under the joined server's
  `fs_game` (configstring 1), or `--fs-game` offline: `mod.ff` walked
  beside the map and `common_mp`, its assets first, the usermap's and the
  mod's iwds over `main/`'s on the search path, which it opens again on
  every map change, and `<map>_load.ff` over all of them, its
  `$levelbriefing`'s image the connect screen's levelshot
  (cod4-hud.md 18). The HUD
  and the script menus walk `mod.ff` after the map, so ModWarfare's
  `changeclass_mw` and `ocd_*` menus reach the game, and load again when
  the server's `fs_game` changes.
- The client reads the weapon files too: under a set `fs_game` its
  prediction's weapon defs come off `weapons/mp/<name>` on the search
  path, as the server's do (`Cod4World::overlay_weapon_files`). A weapon
  file's model field, when set, reads as a set pointer in the struct, so
  `knifeModel`'s test (`WeaponDef::knife_model`, offset 772) holds off a
  file as off the zone. VERIFIED (`cod4_weapon_files_replace_the_zone_defs_under_a_mod`:
  under ModWarfare, which carries no weapon files, `main/`'s stock files
  give the zone's defs).
- Downloads: `cod4-handshake.md` 11.
- Not done: the HUD's other map-dependent
  assets on a map change under one `fs_game`.
