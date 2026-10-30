# CoD 1.1 gsc entity object model

The server side of a script object: which field names the engine backs with
real state, which fall through to a per-object script store, which BSP
classnames get an engine spawn function, and where the builtin tables live.

VERIFIED is read out of the binary or the shipped assets; INFERRED is read off
disassembly without a live test. The module is `game.mp.i386.so`, the 1.1d
Linux dedicated server's MP game module, with a full dynamic symbol table.
Addresses are module-relative (image base 0), as `nm -D` and `objdump -d`
print them.

Two tools in `tools/re/` reproduce every table below:

```
python3 tools/re/dump_script_fields.py game.mp.i386.so all
python3 tools/re/dump_builtins.py     game.mp.i386.so all
```

## 1. One trap that cost the previous pass three wrong claims

Pointers stored in `.data` read as zero in the file. The module is a shared
object, so every address in a static table (`SP_info_null`,
`ScriptEnt_MoveTo`, ...) is an `R_386_32` relocation in `.rel.data` with the
in-place word left at 0.

Read raw, that produced three wrong claims in `cod11-gsc-language.md` section 7
(corrected there): some builtin records carry a null function pointer,
`getent` and `getentarray` among them, and the `move*`/`rotate*` builtins sit
in a `.rodata` whitelist with a zero second field. Nothing has a null function
pointer; the movers are an ordinary dispatch table in `.data`. Both dumpers
resolve `.rel.data` and print the symbol.

## 2. Struct sizes, VERIFIED

| Struct | Size / offset | Evidence |
|---|---|---|
| `gentity_t` | 0x314 (788) bytes | `Scr_GetEntityField` 0x6282f and `Scr_SetEntityField` 0x6264f index with `imul $0x314` |
| `gentity_t.client` | +0x158 | both accessors test it for null before routing a client field |
| HUD element | 124 bytes | `Scr_GetHudElemField` 0x4c003 computes `(n * 32 - n) * 4` |

Field offsets below are byte offsets into those structs. Three cross-check
against unrelated code:

- `origin` at 308 = 0x134: `G_CallSpawn` passes `ent + 0x134` to
  `G_SetOrigin` (0x6168d).
- `angles` at 320 = 0x140, passed to `G_SetAngle` (0x6169d).
- `model` at 373 = 0x175: the type-8 branch of `G_ParseField` writes a byte to
  `0x175(%edi)` (0x6150a).

## 3. The field tables, VERIFIED

Four functions register script fields; three walk a static array.

| function | address | table | stride | records | 1.5b address, table, records | UO 1.51b address, table, records |
|---|---|---|---|---|---|---|
| `GScr_AddFieldsForEntity` | 0x62400 | 0x78e68 | 16 | 34 | 0x65bbf, 0x84340, 34 | 0x71d79, 0xae5a0, 37 |
| `GScr_AddFieldsForClient` | 0x41700 | 0x72ed4 | 20 | 13 | 0x3bd88, 0x82b40, 15 | 0x44082, 0xacb00, 16 |
| `GScr_AddFieldsForHudElems` | 0x4bf84 | 0x744e0 | 20 | 11 | 0x4965c, 0x82f80, 11 | 0x52659, 0xacfa0, 11 |
| `GScr_AddFieldsForRadiant` | 0x62470 | none, see section 6 | | | 0x65c53 | 0x71e16 |
| `GScr_AddFieldsForVehicleNode` | | | 12 | | | 0x983d4, 0xb33a0, 6 |

VERIFIED, `dump_script_fields.py` on 1.5b `game.mp.i386.so` (md5
343f99cd67b79ac74aeaa5261f63c011) and UO `game.mp.uo.i386.so` (md5
a4ffc2bfc3ae7af0b2374a23db3fa8ee): 1.5's entity and HUD tables carry 1.1's
names, types and order; entity offsets moved with the `gentity_t` layout
(`origin` 312, `classname` 380). 1.5's client table adds `sessionsquad`
(0xc002) and `maxspeed` (0xc005), renumbering every later client id. UO's
additions and renumbered type enum are in `coduo-diff.md` section 13.

Each walk ends at a record with a null name. Record layout:

```c
struct entity_field  { char *name; int offset; int type; void (*set)(); };            // 16
struct object_field  { char *name; int offset; int type; void (*set)(); void (*get)(); }; // 20
```

A non-null `set` or `get` replaces the generic conversion for that field.
`Scr_GetEntityField` never consults a getter, so entity fields always read
through the generic path; client and HUD tables use both hooks.

Each registering function skips records whose `type` it does not accept; a
skipped record is no script field. Entity and HUD accept 0 to 5, 7 and 8;
clients 0 to 5 and 7. Exactly one record is skipped: entity `light`, type 9,
offset 0, a Radiant-only key. The index still advances across it, so entity
field id 0x0e is never used.

## 4. The storage type enum, VERIFIED

`Scr_GetGenericField` (0x6248c) switches on `type` through a jump table at
0x7968c:

| type | stored as | script sees |
|---|---|---|
| 0 | int32 | int |
| 1 | float | float |
| 2 | `char[]` in place | string |
| 3 | u16 interned-string id | string, 0 reads as undefined |
| 4 | 3 floats | vector |
| 5 | `gentity_t *` | entity, null reads as undefined |
| 6 | float | vector `(0, f, 0)` |
| 7 | u16 object handle | object, 0 reads as undefined |
| 8 | u8 model index | string, via `G_ModelName` |

An unwritten type 0 field reads 0, not undefined. VERIFIED, retail
`client-probes/probe_bobbing.gsc` (2026-10-11): a script-spawned entity's
`spawnflags` concatenates as `0`. vcod gives types 0, 1 and 4 the zero for an
unwritten entity slot (`GameHost::get_field`).

Type 6 is the yaw-only angle form. No record uses it and entity and HUD
registration reject it: dead on the 1.1 MP server.

## 5. Field ids and how one namespace covers two objects

`GScr_AddFieldsForClient` ORs 0xC000 into the index (`or $0xc0,%ah` at
0x41740), so client field ids run 0xC000 to 0xC00C; entity and HUD ids are
plain indices.

`Scr_GetEntityField` and `Scr_SetEntityField` both test
`id & 0xC000 == 0xC000`. On a hit they take `ent->client`, mask the id with
`and $0x3f,%bh` and call `Scr_GetClientField` / `Scr_SetClientField`; else
they index the entity table. So one flat namespace per object kind, resolved
by name at registration, sends `self.score` and `self.origin` on one entity to
two structs through one opcode. A client-tagged id on an entity with no client
is a runtime error, not undefined.

### Entity fields, 0x78e68

```
 id  name             off  type
0x00 classname        374  3    (custom setter 0x6262c)
0x01 origin           308  4
0x02 model            373  8    (custom setter 0x6262c)
0x03 spawnflags       376  0    (custom setter 0x6262c)
0x04 speed            480  1
0x05 closespeed       484  1
0x06 target           468  3
0x07 targetname       470  3
0x08 message          456  3
0x09 teamname         472  3
0x0a wait             616  1
0x0b random           620  1
0x0c count            592  0
0x0d health           560  0
0x0e light              0  9    SKIPPED, not a script field
0x0f dmg              568  0
0x10 angles           320  4
0x11 duration         636  1
0x12 rotate           640  4
0x13 degrees          464  1
0x14 time             480  1
0x15 _color           668  4
0x16 color            668  4
0x17 key              680  0
0x18 harc             684  1
0x19 varc             688  1
0x1a delay            628  1
0x1b radius           624  0
0x1c missionlevel     692  0
0x1d start_size       700  0
0x1e end_size         704  0
0x1f shard            592  0
0x20 spawnitem        712  3
0x21 track            732  3
```

`speed`/`time` (480), `count`/`shard` (592) and `_color`/`color` (668) are
aliases. The setter 0x6262c on `classname`, `model` and `spawnflags` is one
function, the "read-only after spawn" guard.

### Client fields, 0x72ed4

```
 id     name             off   type
0xc000 name             8628  2
0xc001 sessionteam         0  3   fully custom get and set
0xc002 sessionstate        0  3   fully custom get and set
0xc003 maxhealth        8528  0
0xc004 handicap         8524  0
0xc005 score            8416  0
0xc006 deaths           8420  0
0xc007 statusicon          0  3   fully custom get and set
0xc008 headicon            0  3   fully custom get and set
0xc009 headiconteam        0  3   fully custom get and set
0xc00a spectatorclient  8404  0
0xc00b archivetime      8412  1
0xc00c pers             8424  7
```

`pers` is type 7, an object handle: `self.pers["team"]` works without the
engine knowing the key.

`sessionstate`'s getter answers `"spectator"` before script writes it.
VERIFIED live on retail 1.1d, 2026-09-07, with a copy of `sd.gsc` logging
`self.sessionstate` right after `Callback_PlayerConnect`'s
`waittill("begin")`: every first connect and every reconnect across a
`map_restart` printed `spectator`, before `spawnPlayer`'s assignment printed
`playing`. `_spawnlogic::getSpawnpoint_DM` compares every player's
`sessionstate` against strings, and an undefined one is a fatal "cannot
compare undefined"; vcod's `spawn_client` seeds the field with that string.
The other four custom getters' pre-write values are unmeasured.

`spectatorclient` reads -1 before script writes it. VERIFIED: `ClientConnect`
stores -1 at `client+0x20d4` (0x42534), the field's offset, after its `bzero`.
What the field does is `cod11-spectator-follow.md`.

The engine puts the `pers` handle there. VERIFIED live on retail 1.1d with
`crates/gsc/tests/fixtures/semantics/client-probes/probe_pers.gsc`,
2026-08-31, mp_pavlov, one client (recipe and log in that directory's README):
inside `Callback_PlayerConnect`, before `waittill("begin")`,
`isdefined(self.pers)` is true, `isdefined(self.pers["team"])` false, and
`self.name` holds the userinfo name. VERIFIED, same run: indexing a genuinely
undefined field is fatal (`undefined is not an array, string, or vector`) and
took the server down.

INFERRED: `ClientConnect` (`0x4246c`) saves its second argument across the
`bzero` of `gclient_t` and stores it back into `+0x20e8` (`0x4250f`), `pers`'s
offset above. No stock gametype creates `pers` (`dm.gsc:202` reads
`self.pers["team"]` as its first use), so a client entity needs both a `name`
and an indexable `pers` at connect.

### HUD element fields, 0x744e0

```
 id  name        off  type
0x00 x             4  0
0x01 y             8  0
0x02 fontscale    12  1
0x03 font         16  0   custom get and set
0x04 alignx       20  0   custom get and set
0x05 aligny       24  0   custom get and set
0x06 color        28  0   custom get and set
0x07 alpha        28  0   custom get and set
0x08 label        44  0   custom set
0x09 sort        108  1
0x0a archived    120  0   custom set
```

`color` and `alpha` share offset 28, one packed RGBA word. VERIFIED, the
getters: `color`'s (0x4c1ac) reads bytes +0x1c, +0x1d, +0x1e into a vector,
`alpha`'s (0x4c27c) reads +0x1f, each scaled by 1/255 (`0x74ac0`, `0x74acc`).
VERIFIED: both setters clamp to 0..1, multiply by 255.0 and add 0.5 with the
x87 mode set to truncate (0x4b082, 0x4c240), so a lane is
`(int)(clamp(v) * 255 + 0.5)`. VERIFIED: `GScr_NewHudElem` seeds the word to
`0xFFFFFFFF` (0x4b1d9), the opaque white the retail round clock carries on
the wire.

`font`, `alignx` and `aligny` are stored as ints but script uses names; the
type column is the storage. VERIFIED, the assets: `startGame` in
`maps/MP/gametypes/dm.gsc` (pak5) writes `level.clock.alignX = "center"`.
VERIFIED, a data read: each carries a set and a get hook over a name table:

| field | setter | getter | name table | names, in index order |
|---|---|---|---|---|
| `font` | 0x4c2f8 | 0x4c320 | 0x7de04 | `default`, `bigfixed`, `smallfixed` |
| `alignx` | 0x4c350 | 0x4c378 | 0x7de10 | `left`, `center`, `right` |
| `aligny` | 0x4c3a8 | 0x4c3d0 | 0x7de1c | `top`, `middle`, `bottom` |

INFERRED FROM DECOMPILATION: each setter wraps a shared helper at 0x4af80
`(elem, record, names, count)` that `strcmp`s the string against the table,
stores the matching index, and otherwise builds a message from the whole table
and calls `Scr_Error`. Each getter indexes the table back through
`Scr_AddString`. The name-to-index direction is not measured live.

`label`'s hook (0x4c400) is a setter only and takes a localized string.
VERIFIED: it calls `Scr_GetIString` (0x4c419) and stores what
`G_LocalizedStringIndex` (0x4c422) answers, the 1-based index a client
resolves through configstring `1244 + n`. `setText` (0x4c590) does the same
into +0x68.

VERIFIED: `archived` (0x4c43c, +0x78) is seeded to 1 by `GScr_NewHudElem`
(0x4b1fc). It picks which of the playerstate's two HUD arrays the record is
copied into (`docs/research/cod11-hud-protocol.md` section 5).

The rest of the record (type tag at +0x0, `setShader`'s size and material
index, the timers' deadline, `setValue`'s number, `setText`'s string index,
the owner and team the wire build filters on) has no script field; the
fourteen hudelem methods write it.

## 6. Radiant fields come from a file, not a table

`GScr_AddFieldsForRadiant` (0x62470) is three instructions:
`Scr_AddFields("radiant", ".txt")`, no static array.

`radiant/keys.txt` ships in stock `pak4.pk3`, 7303 bytes; its header gives the
syntax: one line per key, a type keyword (`float`, `int`, `string`), then the
name. 113 keys: 56 int, 43 string, 14 float. 99 are the `script_*` family the
SP campaign reads (`script_delay`, `script_noteworthy`, `script_exploder`,
...). The other 14: `delay`, `count`, `radius`, `spawnflags`, `export`,
`dontdropweapon`, `dontdrawoncompass`, `target`, `targetname`, `groupname`,
`name`, `weaponinfo`, `vehicletype`, `ambient`.

Six of those (`delay`, `count`, `radius`, `spawnflags`, `target`,
`targetname`) are also entity fields, and stage 1 of `G_ParseField` runs
first, so their radiant entry never fires during a map load.

INFERRED: `Scr_AddFields` is an engine callback through the syscall pointer at
0xc10a4, invisible in this module. The name, extension and the file at that
path make "load `radiant/keys.txt` and register each line as a typed script
field" the only reading that fits; it has not been observed.

## 7. What the map load actually does with an entity block

`G_CallSpawn` (0x615e4) runs these cases over the classname, in order:

1. No `classname` spawn var: `G_Printf` a warning, return, no entity.
2. `bg_itemlist` (stride 0x30, `strcmp`) hit: `G_Spawn`, apply every spawn var,
   `G_SetOrigin`, `G_SetAngle`, `G_SpawnItem`.
3. `spawns` (section 8, stride 8, `strcmp`) hit: `G_Spawn`, apply every spawn
   var, `G_SetOrigin`, `G_SetAngle`, the record's `SP_` function. Five of those
   free the entity again (section 13).
4. Neither: `G_Spawn`, apply every spawn var, `G_SetOrigin`, `G_SetAngle`,
   nothing else.

Case four is what the object model rests on: an unknown classname still makes
a live `gentity_t` with all its Radiant keys applied. That is why
`mp_deathmatch_spawn`, in neither table, survives the load for
`_spawnlogic.gsc`'s `getentarray`. Classname matching is case-sensitive
(`strcmp`); key names are not (below).

### Applying one key/value pair

The spawn-var applier is the unnamed function at 0x61400, here
`G_ParseField`:

1. Walk the entity field table with `Q_stricmp` (Radiant keys match engine
   field names case-insensitively). On a hit, convert by the field's type and
   write into the `gentity_t`: `%f %f %f` through `sscanf` for a vector,
   `strtol` for an int, `strtod` for a float, `G_NewString` plus an interned id
   for a string. A `model` value starting with `*` is a brush model; its
   number goes to `ent+0x8c` instead.

   A type-8 `model` value that is not a brush model goes through
   `G_ModelIndex`. VERIFIED: the call at 0x61505 relocates against that symbol
   and the returned byte is stored at `ent+0x175`. So **the entity lump fills
   the model configstring block before any script runs**, in lump order, and
   script's `precacheModel`/`setModel` continue from there. VERIFIED
   independently by the committed retail captures: `mp_carentan-dm.txt` opens
   the model block with map props (`xmodel/static_vehicle_german_truck` and
   the rest of the lump) and reaches the team player models only after.

   Four facts decide which keys take which slot (section 18 has the arithmetic
   on both gate maps):

   - **`model` is the only type-8 record in any field table.** VERIFIED,
     `python3 tools/re/dump_script_fields.py game.mp.i386.so`: one type-8 row
     of 58, `model` at 373 (0x175). `radiant/keys.txt` declares only `float`,
     `int` and `string` (section 6), so stage 2 cannot produce one.
   - **A brush model takes no slot.** INFERRED for the mechanism: the
     `*`-prefix branch stores at `ent+0x8c` without reaching 0x61505.
     VERIFIED for the outcome: no `*N` appears in either capture's model block,
     though the lumps carry 10 (`mp_carentan`) and 34 (`mp_pavlov`) brush
     `model` keys.
   - **A block whose `SP_` function frees still takes its slot.** VERIFIED,
     the captures: `misc_model` is one of the five freeing classnames (section
     13), 592 of `mp_carentan`'s 855 blocks are `misc_model`, and slot 269 of
     its capture is the first of them. INFERRED: `G_CallSpawn` applies every
     spawn var before the `SP_` function, so the index is taken before the
     free; the opposite of the entity *number* rule the same five obey.
   - **`G_ModelIndex` interns.** INFERRED: the call at 0x61505 is the only
     writer into the block during the lump parse, so a repeated name can only
     intern onto its first slot. VERIFIED for the outcome: `mp_carentan`'s 672
     non-brush `model` keys are 57 distinct names, each held exactly once in
     first-appearance order across slots 269-326 (the one non-lump slot among
     them is an item registration's, section 18).

   The index is a byte, so a model index above 255 cannot sit on an entity
   though the configstring range runs to 523. Nothing measured reaches it
   (`mp_carentan`, the larger gate map, ends its model block at 399); an
   untested observation.
2. On a miss, `Scr_FindField(key, &type)`. Non-zero means a registered script
   field: convert by that type (1 `Scr_AddString`, 4 `strtod` then
   `Scr_AddFloat`, 5 `strtol` then `Scr_AddInt`) and store with
   `GScr_SetDynamicEntityField`.
3. Neither: the pair is dropped silently.

So the split is three-way: engine-backed fields land in the C struct,
registered script fields in the entity's script object, and everything else in
the BSP entity lump is discarded at load, invisible to script.

INFERRED: `Scr_FindField` is also an engine callback (0xc10a8), so whether its
search covers only radiant fields or every registered field (client and HUD
names included) is not visible here. vcod assumes the narrow reading; a probe
settles it if a map ever depends on the wide one.

### The order of a block's keys

- `G_ParseSpawnVars` (0x61a8c) reads a block's tokens pairwise and appends each
  pair to `level.spawnVars` (count at `level+0x1348`) in entity-string order,
  repeats kept. VERIFIED, control flow of 0x61a8c: one append per pair, count
  incremented after.
- `G_CallSpawn` (0x615e4) hands the pairs to `G_ParseField` (0x61400) from
  index 0 up in each of its three spawning arms (loops at 0x61660, 0x61707,
  0x6177d: `inc esi`, `cmp esi, level+0x1348`, `jl`), then calls
  `G_SetOrigin`, `G_SetAngle` and the arm's spawn function. INFERRED,
  instruction order. INFERRED from both facts: a repeated key is applied once
  per copy in lump order and the field keeps the last; a repeated type-8
  `model` indexes every name it carries.
- `G_SpawnString` (0x61390), behind every `G_SpawnInt`, `G_SpawnFloat` and
  `G_SpawnVector`, `strcmp`s the pairs from index 0 and returns the first
  match. VERIFIED: its one call relocates against `strcmp`
  (`tools/re/annotate_func.py game.mp.i386.so G_SpawnString`, 0x613c9). So the
  spawn functions' own reads are case-sensitive and take a repeated key's
  first copy; the field pass keeps its last.
- CoD2 1.3 keeps the shape. INFERRED, control flow of the decompile:
  `G_ParseSpawnVars` (cod2_lnxded 0x80f3fd2) appends pairs in token order;
  `G_ParseEntityFields` (0x811a7b0), called by `G_CallSpawn` (0x811aa7a) in
  each spawning arm, walks them from 0 through `G_ParseEntityField`, then
  `G_SetOrigin` and `G_SetAngle`; the `script_struct` copy (0x811a828) walks
  them the same way; the `G_SpawnString` walk (0x80f415e) `strcmp`s from 0 and
  returns the first.

vcod applies a block's pairs in lump order (`bsp::entity_block_pairs`,
`spawn_entities_from_string`), as do the CoD2 struct pass and UO's
`spawnDuplicate` (`spawn_from_vars`). Trap: a `HashMap`'s order changes per
process. On UO's mp_cassino `script_origin` "auto9" (entity 414, `"origin"`
then `"angles" "0 2.50448e-006 0"`) the `origin` write's link snapped a yaw
already set when `angles` came first, and the baseline left `apos.trBase[1]`
out. Applied in lump order the yaw stays unsnapped and goes out as a sent 0,
byte for byte the retail UO 1.51b gamestate's baseline
(`fixtures/net/gamestate-uo.bin`). VERIFIED by
`uo_cassinos_auto9_baseline_matches_retail`.

vcod's lookups follow the two rules. A spawn function's own read goes through
`bsp::spawn_string` (`strcmp`, first copy). VERIFIED, the calls and their key
strings:

| Reader | Keys | Address |
|---|---|---|
| worldspawn | `classname` (then `Q_stricmp`'d against `worldspawn`, 1.1d 0x61d16), `ambienttrack`, `message`, `gravity`, `northyaw` | |
| `G_CallSpawn` | `classname` | 0x615fe |
| `SP_turret`, `G_SpawnTurret` | `weaponinfo`; the arcs | 0x533c8 |
| `SP_trigger_multiple` | `wait` | 0x64c6f |
| `SP_trigger_hurt` | `sound`, `life` | 0x64f99, 0x64ffe |
| `SP_trigger_damage` | `wait`, `random`, `accumulate`, `threshold` | 0x65297..0x6532d |
| bobbing, pendulum, static movers | their keys | 0x583ef.., 0x56ffd.., 0x58280 |
| CoD2 `trigger_radius`, `trigger_disk` | `radius`, `height` (`G_SpawnFloat` in `SP_trigger_radius`/`SP_trigger_disk`) | cod2_lnxded |
| UO `SP_script_vehicle` | `delay`, written over the field pass's | game.mp.uo 0x9dec6 |

A read of what the field pass left on the entity goes through
`bsp::field_value` or `bsp::entity_blocks` (keys folded, last copy):
`spawnflags`, `model`, `origin`, UO's `teamname` at `ent+0x1e8` in
`SP_trigger_damage` (0x86f59), CoD2's round-trigger `wait` (read by no
`G_SpawnFloat` in cod2_lnxded). INFERRED for `spawnflags` and `model`, which
spawn functions read off the entity in the Q3 lineage. The client's and the
bots' readers of entity fields (spawn points, turrets, rubble, cover) take
`entity_blocks` too.

The renderer reads the lump with a third rule. VERIFIED, decompile:
CoDMP.exe 1.1's entity pass (0x4dbf50) keeps a key spelled exactly
`classname` in a slot of its own, each copy over the last, and compares it
with `_stricmp`; any other key is looked up by 0x4dba20, the first pair under
`_stricmp`. `bsp::coronas` reads through it (`render_classname`,
`render_key`). The static models are read twice: 0x4dbae0 registers the drawn
ones through 0x4dba20 (first copy); 0x41f950 walks the pairs with `_stricmp`
and overwrites each value, so the last copy stands (VERIFIED, decompile of
0x41f950). INFERRED: 0x41f950 is the client's twin of cod_lnxded's collision
parse 0x80515d4 (it registers what it reads through 0x41f780). vcod's one
`props::placements` serves both and reads the field view (last copy).

VERIFIED, the scan below: the two parsers part on more than repeats.

- 0x41f950 applies `modelscale` and `modelscale_vec` in lump order; 0x4dbae0
  and vcod take `modelscale_vec` first. 82 of mp_brecourt's `misc_model`s
  (196 of UO mp_bocage's, a few on mp_rocket, mp_foy, mp_neuville, mp_ponyri,
  mp_rhinevalley, mp_kursk, mp_italy, mp_uo_stanjel) list a `modelscale_vec`
  before a different `modelscale`.
- 0x4dbae0 takes an `angle` other than 0.0 (float at 0x568e64) over `angles`;
  vcod takes `angles` (no stock MP block carries both).

VERIFIED, a scan of every map's entity string in the 1.1 and UO stock paks
and the CoD2 1.3 iwds: no MP block repeats a key exactly. Case-folded, UO
mp_cassino's `light`s 237, 238, 311, 312 and 654 spell `overbrightshift` twice
(`overbrightShift`, `overbrightshift`), which names no field. Among the stock
single-player maps, brecourt's worldspawn lists `ambient` twice ("0.18", then
".1"), UO credits' `script_brushmodel`s 554-557 and 710-715 list `origin`
twice with one value, and factory's and trainbridge's `light`s repeat
`overbrightshift` folded.

## 8. The `spawns` classname table, VERIFIED

0x7eb30, 8-byte records `{char *name; void (*spawn)(gentity_t *);}`, ending at
a null name. 27 entries:

```
info_null           SP_info_null            trigger_hurt         SP_trigger_hurt
info_notnull        SP_info_notnull         trigger_once         SP_trigger_once
func_door           SP_func_door            target_location      SP_target_location
func_static         SP_func_static          mp_target_location   SP_target_location
func_rotating       SP_func_rotating        light                SP_light
func_bobbing        SP_func_bobbing         misc_teleporter_dest SP_misc_teleporter_dest
func_pendulum       SP_func_pendulum        misc_model           SP_misc_model
func_group          SP_info_null            misc_mg42            SP_turret
func_door_rotating  SP_func_door_rotating   misc_turret          SP_turret
trigger_multiple    SP_trigger_multiple     misc_spawner         SP_misc_spawner
corona              SP_corona               script_brushmodel    SP_script_brushmodel
trigger_use         trigger_use             script_model         SP_script_model
trigger_damage      SP_trigger_damage       script_origin        SP_script_origin
trigger_lookat      SP_trigger_lookat
```

`func_group` maps to `SP_info_null` (grouping is compile-time; the entity is
inert and freed outright, section 13). `mp_target_location` shares
`SP_target_location`; `misc_mg42` and `misc_turret` share `SP_turret`.
`worldspawn` is absent because `G_SpawnEntitiesFromString` handles entity 0
before this loop. No MP gametype spawn classname (`mp_deathmatch_spawn`,
`mp_teamdeathmatch_spawn`, the `mp_*_intermission` markers) is in the table:
they take case four above.

### 8.1 `trigger_hurt`: what its spawn function and its touch do

The heading's label covers the table; every claim below carries its own.
`SP_trigger_hurt` is at 0x64ef8, its touch function `hurt_touch` at 0x64dc4.

- **Damage from `dmg`.** VERIFIED: `SP_trigger_hurt` reads gentity+568 and at
  0x64fbb writes 5 there; 568 is the `dmg` slot (section 5). INFERRED, from
  the `cmp ... ,0` / `jne` before the write: 5 is the default when the field is
  still 0, so `"dmg" "0"` takes the default too.
- **Damage flags from spawnflags.** VERIFIED: `hurt_touch` bit-tests the
  spawnflags word, and the immediate 0x10 reaches `G_Damage`'s `dflags`; 0x10
  is `DFLAG_NO_PROTECTION` (`crates/server/src/game/combat.rs`). INFERRED,
  from the test and its branch: the bit is spawnflag 0x8, the argument 0x10
  when set and 0 when not.
- **Means of death `MOD_TRIGGER_HURT`.** VERIFIED: the `mod` argument is the
  immediate 0x17, index 23 of the mod table (`MOD_NAMES`,
  `crates/server/src/game/combat.rs`, and `cod11-combat.md`).
- **Inflictor and attacker the trigger; point and direction zero.** VERIFIED:
  `hurt_touch` pushes its own `self` twice and the immediate 0 for the other
  two.
- **Own cadence, no `wait`/`random`.** VERIFIED: `hurt_touch` reads and writes
  a timestamp at gentity+0x1cc, compares it with the level clock, carries the
  immediates 100 and 1000 as addends to that clock, and contains a
  `Scr_Notify` and the `G_Damage` call. INFERRED, from the compare and its
  branches: an early return while the timestamp is in the future, 1000 the
  spawnflag 0x10 arm and 100 the other, the timestamp gating both the
  `Scr_Notify` and the `G_Damage`. VERIFIED on retail 1.1d, 1.5b and UO: the
  100 and 1000 ms cadences, spawnflags 0x4, 0x8 and 0x20 and the per-touch
  read of `dmg` (`cod11-combat.md` 5.9).
- **A byte at gentity+0x171 on the toucher.** VERIFIED: the compare against 0
  at 0x64dd2 on the second argument (loaded at 0x64dcc). INFERRED, from the
  `je`: the first gate, an early return ahead of the timestamp compare. The
  byte's meaning is not measured; 0x171 is not in the entity field table, so
  it is engine-only, and calling it Q3's `takedamage` is a guess. vcod does
  not model this gate: it is the inner of two, and a dead client never reaches
  `hurt_touch` because the touch pass drops him first (8.2).

### 8.2 `G_TouchTriggers` never reaches a dead client

`G_TouchTriggers` (0x3f88c) opens with two guards, so every per-classname touch
function (`hurt_touch`'s `+0x171` byte included) is an inner gate.

- **No client, no pass.** VERIFIED: `mov 0x158(%eax),%eax` at 0x3f89b
  (`ent->client`, section 2), `test`/`je` at 0x3f8a1-0x3f8a3 to the exit at
  0x3faa8.
- **`client+0x4` above 1 returns.** VERIFIED: `cmpl $0x1,0x4(%eax)` at
  0x3f8a9, `jg` at 0x3f8ad to the same exit.
- **`client+0x4` is `ps.pm_type`.** INFERRED, three readings that agree:
  `ClientThink_real` computes `msec` as `ucmd->serverTime - *client`
  (0x3ff25), putting `ps.commandTime` at +0x0 and `pm_type` at +0x4 on the Q3
  layout; `SpectatorThink` writes 4 there (0x3fb94) and `ClientEndFrame`'s
  intermission arm writes 5 (0x40f00, via `ent->client` loaded at 0x40eea);
  4 and 5 are the values vcod measured on the wire for those states.
- INFERRED, from the rule and the measured values: only `pm_type` 0 and 1
  touch anything; the dead (6), a spectator (4) and the intermission camera
  (5) touch nothing. A dead player in a trigger notifies it on no frame (else a
  corpse waiting to respawn in a minefield re-fires `_minefields.gsc`'s
  `minefield_kill` every pass). vcod takes the guard.
- Retail does not leave the dead to script: `sd.gsc`'s `isalive(other)` tests
  cover a player who dies mid-plant.

## 9. Builtins are five tables, VERIFIED

`cod11-gsc-language.md` section 7 points here for the record struct.

| table | address | stride | count | walked by | 1.5b address, count | UO 1.51b address, count |
|---|---|---|---|---|---|---|
| `functions` | 0x7e508 | 12 | 106 | `Scr_GetFunction` 0x5c15c | 0x83c40, 110 | 0xadde0, 120 |
| player methods | 0x733dc | 8 | 46 | `Player_GetMethod` 0x448e4 | 0x82ca0, 54 | 0xacc80, 60 |
| scriptent methods | 0x78d40 | 8 | 12 | `ScriptEnt_GetMethod` 0x60f60 | 0x842e0, 12 | 0xae540, 12 |
| hudelem methods | 0x749b4 | 8 | 14 | `HudElem_GetMethod` 0x4be38 | 0x83080, 14 | 0xad0a0, 14 |
| entity methods | 0x7ea00 | 8 | 38 | unnamed, 0x5c1c0 | 0x84180, 44 | 0xae380, 56 |
| vehicle methods | | 8 | | `ScriptVehicle_GetMethod` (UO 0x8df6e) | | 0xaec80, 31 |

VERIFIED, `dump_builtins.py` on the modules of section 3; each count is the
lookup's loop bound. Against 1.1 by name, 1.5 removes nothing and adds:

| Table | 1.5 additions |
|---|---|
| functions | `spawnturret`, `precacheturret`, `prof_begin`, `prof_end` |
| player methods | `saysquad`, `disableweapon`, `enableweapon`, `allowspectateteam`, `getguid`, `autodemostart`, `autodemostop`, `autoscreenshot` |
| entity methods | `getattachignorecollision`, `getstance`, `setrightarc`, `setleftarc`, `settoparc`, `setbottomarc` |

VERIFIED, same dump: the `spawns` table (section 8) sits at 1.5b 0x84580 with
1.1's 27 classnames, and at UO 0xae800 with 32.

216 builtins in 1.1. Every walk is a linear `strcmp` over a hardcoded count,
not a null terminator (bounds 0x69, 0x2d, 0x0b, 0x0d, 0x25, inclusive
compares).

`functions` records are `{char *name; void (*fn)(); int developer;}`, method
records `{char *name; void (*fn)();}`. The stride difference is why a uniform
8-byte scan of `[0x7e508, 0x7eb30)` garbles the first table and lands on 144
(106 plus 38).

`developer` is 1 on exactly `print`, `println` and `assert`. CoDExtended's
`src/script.c` declares the same struct as
`SCRIPTFUNCTION { name, function, developer }` and the same
`Scr_GetFunction(const char **fname, int *fdev)` signature, an independent
read on the field name and the out-parameter. `Scr_GetFunction` writes the
table's spelling back through `*fname` (0x5c199): the engine canonicalises a
builtin name on lookup.

VERIFIED, the call order in each `Scr_GetMethod`; first hit wins, so a name in
an earlier table shadows a later one:

| Build | Address | Order |
|---|---|---|
| 1.1 | 0x5f724 | player, scriptent, hudelem, entity |
| 1.5 | 0x62665 | player, scriptent, entity (0x625ea), hudelem |
| UO | 0x6e75f | player, scriptent, vehicle, hudelem, entity |

`spawn`, `iprintln` and `iprintlnbold` appear both in `functions` and the
player methods: the `self iprintln(...)` versus bare `iprintln(...)` split.

The movers are the scriptent table, ordinary code pointers in `.data`:
`moveto`, `movex`, `movey`, `movez`, `movegravity`, `rotateto`,
`rotatepitch`, `rotateyaw`, `rotateroll`, `rotatevelocity`, `solid`,
`notsolid`. `ScriptEnt_GetMethod` sits directly above the entity field table
in `.data`, likely why the region was once mistaken for a `.rodata` string
list.

## 10. `getEntArray` returns ascending entity number, VERIFIED

`Scr_GetEntArray` (0x61980) walks `esi` from 0 to `level.num_entities`
(`level`+0xc), skips a slot whose `inuse` byte (+0x160) is clear, and appends
matches in that order. The keyed branch first checks the entity field table
type at 0x78e70 is 3 (interned string), so the keys the corpus passes
(`targetname`, `classname`, `target`, `teamname`, script-defined string keys)
are type 3 or script-defined, never numeric.

The capture confirms it. `probe_ents` on `mp_pavlov` returned the four map
`script_origin`s as auto5, auto4, auto3, auto6. `mp_pavlov.bsp`'s entity lump
holds 345 blocks; its four `script_origin` blocks sit at indices 2, 3, 4 and
344 with exactly those targetnames in that order. Lump order gives entity
number, entity number gives `getEntArray` order.

INFERRED, disassembly (no probe): `getEntArray()` with no arguments is a
second form. `Scr_GetEntArray` opens with a parameter-count call at 0x61989;
zero parameters take the path at 0x61997, the same slot walk without the field
compare, returning every entity in use in ascending order.
`maps/mp/gametypes/_gameobjects.gsc::main` is the stock caller, reached from
every gametype's `Callback_StartGameType`, so a host with only the keyed form
kills that thread on its first line.

The corpus passes six keys: `targetname` (1183 call sites across both
builtins), `classname` (131), `script_noteworthy` (31), `target` (5),
`export` (2), `team` (1). Two are `radiant/keys.txt` entries and `team` is
neither an engine field nor a radiant key, so the lookup goes through the same
three-way field resolution as a plain `.name` read.

## 11. Entity numbering, VERIFIED

`G_InitGame` (0x4fb14) sets `level.num_entities = 72` at 0x4fdc6. `G_Spawn`
(0x667e0) checks the free list first (section 14) and only bumps
`level.num_entities` when it is empty; a bump that would hand out 0x3fe
(`ENTITYNUM_WORLD`, 1022) goes to `G_Error`. `Scr_GetEntArray` reads the same
`level+0xc`. The free-list check is unconditional and comes first (0x667e9);
0x3fe is the exhaustion error, not where `G_Spawn` starts reusing slots.

So the map load's first entity is number 72, whatever `sv_maxclients` is.
`MAX_CLIENTS` is 64 on CoD 1.1; `G_Spawn` never hands out 64 to 71, which hold
the body queue (`cod11-combat.md` 5.2: `g_entities[64 + bodyQueIndex]`,
index `& 7`).

`G_SpawnEntitiesFromString` (0x622dc) parses the first block, `G_Error`s if
its classname is not `worldspawn`, runs `SP_worldspawn` on it, then loops
`G_ParseSpawnVars` plus `G_CallSpawn` over the rest. The world is not
allocated through `G_Spawn` and takes no number in the 72-and-up range.

## 12. What stage 2 takes from this

The rules the host follows:

- Four object kinds; the field id discriminates two of them: a plain index for
  entity, `| 0xC000` for client, a plain index in its own namespace for HUD
  elements.
- Field lookup by name is case-insensitive, at script level and for Radiant
  keys at map load. Field ids are dense small integers (`u16`).
- `getentarray` sees case-four entities in ascending entity number (BSP lump
  order), numbered from 72. Five classnames free the entity their `SP_`
  function was handed and the slot is reused at once, so those blocks take no
  number (sections 13, 14).
- A Radiant key that is neither an engine field nor in `radiant/keys.txt` is
  dropped at load; stashing it would show scripts fields retail lacks.
- 216 builtin names is the dispatch ceiling, in the five-table split and
  lookup order of section 9. The 37 names `mp_pavlov` needs are a subset.

## 13. Five classnames consume their block without an entity, VERIFIED

`G_CallSpawn`'s third case runs the record's `SP_` function after the entity
is built, and four of those functions are nothing but `G_FreeEntity(self)`:

| function | address | classnames it serves |
|---|---|---|
| `SP_info_null` | 0x531cc | `info_null`, `func_group` |
| `SP_light` | 0x53204 | `light` |
| `SP_misc_model` | 0x53224 | `misc_model` |
| `SP_corona` | 0x53274 | `corona` |

Each body: push the argument, one `R_386_PC32` call relocated against
`G_FreeEntity` (0x531d9, 0x53211, 0x53231, 0x53281), return; no branch.
`python3 tools/re/dump_builtins.py <game.mp.i386.so> spawns` marks these five
rows `frees` by scanning each function's `.rel.text` for `G_FreeEntity`, so it
covers all 27 records whatever a map contains. `G_Spawn` hands the freed slot
straight back out (section 14), so such a block takes no entity number.

### The live counts

`probe_spawnfree`, a throwaway gametype, logged
`getEntArray(<classname>, "classname").size` for all 27 `spawns` classnames on
retail 1.1d (2026-08-29, `sv_maxclients 8`); lump counts from the shipped
BSPs. Every classname whose live count is below its lump count is one of the
five; every other matches exactly.

| classname | mp_railyard lump / live | mp_chateau lump / live | mp_powcamp lump / live |
|---|---|---|---|
| `info_null` | 7 / **0** | 2 / **0** | 38 / **0** |
| `info_notnull` | 0 / 0 | 1 / 1 | 0 / 0 |
| `trigger_multiple` | 3 / 3 | 2 / 2 | 3 / 3 |
| `trigger_hurt` | 2 / 2 | 11 / 11 | 1 / 1 |
| `light` | 42 / **0** | 145 / **0** | 65 / **0** |
| `misc_model` | 246 / **0** | 883 / **0** | 601 / **0** |
| `misc_mg42` | 1 / 1 | 0 / 0 | 0 / 0 |
| `corona` | 33 / **0** | 0 / 0 | 60 / **0** |
| `trigger_use` | 1 / 1 | 2 / 2 | 1 / 1 |
| `trigger_lookat` | 1 / 1 | 0 / 0 | 1 / 1 |
| `script_brushmodel` | 2 / 2 | 2 / 2 | 5 / 5 |
| `script_model` | 9 / 9 | 4 / 4 | 89 / 89 |
| `script_origin` | 4 / 4 | 2 / 2 | 19 / 19 |

The other 14 `spawns` classnames read 0 live on all three maps and appear in
no stock MP lump: `func_door`, `func_static`, `func_rotating`, `func_bobbing`,
`func_pendulum`, `func_group`, `func_door_rotating`, `trigger_once`,
`target_location`, `mp_target_location`, `misc_teleporter_dest`,
`misc_turret`, `misc_spawner`, `trigger_damage`. The relocation scan covers
them and puts only `func_group` in the freeing set. `misc_teleporter_dest`'s
`SP_` function (0x5321c) is an empty body: a keep.

### The arithmetic checks out on every stock map

First entity spawned after the map load, retail against
`72 + (blocks - 1 - freeing blocks)`:

| map | lump blocks | freeing blocks | predicted | retail |
|---|---|---|---|---|
| mp_pavlov | 345 | 117 | 299 | 299 |
| mp_chateau | 1176 | 1030 | 217 | 217 |
| mp_powcamp | 1065 | 764 | 372 | 372 |
| mp_railyard | 533 | 328 | 276 | 276 |

mp_pavlov's figure is `# probe_ents` in
`crates/gsc/tests/fixtures/semantics/retail-captures.txt`: its three
script-spawned entities are 299, 300, 301, behind the map's four
`script_origin`s at 73, 74, 75 and 298. The other three are
`probe_spawnfree`'s `spawn1` line. No stock MP lump has a block without a
classname.

## 14. `G_Spawn` drains a free list before it bumps the counter

VERIFIED, the binary: `G_Spawn` 0x667e0, `G_FreeEntity` 0x66948. The two words
`G_Spawn` loads at 0x667e9 and `G_FreeEntity` stores through at 0x66bb3 are
`level+0x10` and `level+0x14`; the per-entity words they chain and bump are
`gentity+0x304` and `gentity+0x300`; the guard at 0x66bb1 compares the entity
index against 71; the counter path is 0x6688b, its `G_Error` limit 0x3fe.

INFERRED FROM DECOMPILATION: the two `level` words are head and tail of a
singly linked FIFO of freed entities. `G_Spawn` takes the head entity when
non-null (0x668c8), pops it, and clears the tail if the list empties; only an
empty list reaches the counter path and its `G_Error`. `G_FreeEntity` appends
at the tail, and its guard keeps 0..71 out of the list.

VERIFIED: `gentity+0x300` is a free count that survives the free:
`G_FreeEntity` loads it at 0x66b8b, clears the record with
`__bzero(ent, 0x314)` at 0x66b97, and stores the value plus one at 0x66be6.
VERIFIED, a disassembly scan for the displacement: its other readers are
`Add_Ammo`, `Drop_Weapon`, `G_RunFrame`, `hurt_touch`,
`Activate_trigger_damage`, `G_Trigger`, `Touch_Multi`; what they use it for
was not read. It does not catch a stale script handle: that is the VM's own
`dead entity` type, measured by `probe_stale_handle`
(`docs/research/cod11-gsc-language.md` §9, with the call chain from
`G_FreeEntity` into the VM).

VERIFIED live on mp_chateau (2026-08-29): after the load the first spawn was
217, the second 218. Deleting the first and spawning gave 219, not 217; a
fourth spawn 220. After `wait 0.5` the next spawn was 217 and the one after
221, continuing from the high-water mark. So:

- A freed slot is handed back out ahead of the counter, FIFO.
- `delete()` does not free at the call; the free comes a tenth of a second
  later off the entity think. INFERRED FROM DECOMPILATION: the `delete` method
  (0x5da14) notifies, unlinks, clears the touch and use handlers, and sets
  `think = G_FreeEntity`, `nextthink = level.time + 100`; the two constants are
  VERIFIED at 0x5da9a and 0x5daa4. INFERRED, same reading: at map load the
  `SP_` functions call `G_FreeEntity` directly, with no delay.
- VERIFIED (1.1d `delete` at 0x5da14; UO 1.51b `game.mp.uo.i386.so` 0x68bda,
  same body): the notify is `Scr_AddEntity(self)` then
  `Scr_Notify(self, scr_const+0x1c, 1)`, and `scr_const+0x1c` is `"death"`
  (`GScr_LoadConsts`, 1.1d 0x58550 storing the string at 0x75f9e, UO 0x652d2
  the one at 0xa1c8a). A delete ends every `ent endon("death")` and wakes
  `ent waittill("death", e)` with the entity. UO's `clearVehiclePosition`
  (0x6950e) runs the same four steps inline on each entity in its box.
  INFERRED, the stock script: UO's `_spawnlogic::Player_WaitForDeath` holds
  `point endon("death")` and writes `point.delay_use` once its player dies,
  and `bas.gsc` deletes a base's spawn points (`BaseAssault_KillBase`
  3519-3530, `BaseAssault_base_think` 4029-4037); without the notify the write
  lands on a dead entity, a script error (seen on mp_foy bas with four
  shooting bots, seed 11).

`ObjectTable::schedule`/`run_thinks` (`crates/server/src/game/entity.rs`)
reproduce the think pass. A think is an absolute `nextthink` in `level.time`
ms; `run_thinks` fires every entity whose time has come and clears
`nextthink` before the call, so a think re-arms itself to repeat. A
`nextthink` of 0 is no think armed. INFERRED, from lineage, not this module:
Q3's `G_RunThink` (`g_main.c`, `if (thinktime <= 0) return;`); nothing in CoD
1.1 tells the two readings apart, since every arming site read writes a
positive absolute time.

The live measurement only bounds the 100 ms: `probe_delete`'s frames step
50 ms, so any defer in (0, 150] would have freed the entity before the
following assertion, and `probe_delete_matches_retail` inherits that limit.

## 15. The item registry (`bg_itemlist`), configstring 8, VERIFIED

`bg_itemlist` (`.data` 0x7b9d8, stride 0x30, `bg_numItems` = 70 at `.rodata`
0x70804, `python3 tools/re/dump_itemlist.py`) has a compiled-in classname for
five of its 70 rows: `item_ammo_stielhandgranate_open` (65),
`item_ammo_stielhandgranate_closed` (66), `item_health_small` (67),
`item_health` (68), `item_health_large` (69). Index 0 is blank. Indices 1-64
hold placeholders `emptyitem_"w01"` .. `emptyitem_"w64"`; no `mp40_mp`-shaped
string exists in the binary, so a weapon classname reaches its slot only at
runtime, from the mounted paks' weapon files, in an order the static dump
cannot recover.

**R1 (VERIFIED against two live captures):** a weapon's item index is its
1-based index into configstring 7's weapon list
(`crate::configstrings::WEAPON_LIST`). Decoding
`crates/server/tests/fixtures/configstrings/mp_{pavlov,carentan}-dm.txt` line
13 (configstring 8) by `SaveRegisteredItems`' packing (below) and naming each
set bit through `WEAPON_LIST` accounts for every bit, with no cross-theatre
names:

```
mp_pavlov   1ce0cfb40000000001  (17 bits)
  0 <null>  6 fg42_mp  7 fg42_semi_mp  9 kar98k_mp  10 kar98k_sniper_mp
  11 luger_mp  18 mosin_nagant_mp  19 mosin_nagant_sniper_mp  20 mp40_mp
  21 mp44_mp  22 mp44_semi_mp  23 panzerfaust_mp  24 ppsh_mp
  25 ppsh_semi_mp  27 rgd-33russianfrag_mp  30 stielhandgranate_mp
  68 item_health

mp_carentan 7df31f0d1000000001  (22 bits)
  0 <null>  1 bar_mp  2 bar_slow_mp  4 colt_mp  6 fg42_mp  7 fg42_semi_mp
  8 fraggrenade_mp  9 kar98k_mp  10 kar98k_sniper_mp  11 luger_mp
  12 m1carbine_mp  13 m1garand_mp  16 mg42_bipod_stand_mp  20 mp40_mp
  21 mp44_mp  22 mp44_semi_mp  23 panzerfaust_mp  28 springfield_mp
  30 stielhandgranate_mp  31 thompson_mp  32 thompson_semi_mp  68 item_health
```

Pavlov (Russian v German) sets only Russian and German weapons; Carentan
(American v German) only American and German. A wrong ordering would scatter
names across theatres.

**The packing (VERIFIED against both captures):** `SaveRegisteredItems`
(`.text` 0x4ef08) walks `0..bg_numItems`, accumulates `1 << (i & 3)` into a
nibble, and every fourth item emits one lowercase hex character (`+0x30` under
10, `+0x57` at or above), flushing the trailing partial nibble the same way.
70 items make 18 characters.

**M1, is index 0 unconditional?** `ClearRegisteredItems` (`.text` 0x4eecc)
resolves via its relocations to `__bzero(itemRegistered, 0x400)`, no special
case for 0. Index 0's classname is blank, so no `precacheItem` can name it.
`RegisterItem` (`.text` 0x4e504, mirrored as `Items::register`) is the only
writer of `itemRegistered` (`.bss` 0x18e0e0); its four call sites pass a
computed argument, never a literal 0, but two are inside `BG_GivePlayerWeapon`
(`.text` 0x36a38, sites +0x4f and +0xe1), which runs for every weapon a player
is given, the "no weapon" slot 0 included. INFERRED FROM DECOMPILATION (not
single-stepped): that is the trigger. VERIFIED: both retail captures set bit 0
with nothing per-map in common but the `dm` gametype. `Items::new` seeds index
0 registered unconditionally.

**M2, does registering a weapon also register its alt-fire mode?** Yes, as
weapon-definition behaviour. `RegisterItem`, after setting the item's bit,
checks `bg_itemlist[index]`'s `giType` (offset 0x20) against 1 (`IT_WEAPON`);
on a match it calls `BG_GetInfoForWeapon` (`.text` 0x3ac68, via `readelf -r`),
reads a "next" index at offset 0x2fc of the definition, registers it directly
(bypassing the "already registered" return), precaches two of its model fields
(0x188, 0x31c) through `G_ModelIndex` (`.text` 0x66ed8), and repeats until the
next index is 0 or loops back to the start. `BG_GivePlayerWeapon` (0x36a38,
`.text` 0x36b0a) reads the same 0x2fc field and registers it too when a player
is given a weapon. INFERRED FROM DECOMPILATION (not single-stepped): 0x2fc
carries the alt-fire link. VERIFIED: every `*_semi_mp`/`*_slow_mp` bit in both
captures sits at the `WEAPON_LIST` index right after its base weapon's
(`bar_mp`/`bar_slow_mp`, `fg42_mp`/`fg42_semi_mp`, `mp44_mp`/`mp44_semi_mp`,
`ppsh_mp`/`ppsh_semi_mp`, `thompson_mp`/`thompson_semi_mp`). VERIFIED, the
stock files: each of the five names the other in `altWeapon`, so
`Items::register` walks the parsed `WeaponDef::alt_index`
(`WeaponTable::alt_chain`), not list adjacency.

**M4, a registration after the first wait.** VERIFIED, the instructions of
`RegisterItem` (0x4e504): an item already registered returns at once
(0x4e518); otherwise a clear `level.initializing` (`level+0x1c`, 0x4e521)
formats `"game tried to register the item '%s' after initialization
finished\n"` (0x74de0) and calls `Scr_Error` (0x4e577). The name is
`bg_itemlist`'s +0x18 (`Small Health`, `Med Health`, `Large Health`,
`Stielhandgranate_mp Ammo Open` / `Closed`, read with `gameso.py`), or for a
weapon row (blank +0x18) the weapon file's internal name (`def+4`, what
`BG_GetWeaponIndexForName` compares). VERIFIED, the 1.5b and UO decompiles
carry the same branch (UO's flag at `level+0x20`). VERIFIED decompile:
`BG_GivePlayerWeapon` (0x36a38) calls `RegisterItem` on the weapon it gives,
so `giveWeapon` and `setWeaponSlotWeapon` of an unprecached weapon fail there
past the first wait. vcod: `GameHost::register_item` returns the error, and
the two builtins register what they give
(`giving_an_unregistered_weapon_past_initialization_is_an_error`).

**M3, the item's own models.** `RegisterItem` precaches two more model fields
for every item, at 0x8 and 0xc of its `bg_itemlist` record. VERIFIED, read
with `dump_itemlist.py`'s `Elf` helper: the five compiled-in rows carry a
non-empty +0x8 and a null +0xc (`xmodel/ammo_stielhandgranate1` (65),
`xmodel/ammo_stielhandgranate2` (66), `xmodel/health_small` (67),
`xmodel/health_medium` (68), `xmodel/health_large` (69)); rows 1-64 carry the
empty string in both.

VERIFIED, the captures and the shipped weapon files: a weapon's pair is its
file's `worldModel` and `projectileModel`, in that order and verbatim.
`_teams::precache()`'s American list on `mp_carentan` is `fraggrenade_mp
colt_mp m1carbine_mp m1garand_mp thompson_mp bar_mp springfield_mp`, and
slots 385-392 of the capture are `weapon_MK2FragGrenade`,
`projectile_USGrenade`, `weapon_colt45`, `weapon_M1Carbine`,
`weapon_M1Garand`, `weapon_thompson`, `weapon_bar`, `weapon_springfield`: the
grenade's two fields adjacent, one each for the seven weapons with no
`projectileModel`. The German list and `mp_pavlov`'s two lists resolve the
same way.

`crates/server/src/items.rs`'s `item_models` reproduces this;
`GameHost::register_item` holds the rest of `RegisterItem`: the bit, the
alt-fire link, both models per registered item, configstring 8.

INFERRED: the alt-fire link's model precache is the same pair. M2 reads retail
as precaching offsets 0x188 and 0x31c of the weapon definition for a chained
alt mode, a different struct from `bg_itemlist`'s +0x8/+0xc; that they name
the same strings is unconfirmed, and `register_item` uses +0x8/+0xc for base
and alt alike. VERIFIED, the choice is inert on stock content: all five
alt-fire files (`bar_slow_mp`, `fg42_semi_mp`, `mp44_semi_mp`,
`ppsh_semi_mp`, `thompson_semi_mp`) carry their base weapon's exact
`worldModel` and an empty `projectileModel`, so an alt registration interns
onto a slot the base already took.

## 16. The builtins the stock `dm` bootstrap reaches

Running the shipped `dm.gsc` to completion at map load needs four builtins
past the precache and cvar families. VERIFIED, read out of the module: every
address, table index, string literal and numeric constant below, and the pool
geometry (1024 records of 124 bytes, from the `index <= 0x3ff` loop bound and
the 0x7c stride). INFERRED: what each "retail" cell says the code does
(instruction sequencing and branch conditions). `placeSpawnpoint` alone is
measured against a running retail server (16.1).

| builtin | table | address | retail | ours |
|---|---|---|---|---|
| `placeSpawnpoint` | entity methods 37 | 0x5bedc | VERIFIED: three calls to `trap_TraceCapsule` (0x5bf72, 0x5bfb8, 0x5bff4, `R_386_PC32` relocations), each passing `playerMins` and `playerMaxs` (`R_386_32` relocations onto 0x7dce8 and 0x7dcf4, whose `.data` reads (-15, -15, 0) and (15, 15, 72) before `G_SetPlayerSize` writes the 70 height over it, 16.2), contents mask 0x2810011 and the entity's own number; the constants 128 (0x5bf45) and 262144 (0x5bf91). INFERRED: the first sweeps that capsule from the origin up 128, the second from the first's endpoint down 262144, the entity moves to the third's endpoint (16.3), the second trace's result word at results+0x28 goes into `gentity_t+0x7c` (further inferred to be the ground entity), and the third, zero-length at that endpoint, prints `WARNING: Spawn point entity %i is in solid at (%i, %i, %i)` when it starts solid | all three traces as the same capsule under the same mask (`CollisionWorld::box_trace`), and the move; nothing stored for `gentity_t+0x7c` (our trace carries no entity identity). 16.1 is the measurement |
| `setClientNameMode` | functions 89 | 0x5f208 | INFERRED: matches the argument against `auto_change` and `manual_change`, stores 0 or 1 in `level+0x210`, `Scr_Error("Unknown mode")` otherwise. VERIFIED, data reads: the two constants are `scr_const+0xfc`/`+0xfe`, named by `GScr_LoadConsts` 0x58550; the relocation table has exactly four `level+0x210` sites, this function's pair, `ClientUserinfoChanged` (0x421eb) and the name-change path at 0x5ba99 | recorded on the host, both errors faithful |
| `newHudElem` | functions 77 | 0x4b184 | INFERRED: first free `g_hudelems` record, zeroed but for `fontscale` 1.0 (0x4b19b), packed white `color` (0x4b1d9), `archived` 1 (0x4b1fc) and owner `0x3ff` (0x4b249); `Scr_Error("out of hudelems")` when full | allocation, pool size, failure and all four defaults |
| `<hudelem> setTimer` | hudelem methods 2 | 0x4b8e4 | INFERRED: exactly one parameter, seconds to ms with the x87 rounding mode set to round-up (0x4b942), rejects a result not above zero, zeroes +0x30..+0x48 and +0x60/+0x64/+0x68, then writes type 4 at +0x0 and the end time `level.time + ms` at +0x5c | faithful, the clear included |

The record is a tagged union keyed by the type word at +0x0; 0 is a free
record, so a fresh element is type 1. `setText` (0x4c590), `setValue`
(0x4c684), `setShader` (0x4b5b0), the four timers and the two clocks share one
prologue that zeroes the same block, then each writes its type and payload:

| Type | Payload |
|---|---|
| 1 | localized-string index at +0x68 |
| 2 | float at +0x64 |
| 3 | material index at +0x38, size at +0x30/+0x34 |
| 4..7 | deadline at +0x5c |
| 8, 9 | the two clocks |

None of those offsets is in `HUD_FIELDS`, so script reads none back; `label`
(+0x2c) is script-readable and no method touches it.

`newClientHudElem` (0x4b298) and `newTeamHudElem` (0x4b3d0) allocate from the
same pool and write one filter field each, owner at +0x70, team at +0x74.
Which client each reaches, and on what wire, is
`docs/research/cod11-hud-protocol.md` section 5.

`thread addBotClients()` is commented out in the shipped `dm.gsc`, so nothing
in the stock bootstrap reaches `addTestClient` (section 25).

### 16.1 `placeSpawnpoint` drops a player capsule, measured

`probe_placespawn.gsc` runs as the gametype, calls `placeSpawnpoint` on every
`mp_deathmatch_spawn` and logs the origin before and after. VERIFIED, run
2026-09-26 against retail 1.1d on all twelve stock maps: 383 spawnpoints,
`crates/server/tests/fixtures/spawnpoints/placespawn-retail.txt`. VERIFIED,
same capture: mp_harbor's entity 241 at (-10548, -8572, -4) lands at z 0.12,
and entity 137 at (-8144, -6952, 120) at z 8.12.

VERIFIED, our collision on that map: 241's origin is 4 units inside a caulk
brush whose top is the snow floor at z 0, and on the vertical line through 137
nothing lies between z 120 and that floor. INFERRED: a point drop cannot land
137 at 8.12, which only a shape reaching sideways does, and retail's upward
sweep out of 241's brush does not stop at the floor's underside, where a point
trace against our facet clip stopped before falling through the world.

VERIFIED, `crates/server/tests/placespawn_ab.rs` on the fixture: the point
drop put 17 spawnpoints more than 3 units off retail's height, harbor's 137
and mp_depot's 96 by 8, and harbor's 241 at z -262144, below the world.
VERIFIED, a `--net-probe` joined to vcod on mp_harbor with its spawn forced
there: `groundEntityNum` 1023 and a falling origin every snapshot, z -633838
and 24461 u/s down after 32 s. With the capsule every spawnpoint was within
2.63 units of retail and 318 of 383 within the log's two decimals; 16.2 and
16.3 close the rest.

### 16.2 The same drop to the float's last bit, on every stock map

`client-probes/probe_placebits.gsc` runs as the gametype, calls
`placeSpawnpoint` on every spawnpoint of the classes the stock gametypes drop
(dm, tdm, sd, re, UO's `mp_uo_spawn_*` and `mp_gmi_bas_*`) and logs the height
as sign, whole part and 2^-24ths. VERIFIED, run 2026-10-19 against retail
1.1d on its twelve stock maps, 1.5b on its sixteen (the twelve, mp_bocage,
mp_neuville, mp_stalingrad, mp_tigertown) and UO 1.51b on its nineteen and
mp_carentan: `crates/server/tests/fixtures/spawnpoints/placebits-retail.txt`
(1,895 spawnpoints on 1.1d, 2,732 on 1.5b, 5,781 on UO). VERIFIED, same
capture: 1.5b and UO agree to the bit on mp_carentan, the one map both ran;
1.1d differs from 1.5b on 451 of the twelve shared maps' 1,895, mostly in the
last bits of terrain contacts (the stored-float build, cod15-movement.md 4 and
7). UO's mp_cassino ends in a script error from `_tankdrive_gmi.gsc` after the
last line.

VERIFIED, `placespawn_ab`'s three `placespawnpoint_*_to_the_bit` gates:
`placespawn_ab` places every spawnpoint of all three servers to the bit. Each
rule below closed rows:

| Rule | Rows it closed |
|---|---|
| The 2.62 off in 16.1 is the terrain clip's 1e-5 back-off over the 262144-unit drop; the last near tie of that kind went with `TraceInput::reach` (bsp-ibsp59-format.md, "The leaf walk") | 16.1's 65 rows (with the 72-high box, 141 rows still missed: 25 on 1.1d, 34 on 1.5b, 82 on UO, then in `BITS_GAPS`) |
| The 70-high box (below) | all of UO's 82 and 1.5b's but one: UO mp_sicily 181..186 (capsule top touched the 1136 underside of roof brushes 1259 and 3508 and swept up through them), mp_uo_stanjel 557 (in playerclip brush 317), mp_uo_powcamp 288, 332, 334 (under a sloped ceiling); 1.1d mp_powcamp 287, 288 against the brush clip's real `d1 > 0` test; most last-bit residuals (centre and sphere offset 35 and 20, not 36 and 21, round differently, and a slope contact reads that) |
| The third trace's endpoint (16.3) | 1.1d's last 17 last-bit residuals (mp_brecourt, mp_dawnville, mp_hurtgen, mp_rocket; 64 to 256 2^-24ths) |
| The leaf walk's brush rule | 1.5b mp_neuville 90 (128 2^-24ths low) |

VERIFIED, vcod measurement 2026-10-20, neuville 90: ours' ceiling check from
(-14688, 5072, 8) stopped at fraction 0.6396 on caulk brush 2647 (x
-15152..-14624, y 4720..5088, z 160..312, roofed by two 45-degree planes, so
at z 160 and above it reaches only y 5032). The capsule meets the pushed-out
underside past the roof slope's unbevelled edge, 40 units off the brush, and
no leaf the sweep enters lists it, so retail never clips it there
(bsp-ibsp59-format.md, "The leaf walk"). VERIFIED, same recomputation: the drop
from the full 128 up lands on retail's 2 + 12574720/2^24, from ours' 89.875 on
12574592. The face clip and plane build were right: an x87 80-bit
recomputation of 1.5b's `PlaneFromPoints` (0x806aa54, `CrossProduct`
0x80667a1, `VectorNormalize` 0x8066845) and of the face clip gives vcod's
plane and fraction to the bit.

The box is 70 high, not the 72 `playerMaxs` holds in `.data`. VERIFIED,
`tools/re/annotate_func.py` on each `G_InitGame`: it calls `G_SetPlayerSize`
after registering the cvars and before `Scr_LoadGameType` (1.1d 0x4fc1f and
0x4fe6f, 1.5b 0x4f195 and 0x4f542, UO 0x5875a and 0x58b48). VERIFIED,
`G_SetPlayerSize` (1.1d 0x42fd4, 1.5b 0x3d193, UO 0x45692) writes
`playerMaxs.z` from `g_bounds_height_standing` and the x and y extents from
`g_bounds_width` times 0.5 and -0.5. VERIFIED, `tools/re/dump_cvars.py`:
defaults "70" and "30" on all three. So every `placeSpawnpoint` traces a
(-15, -15, 0)..(15, 15, 70) capsule.

Every reader of the two globals in 1.1d, from the `R_386_32` relocations onto
0x7dce8 and 0x7dcf4 (`readelf -r`, `annotate_func.py`):

- VERIFIED: `ClientSpawn` copies both into `r.mins`/`r.maxs`
  (0x42775-0x427b7), then those into the playerstate's box at
  `ps+0x324..0x338` (0x428b2-0x428f4).
- VERIFIED: `G_RunFrame` (0x50711-0x50843) keeps the last `g_bounds_width` and
  `g_bounds_height_standing` it saw at `level+0x29fc` and `+0x2a00`. INFERRED,
  off the compares at 0x50720 and 0x5073a: when either changed it calls
  `G_SetPlayerSize` (0x5075c) and writes the new box into `r.mins`/`r.maxs`
  and `ps+0x324..0x338` of each of the 64 client entities in use, relinking
  each (`trap_LinkEntity` 0x5082f).
- VERIFIED: `positionWouldTelefrag` (0x5a834) builds its `trap_EntitiesInBox`
  box (mask 0x2000000) from them around its argument.
- VERIFIED: `placeSpawnpoint`'s three traces (0x5bf61-0x5bfeb), above.
- VERIFIED, the `Pmove` side: `PM_CheckDuck` (0x316f4) loads `pm.mins` and
  `pm.maxs` from `ps+0x324..0x338` (Ghidra's `FUN_000416f4`, image 0x10000
  above the module's addresses). INFERRED: the standing top is `ps+0x338`, the
  crouched and prone ones its constants 50 and 30.

VERIFIED, `tools/re/dump_cvars.py`: `g_bounds_width` and
`g_bounds_height_standing` carry flags 0x200 (`CVAR_CHEAT`) in 1.1d, 1.5b and
UO, so they hold "30" and "70" unless `sv_cheats` is on. vcod reads neither:
the box is the constants `pmove::HALF_WIDTH` and `pmove::HEIGHT_STAND`
(`game::trigger::PLAYER_MINS`/`PLAYER_MAXS` on the server). That matches
retail except a `devmap` whose rcon changes one of the two cvars, which retail
turns into a resize of every player and vcod ignores.

### 16.3 The spawnpoint takes the third trace's endpoint

VERIFIED, `game.mp.i386.so` 1.1d 0x5bedc (`annotate_func.py`): the three
`trap_TraceCapsule` calls write one result buffer at `ebp-0x30` (0x5bf6b,
pushed again from `[ebp-0x54]` at 0x5bfb4 and 0x5bff0). The third's start and
end are both `ebp-0x3c`, copied from the second's endpoint at
0x5bfc4-0x5bfd3. After it, `G_SetOrigin` (0x5c086) gets `ebp-0x2c`, the
buffer's endpoint: the spawnpoint lands where the zero-length test ends, not
where the drop ended. The in-solid warning tests the buffer's byte +0x2e
(0x5bffc).

VERIFIED, `cod_lnxded` 1.1d `CM_BoxTrace` (0x80563d1-0x8056420 and
0x8056887): the delta is the shifted end, left on the x87 stack, minus the
shifted start, stored as a float; the endpoint is `start + fraction * delta`
off the unshifted start. INFERRED, from that arithmetic: a zero-length trace at
height `z` whose capsule centre `z + 35` needs one bit more than a float holds
gets the centre's rounding error as its delta (half a float step either way),
and its endpoint moves by it; `collision::cm_endpos` computes that. INFERRED,
from 1.5b's store of the shifted end (`cm_endpos` under `Arith::Stored`): on
1.5b and United Offensive the delta is 0 and the endpoint stays.

VERIFIED, `placespawn_ab` on `placebits-retail.txt` (2026-10-19, 70-high box):
reading the third trace's endpoint places all 1,895 of 1.1d's spawnpoints to
the bit, closing the 17 rows 16.2 lists (mp_brecourt 245, mp_dawnville 125 and
166, mp_hurtgen 146 and 147, twelve on mp_rocket), and moves no 1.5b or UO
line.

## 17. Placed weapons register their item at spawn, before any script runs

A map's placed weapon never reaches `spawns`. VERIFIED, addresses and
relocations from the binary: `G_CallSpawn`'s second case (section 7) matches
the classname against `bg_itemlist` with `strcmp` before `spawns`; a hit calls
`G_SpawnItem` (0x4e634) with the entity and the matched record. Inside it:
`sub bg_itemlist,%eax` at 0x4e676 (an `R_386_32` relocation onto
`bg_itemlist`), `imul $0xaaaaaaab; sar $0x4` at 0x4e67b/0x4e689 (the
reciprocal for dividing by the 0x30 stride), and the call at 0x4e68d, an
intra-module call objdump names outright, to `RegisterItem` (0x4e504).

INFERRED FROM DECOMPILATION (control flow, not single-stepped):
`G_SpawnItem` derives the item index from the record's offset into
`bg_itemlist` and passes it to `RegisterItem`, and neither function frees the
entity as section 13's five do. Backed by the outcome below: retail's
configstring 8 keeps the `fg42_mp`/`panzerfaust_mp` bits after the map's own
script deletes those entities, possible only if registration already happened
and survives the delete.

A weapon slot's `bg_itemlist` classname is not the `WEAPON_LIST`/configstring
7 name (`fg42_mp`) but the weapon's own `radiantName`, not always a simple
transform of the file name. VERIFIED, every `weapons/mp/*_mp` file in the
shipped paks: 23 of `WEAPON_LIST`'s 32 weapons carry a non-empty
`radiantName`.

| Shape | Count | Examples |
|---|---|---|
| `mpweapon_` plus the file name | 19 | `fg42_mp` -> `mpweapon_fg42`, `kar98k_mp` -> `mpweapon_kar98k`, `panzerfaust_mp` -> `mpweapon_panzerfaust` |
| underscore dropped from a compound | 3 | `kar98k_sniper_mp` -> `mpweapon_kar98k_scoped`, `mosin_nagant_mp` -> `mpweapon_mosinnagant`, `mosin_nagant_sniper_mp` -> `mpweapon_mosinnagantsniper` |
| unrelated to the file name | 1 | `rgd-33russianfrag_mp` -> `mpweapon_russiangrenade` |
| key present but empty (alt-fire files, reached only through the alt-fire link) | 5 | `bar_slow_mp`, `fg42_semi_mp`, `mp44_semi_mp`, `ppsh_semi_mp`, `thompson_semi_mp` |
| no key (a mounted mg42 is placed as `misc_mg42`, below) | 3 | `mg42_bipod_*` |
| no weapon file under that name | 1 | `ptrs41_antitank_rifle_mp` |

Hence `crates/server/src/game/spawn.rs` carries `RADIANT_NAMES` as a
transcribed table, not a prefix-and-suffix rule.

VERIFIED, both gate maps' entity lumps: `maps/MP/mp_pavlov.bsp` (pak5) places
two `mpweapon_fg42`, eight `mpweapon_panzerfaust` and two `mpweapon_mp44`;
`maps/MP/mp_carentan.bsp` (pak4) two `mpweapon_fg42` and eight
`mpweapon_panzerfaust`.

VERIFIED, the shipped `maps/MP/gametypes/_teams.gsc` (pak5):
`restrictPlacedWeapons` calls `deletePlacedEntity("mpweapon_fg42")` and
`deletePlacedEntity("mpweapon_panzerfaust")` when the matching `scr_allow_*`
cvar is unset, and `deletePlacedEntity` is `getentarray(classname,
"classname")` then `.delete()` on each. So retail's configstring 8 keeps the
`fg42_mp` bit though the placed fg42s are gone by round start (stock
`scr_allow_fg42` is `"0"`, `cod11-gsc-language.md` section 9): registration
happened at spawn, and `delete()` frees only the entity, a tenth of a second
later (section 14). VERIFIED, the retail pickup capture
(`crates/server/tests/fixtures/items/mp_carentan-dm-pickup-script.txt`, stock
`scr_allow_panzerfaust`): all eight placed panzerfausts survive the map load,
so the `panzerfaust_mp` bit comes from live entities (`cod11-items.md` section
12.1).

INFERRED FROM DECOMPILATION, the one open link: the function that copies a
weapon's `radiantName` into its `bg_itemlist` row at weapon-file load is
untraced (a raw-pointer scan found `radiantName` as a field name in a
weapon-definition table in `.data` near `bg_itemlist`, no proof of the copy).
The VERIFIED facts above (the assets carry each `radiantName`; the BSPs place
entities with exactly that classname, reached only by `bg_itemlist`'s match in
`G_CallSpawn`) leave no other candidate.

`misc_mg42` and `misc_turret` (section 8) reach `RegisterItem` by a second
path: their `SP_` function is `SP_turret` (0x533b0). VERIFIED, addresses and
relocations: `SP_turret`'s body calls `G_SpawnString` (0x533c8 relocation)
with the key `weaponinfo`, `Com_Error` (0x533de relocation) with code 1 and
`"no weaponinfo specified for turret"`, and `G_SpawnTurret` (0x52c84, body not
traced). INFERRED FROM DECOMPILATION: `Com_Error` is the arm for an absent
key, `G_SpawnTurret` for a present one, so a mounted mg42's item name is its
`weaponinfo` value, verbatim. VERIFIED at the functional level:
`mp_carentan.bsp`'s two `misc_mg42` blocks carry
`"weaponinfo" "mg42_bipod_stand_mp"`, and retail's configstring 8 on that map
sets exactly bit 16 (`mg42_bipod_stand_mp`) beyond the
`fg42_mp`/`fg42_semi_mp`/`panzerfaust_mp` bits `mp_pavlov` also sets: the one
extra bit the key predicts.

`spawn_item_name` (`crates/server/src/game/spawn.rs`) reproduces both rules (a
`RADIANT_NAMES` lookup for `mpweapon_*`, a direct `weaponinfo` read for
`misc_mg42`/`misc_turret`), run for every spawned entity beside `SPAWN_FREES`
and independent of it, so a classname that both registers and frees does
both.

## 18. What fills the model configstring block

Three things write into 269-523:

1. Every non-brush `model` key in the entity lump, during
   `G_SpawnEntitiesFromString` (section 7).
2. Each registered item's `worldModel` and `projectileModel`, wherever the
   item is registered: at spawn for a placed weapon or a mounted mg42
   (section 17), inside `precacheItem` for the script's list (section 15, M3).
3. The script's own `precacheModel` and `setModel`, continuing from there.

VERIFIED, the two captures in `crates/server/tests/fixtures/configstrings/`:
the three interleave. On both maps an item registration's model sits between
two of the lump's, and the script's precaches start only after the last of
either. INFERRED, a generalisation from two maps: the interleave is strictly
call order, so one contributor wrong offsets everything after it rather than
leaving a hole; supported by `G_ModelIndex` (section 7) being the one writer
on all three paths.

The arithmetic, VERIFIED slot by slot against both captures:

| | `mp_pavlov` | `mp_carentan` |
|---|---|---|
| entity-lump `model` keys, distinct and non-brush | 27 | 57 |
| item models registered at spawn | 1 | 1 |
| item models from `precacheItem("item_health")` | 1 | 1 |
| **first slot the script's own precaches reach** | **298** | **328** |
| block ends at | 361 (93 models) | 399 (131 models) |

The spawn-time item model on each map is `xmodel/weapon_panzerfaust_rocket`,
the `projectileModel` of the `panzerfaust_mp` both maps place; every other
placed weapon's `worldModel` is already the placing block's own `model` key
and interns onto it, and the `mg42_bipod_*` files carry no `worldModel`. The
script-side one is `xmodel/health_medium`, `dm.gsc`'s
`precacheItem("item_health")` two lines before `_teams::precache()` starts the
player models.

**`g_useGear` decides 49 slots on `mp_carentan` and 41 on `mp_pavlov`.**
VERIFIED, measured directly: seeding the cvar took
`crates/server/tests/configstrings_ab.rs` from 53 differing slots to 4 on
`mp_carentan` and from 43 to 2 on `mp_pavlov`, with no other change. Every
`character/mp_*.gsc` gates its gear model precaches on
`character\_utility::useOptionalModels()`, which is `getcvarint("g_useGear")`
(VERIFIED, the shipped `pak5.pk3`); the gear models sit mid-block, so an unset
cvar loses them and offsets every later player and weapon model.

VERIFIED, `python3 tools/re/dump_cvars.py game.mp.i386.so usegear`: the game
module's cvar table is `.data` 0x7de28, 71 records of 24 bytes
`{vmCvar_t *, name, default, flags, trackChange, teamShader}` ending at a null
name; `g_useGear` is index 67 (record at `.data` 0x7e470), default `"1"`,
flags 0x21. The 21 rows flagged 0x800 are exactly
`crates/server/src/cvars.rs`'s `ENGINE_MIRRORED`, so `g_useGear` never
appears in the 140/204 mirror and is recoverable only from the table.

**The other 70 rows are not transcribed, and nothing warns when one is
missing.** A script reading an unregistered cvar gets `""`, which
`getcvarint` reads as 0, silently. `crates/server/src/cvars.rs` carries only
rows whose absence was measured; for the next miss, run the dumper with the
cvar's name and take the row's default. Two the stock MP scripts read and vcod
does not carry: `g_allowVote` (index 26, default `"1"`) and `g_debugDamage`
(index 22, default `"0"`); neither changes a measured outcome
(`g_debugDamage`'s 0 is what an absent cvar reads).

**`scr_*` cvars come from `default_mp.cfg`, not that table.** VERIFIED, the
binaries and paks: `cod_lnxded` carries the literal `exec default_mp.cfg\n`
(`.rodata` 0x080cf6de); the file ships in `localized_english_pak0.pk3`; its
140 lines set 45 cvars, line 95 `set scr_allow_fg42 0`. No `scr_*` name is in
the module's cvar table. 38 of the 45 are `scr_*` read by the stock gametypes:
seventeen `scr_allow_*` (`scr_allow_vote`, the eighteenth, comes from the
script alone), the `scr_dm_*`, `scr_sd_*`, `scr_re_*` and `scr_bel_*` limits,
`scr_friendlyfire`, `scr_drawfriend`, `scr_forcerespawn`. The other seven are
the client's mouse settings and `ui_allow_sniperrifles`. All 38 but one are
invisible in a capture because the file's value and the script's
`makeCvarServerInfo` default agree. `scr_allow_fg42` disagrees: the file says
`0`, `_teams::initGlobalCvars()` passes `"1"`, and `makeCvarServerInfo` keeps
the value already there; hence the only `scr_allow_*` the gate surfaced (slot
164/228). `Server::cvars` reads the file out of the mounted paks and applies
its `set` lines. Plain `set` is right: nothing the file names is in the
module's table, so there is no registration order, and `set` never flags a
cvar into the 140/204 mirror. A file value reaches the mirror only when a
script later flags that name, so seeding all 45 leaves the mirror
byte-identical; the gate still matches both maps slot for slot.

## 19. The last two configstring mechanisms: `northyaw` and two `SP_` sound aliases

These closed `configstrings_ab.rs`'s last four slots on `mp_carentan` and two
on `mp_pavlov`: a raw text copy and two more calls into `G_SoundAliasIndex`
beside section 17's `RegisterItem`.

**`northyaw`, configstring 11.** VERIFIED, data reads: `SP_worldspawn`
(0x61cec) calls `G_SpawnString("northyaw", "", &out)` (`0x61e18`-`0x61e1d`),
then on `*out` either `trap_SetConfigstring(11, out)` or the compiled-in
literal `"0"` (`0x7964b`) (`0x61e28`-`0x61e3d`). VERIFIED, both arms' outcome:
the committed captures take one each, `mp_pavlov` the present key and
`mp_carentan` the absent one. It is a raw copy of the key text, never a number
the engine formats (no `G_SpawnFloat`, `strtol` or `strtod` in the sequence),
so vcod does not route it through `Cx::format_number`.

VERIFIED, the shipped BSPs: `mp_pavlov`'s worldspawn has `"northyaw" "90"`;
`mp_carentan`'s has no `northyaw`, so its capture's `"0"` is the literal
fallback, not a rendered zero. `worldspawn_northyaw`
(`crates/server/src/game/spawn.rs`): the key's value if present and
non-empty, else `"0"`.

**`world_hurt_me`, configstring 525.** INFERRED FROM DECOMPILATION:
`SP_trigger_hurt` (0x64ef8) calls `G_SpawnString("sound", "world_hurt_me",
&out)` (`0x64f8e`-`0x64f98`) unconditionally, before the entity's other
fields, then `G_SoundAliasIndex(out)` (`0x64fa4`). VERIFIED, it explains both
captures: both gate maps place exactly one `trigger_hurt` block with no
`sound` key (the shipped BSPs), so the default `"world_hurt_me"` (`0x7998c`) is
what every stock map registers, `mp_pavlov` included though it has no
`misc_mg42`.

**`weap_mg42_loop` / `weap_mg42_cooldown`, configstrings 526/527.**
VERIFIED, the table row and call target: `misc_mg42` and `misc_turret` name
`SP_turret` (0x533b0) in `spawns` (section 8), and its body calls
`G_SpawnTurret` (0x52c84); which arm reaches that call is section 17's
INFERRED reading, the link left untraced there.

VERIFIED, only the presence of these calls and offsets: `SP_turret` and
`G_SpawnTurret` sit at 0x533b0/0x52c84 in the dynamic symbol table; the call
at `0x52d74` targets `RegisterItem`, those at `0x52dad`/`0x52dd8`
`G_SoundAliasIndex`; `BG_GetInfoForWeapon` is called; `edi+0xa0`/`edi+0xa4` are
read offsets in that code. INFERRED FROM DECOMPILATION, the order and meaning:
`G_SpawnTurret` calls `RegisterItem(weaponinfo)` (section 17), reads the two
fields off `BG_GetInfoForWeapon`'s pointer, and calls `G_SoundAliasIndex` on
each that is a non-null, non-empty string, registering sound aliases for the
same weapon.

VERIFIED, the shipped `weapons/mp/mg42_bipod_stand_mp` (`pak0.pk3`), the file
both gate maps' mounted mg42s name in `weaponinfo`: `loopFireSound` and
`stopFireSound` are `weap_mg42_loop` and `weap_mg42_cooldown`, exactly
`mp_carentan`'s captured 526/527 in that order. So the alias names are
weapon-file data reached through `weaponinfo`, as `RegisterItem`'s item name
is; `turret_sound_aliases` (`crates/server/src/game/spawn.rs`) reuses
`spawn_item_name`'s resolved string. `mp_carentan` places two `misc_mg42`
blocks naming the same file (section 8's live counts), and
`CsRange::SoundAlias`'s intern-or-append allocator keeps the second from
taking two more slots.

## 20. The spawn path: `ClientSpawn`, the weapon builtins, `positionWouldTelefrag`

What the stock `spawnPlayer()` reaches once a client has answered both menus:
five builtins and one name table.

### `spawn` is two builtins under one name

VERIFIED, the tables `dump_builtins.py` walks: `spawn` (section 9) is
`functions[8]` at 0x5d268, `spawn(classname, origin)`, which allocates a map
entity, and player method 40 at 0x455cc, `self spawn(origin, angles)`.

0x455cc is a wrapper: it bound-checks the entity number against 0x3ff,
indexes `g_entities` by 0x314, reads two vectors with `Scr_GetVector`, and
tail-calls `ClientSpawn` (0x4268c). VERIFIED: the `R_386_PC32` relocation at
0x45652 names `ClientSpawn`, and `nm -D` puts it at 0x4268c. 0x455cc is the
builtin around `ClientSpawn`, not `ClientSpawn` itself. INFERRED: it raises
`entity %i is not a player` (0x73140) when `ent->client` is null (the string
is in the binary; the branch is control flow).

VERIFIED, the shipped `maps/mp/gametypes/*.gsc`: `spawnSpectator()` and
`spawnIntermission()` call the same `self spawn(origin, angles)` as
`spawnPlayer()`; the mode comes from the `sessionstate` they set just before.

UNVERIFIED: whether `ClientSpawn` clears `ps.weapons` (0x4268c not read).
INFERRED, from the stock scripts' order: they require it, since
`spawnPlayer()` calls `spawn` before `_teams::loadout()` and `giveWeapon`
errors on an occupied slot (below), so a player who changed side would fail to
respawn if his last life's pistol still held the pistol slot.

### `positionWouldTelefrag(origin)`, `functions[96]` 0x5a834

Takes one vector, no receiver, returns 0 or 1 through `Scr_AddInt`. It adds two
rodata vectors to the argument to build a box, hands it to
`trap_EntitiesInBox` (0x63a78) with a 1024-entry buffer and mask 0x2000000,
and walks the result. VERIFIED: the relocations at 0x5a84b, 0x5a8d8 and
0x5a904/0x5a91b name `Scr_GetVector`, `trap_EntitiesInBox` and `Scr_AddInt`.
INFERRED, the branches at 0x5a8f4-0x5a8fc: 1 for the first entity whose
`ent->client` (+0x158) is non-null and `client->ps.pm_type` (+0x4) is not
above 5; 0 when the walk runs out. Playerstate +0x4 is the `pm_type` offset of
`cod11-sound-system.md` and `cod11-hud-protocol.md`.

VERIFIED, `tools/re/annotate_func.py`: the two rodata vectors are
`playerMins` and `playerMaxs`. vcod (`builtins::client::position_would_telefrag`)
walks its area tree (`crate::area::AreaTree`, `cod11-combat.md` 14.7) over the
same box and mask and filters on the client and its last wire `pm_type`: a
client counts where his last cmd, spawn or `setOrigin` linked him; a spectator
(contents 0) and a dead player (`CONTENTS_CORPSE`) never.

### `weaponSlot`: the name table at `.data` 0x7c940

VERIFIED, read through `.rel.data`: six pointers, `none` (0x70f6b),
`primary` (0x70f8d), `primaryb` (0x70f84), `pistol` (0x70f7d), `grenade`
(0x70f38), `smokegrenade` (0x70f70). They carry `R_386_RELATIVE` relocations,
so the raw dwords are the addends and do resolve, unlike section 1's function
pointers. A weapon's slot is 1..=5; 0 is empty. VERIFIED: the five non-empty
names are exactly those of retail's `Unknown weaponslot name %s. Valid
weaponslots are "primary", "primaryb", "pistol", "grenade", and
"smokegrenade"` (0x73220).

Trap: `.data` sits at vaddr 0x7b3a0, file offset 0x7a3a0, so raw offset
0x7b940 is vaddr 0x7c940. Reading the file offset as an address lands on the
`EV_*` name table (`EV_MELEE_HIT`, `EV_MELEE_MISS`, `EV_FIRE_WEAPON_MG42`).

### `ps.weapons` and `ps.weaponslots`

`ps.weapons` (playerstate +780, netfields `weapons[0]` and `weapons[1]`) is a
bitset indexed by the weapon's 1-based configstring 7 position.
`ps.weaponslots` (+788, netfields `weaponslots[0]` and `weaponslots[4]`) is
eight bytes, one weapon index per slot of the table above, 0 for empty.
VERIFIED, both captures in `crates/server/tests/fixtures/playerstate/`:

| Map | Loadout | `weapons[0]` | `weaponslots[0]` | `weaponslots[4]` |
|---|---|---|---|---|
| `mp_carentan`, americans | `colt_mp` (4), `fraggrenade_mp` (8), `m1carbine_mp` (12) | 4368 (bits 4, 8, 12) | 0x04000C00 (bytes `00 0C 00 04`: carbine `primary`, colt `pistol`) | 8 (grenade) |
| `mp_pavlov`, russians | `luger_mp`, `mosin_nagant_mp`, `rgd-33russianfrag_mp` | 134481920 (bits 11, 18, 27) | 0x0B001200 | 27 |

VERIFIED, `weapons/mp/*` in `pak0.pk3`: `m1carbine_mp` names
`weaponSlot primary`, `colt_mp` `pistol`, `fraggrenade_mp` `grenade`.

### `giveWeapon`, player method 0, 0x43020

VERIFIED, every call a named `R_386_PC32` relocation, in this order:
`Scr_GetString` (0x6bd40), `BG_GetWeaponIndexForName` (0x3adc0),
`Com_BitCheck` (0x6b164) on `client+0x30c` (`ps.weapons`, the same +780),
`BG_GetEmptySlotForWeapon` (0x3aaac), `BG_GivePlayerWeapon` (0x36a38), then
`Add_Ammo` for the difference between the definition's start ammo (+0x198)
and the current `ps.ammo` entry (`client+0x10c`, indexed by the definition's
ammo type at +0x1a0).

A zero from `BG_GetEmptySlotForWeapon` raises `Can not give player weapon
without having an empty weapon slot` (0x73180). INFERRED: that is the branch
at 0x430c1; whether re-giving a held weapon counts as an occupied slot is not
measured. The `Com_BitCheck` result rides a register to the `Add_Ammo` call,
not a guard here.

INFERRED, from the instruction sequence: `BG_GetWeaponIndexForName`'s result
is narrowed with `movzbl %al` at 0x43091 and reaches `Com_BitCheck` untested,
so a name no weapon file backs sets bit 0 and carries on. `ps.ammo` has no
netfield in the 1.1 playerstate, so nothing written there reaches a client.

### `giveMaxAmmo`, player method 7, 0x43134

VERIFIED, the relocations: `Scr_GetString`, `BG_GetWeaponIndexForName`,
`Com_BitCheck`, then `BG_GetInfoForWeapon` (0x3ac68), `BG_GetAmmoTypeMax`
(0x3aca0) and `Add_Ammo` (0x4ca10). INFERRED, the branch at 0x431c3: it does
nothing when the `Com_BitCheck` fails. `ps.ammo` is no netfield, so this
builtin cannot move the wire.

### `setSpawnWeapon`, player method 23, 0x452a4

`Scr_GetString`, `BG_GetWeaponIndexForName`, `Com_BitCheck` on
`client+0x30c`, then `ps.weapon` (+0xb0, netfield offset 176) takes the index
and `ps.weaponstate` (+0xb4, offset 180) takes 0. VERIFIED, the two stores at
0x45339 and 0x45345 and the netfield offsets in
`crates/common/src/net/cod1/fields_v1.rs`. INFERRED, the branch at 0x4532f:
the stores are skipped for a weapon the player does not hold.

### The slot and switch methods: table positions only

VERIFIED, the player method table `dump_builtins.py` walks (two carry symbols,
six resolve to addresses):

| Method | Index | Address |
|---|---|---|
| `takeAllWeapons` (`PlayerCmd_takeAllWeapons`) | 2 | |
| `switchToWeapon` (`PlayerCmd_switchToWeapon`) | 5 | |
| `getWeaponSlotWeapon` | 32 | 0x43cf4 |
| `setWeaponSlotWeapon` | 33 | 0x43df4 |
| `getWeaponSlotAmmo` | 34 | 0x43ff4 |
| `setWeaponSlotAmmo` | 35 | 0x44130 |
| `getWeaponSlotClipAmmo` | 36 | 0x442c8 |
| `setWeaponSlotClipAmmo` | 37 | 0x443e4 |

VERIFIED, the compares and calls of the two setters' clamps:

- `setWeaponSlotClipAmmo` zeroes a negative count (`xor esi,esi` at 0x444d7
  behind the `jge` at 0x444d5), calls `BG_GetAmmoClipSize` and replaces a
  count above it (0x444dd-0x444f2), and stores at `client+0x20c+4*clip`
  (0x444f4-0x444ff).
- `setWeaponSlotAmmo` branches on `BG_WeaponIsClipOnly` (0x4420f) into the
  same clip clamp (0x44244-0x44254) or a reserve clamp against
  `BG_GetAmmoTypeMax` (0x44294-0x442a4).

VERIFIED, the retail round-restart target capture: `sd.gsc`'s
`setWeaponSlotClipAmmo("primary", 999)` reads back as `ammoclip=7:5` for the
kar98k. INFERRED: the reserve's cap is the ammo type's maximum, which vcod
reads as the slot weapon's own `maxAmmo`; no stock pair sharing an ammo name
ships different values, so `sd.gsc`'s 999 (lines 541-542) lands as the file's
`maxAmmo`.

The rest is vcod's design: a slot name resolves through the `weaponSlot`
table, ammo and clip are addressed by the slot weapon's own ammo and clip
index, and `switchToWeapon` goes through the putaway rather than swapping in
place (the weapon machine of `docs/research/cod11-combat.md` 1.8, not anything
read near 0x452a4). All six slot methods are implemented
(`builtins::client`; the two ammo getters in `builtins::uo`).

### `setWeaponSlotWeapon` inside, and its `"none"` argument

VERIFIED: the slot argument goes through `BG_GetWeaponSlotForName` at 0x43e7b;
a zero result is a `Scr_ParamError` carrying `"Unknown weaponslot name %s.
Valid weaponslots are ..."` (0x73220) at 0x43ea6.

VERIFIED: the weapon argument is compared against `"none"` (0x731c0) with
`Q_stricmp` at 0x43ec3; a match stores weapon index 0 at 0x43ecf and jumps to
0x43f6b, past `BG_GetWeaponIndexForName` (0x43ee4), its `Scr_ParamError`, and
the slot check at 0x43f26 that refuses a weapon whose file names a different
`weaponSlot`.

VERIFIED: at 0x43f6b the slot's byte is read from `client+0x314` and handed to
`BG_TakePlayerWeapon` (0x36b78) when non-zero (0x43f7e); `BG_GivePlayerWeapon`
(0x36a38) is reached only past a `cmp` of the resolved index against 0 at
0x43f8d. INFERRED, off those branches: `setWeaponSlotWeapon(slot, "none")`
empties the slot instead of erroring, and every call takes what stood in the
slot before it gives.

VERIFIED: `BG_TakePlayerWeapon` returns at once for a weapon not held
(`Com_BitCheck` at 0x36b92, branch at 0x36b9c) and ends in a `Com_BitClear` of
that bit at 0x36c6e; between, it walks the weapon list for another held weapon
whose `BG_GetInfoForWeapon` record carries the same `+0x78` slot to leave in
its place (0x36bee..0x36c44), zeroing the slot byte when there is none
(0x36c51).

The `"none"` arm matters because stock script feeds `getWeaponSlotWeapon`'s
own `"none"` back in: `sd.gsc`'s `endRound` stores
`pers["weapon2"] = player getWeaponSlotWeapon("primaryb")`, `"none"` for a
player with one primary, and `spawnPlayer` re-gives it after the
`map_restart`. A host that errors on the name kills the spawning thread there
and the player comes back holding nothing.

Not modelled in `crates/server/src/game/builtins/client.rs`: the slot check at
0x43f26 and the replacement search inside the take; no stock loadout reaches
either.

### The spawn's memset and the weapon op queue

VERIFIED (UO `game.mp.uo.i386.so`, `ClientSpawn` 0x54df4): the spawn copies
the session block aside, `memset`s the whole `gclient_t` (0x4734 bytes) and
copies the session back, so `ps.weapons`, `ps.ammo` and `ps.ammoclip` are
empty when `self spawn()` returns, ahead of the script's loadout. On 1.1 the
clip setter stores into the playerstate inside the call
(`client+0x20c+4*clip`, 0x444f4, above): retail's loadout lands on the spawned
playerstate in call order.

vcod's weapon builtins queue a `WeaponOp` for the sim, and the sim's
playerstate is reset in the frame's spawn pass (`Server::game_frame`, after
the script threads). Any pass that drains the queue between the threads and
the spawn pass (a vehicle round's callbacks in `vehicle_fire`, which applies
every slot's ops) or before the frame (a cmd callback's
`apply_callback_ops`) lands the loadout on the old life's playerstate. Trap:
a spawn pass that empties the ammo arrays then wipes it, every weapon at 0
rounds. VERIFIED (measured, ours, UO `tdm` on mp_cassino, `bot_match` seed 2,
10 bots, a temporary log in `vehicle_fire`): 12 vehicle rounds were delivered
with a spawn queued, two at 88 250 and 121 450 ms, the sniper's dry spawns at
ticks 1758 and 2422 (`bot-combat.md` 12.3).

So the `spawn` builtin queues `WeaponOp::TakeAll` in call order as the
memset's emptying, and the spawn pass keeps `ps.ammo` and `ps.ammoclip`; the
loadout survives whichever pass applies it. VERIFIED (measured, ours):
`server::tests::a_loadout_applied_before_the_spawn_pass_survives_it` spawns
and gives a frag inside a damage callback; the frag's clip reads 3 (0 without
the change).

### `setViewmodel` reaches `ps.viewmodelIndex`

VERIFIED: `setViewmodel` (player method 17, 0x4512c) resolves its argument
through `G_ModelIndex` (0x66ed8; relocation at 0x4519b) and stores the result
at `client+0x2170` (0x451a7). INFERRED, off two instructions: that word is
copied into `ps.viewmodelIndex` (+0xbc, netfield offset 188) at
0x40faa/0x40fb4.

The value is a model configstring index, 1-based on base 268
(`clientstate-wire-format.md`'s configstring map). VERIFIED, the fixtures: the
retail playerstate captures carry `viewmodelIndex` 52 on `mp_pavlov` and 82 on
`mp_carentan`, and configstrings 320 and 350 in the matching
`crates/server/tests/fixtures/configstrings/` files are
`xmodel/viewmodel_hands_russian` and `xmodel/viewmodel_hands_us`. VERIFIED,
`pak5.pk3`: the name comes from the character script of the team model chain,
e.g. `character/mp_russian_conscript01.gsc`'s
`self setViewmodel("xmodel/viewmodel_hands_russian")`. The field follows
nationality, not weapon: `m1carbine_mp`'s `handModel` is
`viewmodel_hands_new`, not what the capture carries.

### What `ClientEndFrame` writes for a live client's own view

INFERRED, the compares at 0x40ed1, 0x40f24 and 0x40f45: `ClientEndFrame`
(0x40e98) branches on the `sessionstate` word at `client+0x20d0`: 3 the
intermission arm, 2 tail-calls `SpectatorClientEndFrame` (0x40760), anything
else the live arm, which compares `ps.clientNum` (+0xac, netfield offset 172)
against the entity's own number.

VERIFIED, single stores at named offsets: the equal case (0x40f90), a client
looking out of his own body, sets `pm_flags` (+0xc, netfield offset 12) bit
18 (`orb $0x4,0xe(%edi)` at 0x40fb0, byte 2 of the dword, so 0x40000) and
copies the `setViewmodel` word at `client+0x2170` into `ps.viewmodelIndex`.

VERIFIED, both stores: the other two arms clear that bit and zero
`ps.viewmodelIndex`: the intermission arm at 0x40f11 (`andb $0xfb,0xe(%edx)`),
`SpectatorClientEndFrame` at 0x407b7/0x407ad before anything else.

VERIFIED, `docs/research/cod11-map-cycle.md` 6.2, read store by store: the
intermission arm writes five more fields, `ent.takedamage`, `ent.r.contents`,
`ent.r.svFlags`, `ps.pm_type` and `ps.eFlags`. INFERRED, the same doc's 6.1,
off `0x419ce` being the module's only store of 3 into that word:
`sessionstate` 3 is reached only by a script assignment of `"intermission"`,
which `Scr_SetClientField` maps onto it. Section 6 there also has what arms
the scoreboard drain whose clients the arm's `pm_type` selects.

Bit 18 is the third of the view-source group `cod11-events-and-fx.md` records:
0x10000 following a client (set only on the follow-success path), 0x20000 a
forced follow (script `spectatorclient >= 0`, as against free cycling).
VERIFIED, the stores and the copy width: `SpectatorClientEndFrame` stores both
at 0x408ea and 0x40903 and copies the followed client's playerstate over its
own with a 0x834-dword `rep movsl` at 0x408ba. INFERRED, the compare at
0x408f4: the sign of `client+0x20d4` picks which. INFERRED, the jump at
0x40890: both stores sit past the branch that leaves when no client was found,
so the committed lone-spectator capture in
`crates/common/tests/fixtures/net/snapshots.bin` reads `pm_flags` 0.

### `serverCursorHintString` 255 is a no-hint sentinel

VERIFIED, three stores at named offsets: `G_CheckForCursorHints` (0x4f59c)
stores -1 into `ps.serverCursorHintString` (+0x38c, netfield offset 908) at
0x4f603, after zeroing `serverCursorHint` (+0x384) and `serverCursorHintVal`
(+0x388). VERIFIED, the fixtures: the netfield is 8 bits
(`crates/common/src/net/cod1/fields_v1.rs`), so -1 reaches a client as 255, as
the committed player captures carry.

INFERRED, the branches at 0x4f5d7 and 0x4f5e4: the store runs only when the
entity's `health` (+0x230, field offset 560) is above zero and the byte at
`ent+0x172` is zero. VERIFIED, the relocations: `ClientEndFrame` calls it from
the live arm (`R_386_PC32` at 0x4111a); the other caller is at 0x484c6.

### `serverCursorHint` is an index into a nine-name table

VERIFIED, `.rel.data` (raw dwords are zero without it): `hintStrings`
(0x7e4d4) is 44 bytes, eleven pointer slots. Slot 0 is the empty string; 1 to
9 are `HINT_NONE`, `HINT_ACTIVATE`, `HINT_NOACTIVATE`, `HINT_DOOR`,
`HINT_DOOR_LOCKED`, `HINT_MG42`, `HINT_HEALTH`, `HINT_LADDER`,
`HINT_FRIENDLY`; slot 10 has no relocation.

`setCursorHint` (0x5a0e4) takes one string, walks the table with
`Q_strcasecmp` and stores the matching index at `ent+0xdc`; `HINT_INHERIT`
stores -1; an unmatched name reaches `Scr_Error` with the "not a valid hint
type" line at 0x76e40. VERIFIED: the addresses, immediates and strings.
INFERRED: the control flow (compares at 0x5a12e and 0x5a19d, stores at
0x5a14f and 0x5a160).

`trigger_use` (0x5742c) is a stock map's hint source. VERIFIED, the stores and
the two `G_SpawnString` calls: it stores 2 into `ent+0xdc` at 0x5749f,
`G_SpawnString`s the `cursorhint` key (0x75e6c) and runs the same table walk,
then stores 0xff into `ent+0xd8` at 0x57550 before reading the `hintstring`
key. INFERRED, that ordering: `HINT_ACTIVATE` is the default of a
`trigger_use` with no `cursorhint`, and 0xff is `hintstring`'s unset value.

`ent+0xd8` is a configstring index. VERIFIED, the addresses, immediates and
calls: `G_GetHintStringIndex` (0x5a238) holds a `trap_GetConfigstring` on
`i + 0x4bc`, a `strcmp`, a `trap_SetConfigstring`, three stores through the
out pointer at `ebp+0x8` (0x5a281, 0x5a2a7, 0x5a2b9), the loop bound 0x1f at
0x5a2b1 and return values 1 and 0. INFERRED, which runs when: the scan starts
at 0, a `strcmp` match returns `i`, an empty slot is claimed and returned, all
32 exhausted writes -1 and returns 0; so the scan allocates. The hint-string
range is 32 configstrings at base 1212 with 0xff as its unset marker, the -1
`serverCursorHintString` carries as 255.

VERIFIED: `G_CheckForCursorHints` (0x4f59c) loads `ent+0xdc` and `ent+0xd8`
(0x4f6b8, 0x4f6c6) and stores them into `ps.serverCursorHint` and
`ps.serverCursorHintString` (0x4f83e, 0x4f854), its candidates from
`G_GetActivateEnt` (0x4f14c). VERIFIED, the compare and the allocation: those
loads sit in an arm compared on the classname word (`ent+0x176`) against
`scr_const+0x94`, which `GScr_LoadConsts` fills with
`Scr_AllocString("trigger_use")` at 0x58c58. INFERRED: that gate selects the
arm.

`trigger_lookat` sets no cursor hint. VERIFIED, the single `.rel.text`
relocation against the offset and the call at 0x4f9b2: `scr_const+0x98`
(`Scr_AllocString("trigger_lookat")` at 0x58c8a) is read in exactly one place,
`G_CheckForPreventFriendlyFire` (0x4f88c) at 0x4f98e, which compares it with
the classname of whatever a `trap_LocationalTrace` down the player's aim hit
and then calls `G_Trigger`. INFERRED, control flow: the kind fires off an aim
trace, not contact, and touches none of the three hint fields. VERIFIED, the
whole function: `SP_trigger_lookat` (0x65df0) installs no touch or think, only
`trap_SetBrushModel`, `ent+0x118` = 0x20000000, `ent+0xf4` = 1,
`svFlags |= 2` and `trap_LinkEntity`.

Cursor hints are aim-based, not touch-based. VERIFIED, the function's calls:
`G_CheckForCursorHints` calls `G_GetActivateEnt` (0x4f622),
`G_IsTurretUsable`, `BG_GetInfoForWeapon` and `Com_BitCheck`, and neither
`trap_LocationalTrace` nor `CalcMuzzlePoints`. VERIFIED, `G_GetActivateEnt`
(0x4f14c) calls `CalcMuzzlePoint` (0x4f19f), `trap_EntitiesInBox` (0x4f237)
and `trap_Trace` (0x4f52e). INFERRED: candidates come from a box around the
muzzle filtered by distance and view cone, with a line-of-sight trace per
candidate; scoring and trace are `docs/research/cod11-items.md` section 2.
INFERRED, from absence: no touch path writes the three hint fields
(`SP_trigger_lookat` installs no touch function, and the trigger touch path
stores nothing at `ps+0x384`, `+0x388` or `+0x38c`).

VERIFIED, `crates/server/src/game/script.rs` and
`crates/server/src/spectate.rs`: vcod writes `serverCursorHint` and
`serverCursorHintString` from `ScriptRuntime::cursor_hint_pass`, with item,
turret and `trigger_use` arms (`docs/research/cod11-gametypes-re-bel.md` 4).
The item arm's values are `docs/research/cod11-items.md` 2.3; the turret arm's
hint 6, the weapon's hint-string slot and a mounted player's hint fields are
`docs/research/cod11-turrets.md` 4.3. No capture of a client aimed at a
`trigger_use` exists.

### `legsAnim` needs the animscript state machine

VERIFIED, the instruction operands: `BG_PlayAnim` (0x2c338) writes
`ps.legsAnim` (+0x70, netfield offset 112): it masks the old value to 0x200,
flips that bit, ORs the new index in and stores at 0x2c3bb, with
`ps.legsTimer` (+0x6c) beside it; `torsoAnim`/`torsoTimer` (+0x78/+0x74) get
the same. 0x200 is `ANIM_TOGGLEBIT` (`player-model-anim-system.md`), so the
captures' 634 is index 122 with the bit set.

VERIFIED, the exported symbols only (the path between them is not traced): the
index comes from the parsed animscript through `BG_AnimScriptAnimation`
(0x29db0), `BG_PlayerAnimation` (0x2c1f4) and
`BG_AnimUpdatePlayerStateConditions` (0x2a2ec), off the trees
`BG_AnimParseAnimScript` (0x28b88) builds. vcod ports the state machine
(`crates/common/src/animscript.rs`, `playerpose.rs`); `playerstate_ab.rs`'s
`GAPS` is empty, `legsAnim` included.

## 21. The client entity as vcod builds it

Not a retail reading: vcod's design, resting on sections 2 and 3.

A connecting client gets a `GEntity` at entity number = his client slot
(`ObjectTable::spawn_client`). Section 11 makes that safe: `level.num_entities`
starts at 72 whatever `sv_maxclients` is and `MAX_CLIENTS` is 64, so clients
own 0..63, map entities start at 72, and the allocator never hands out a
number below 72. Freeing a client clears its slot and does not push the number
onto the free list that feeds map entities.

Client-tagged fields live in a second store on the entity, `client:
Option<Vec<Value>>`, `None` for every map entity and HUD element; retail's null
`ent->client` check becomes a type-level fact. The store is indexed by raw
`CLIENT_FIELDS` position, not the first-appearance-of-each-offset dedup
`ENTITY_FIELDS` uses: five rows (`sessionteam`, `sessionstate`, `statusicon`,
`headicon`, `headiconteam`) carry `offset: 0` as placeholders for a fully
custom accessor (section 5), and deduping on them would merge five fields into
one cell.

`.pers` is the one client field the engine fills: `spawn_client` puts a fresh
array handle there, because `ClientConnect` (0x4250f) writes one and every
gametype reads `self.pers["team"]` without creating it (section 5).

## 22. `G_TouchTriggers` is a box query, then a brush contact

VERIFIED: `G_TouchTriggers` (0x3f88c, `game.mp.i386.so`) builds its candidate
box from three floats in `.data` at 0x7dcdc, 0x7dce0 and 0x7dce4: 40, 40 and
52 (`.data` maps at VA 0x7b3a0 from file offset 0x7a3a0, so the raw dword at
the virtual address belongs to another section and reads 0.0).

INFERRED, control flow: the three are subtracted from and added to the
client's origin for the `trap_EntitiesInBox` box, and each returned entity
goes to `trap_EntityContact` with the client's own `r.mins`/`r.maxs`
(+0x100..+0x114). The box query is the broad phase, the contact the exact
test, and a trigger must clear both; neither shape contains the other (the
box reaches 52 below the feet, the clip box 72 above them).

VERIFIED: one `trap_EntityContact` call per candidate. The
`add $0xfffffffc,%esp` and three pushes at 0x3fa10-0x3fa15 precede the `call`
at 0x3fa16, whose operand at 0x3fa17 carries the `R_386_PC32` relocation
against `trap_EntityContact` (`objdump -R`; the raw operand reads
`fc ff ff ff`, so a plain disassembly shows the call pointing at itself).

INFERRED, from the Q3 lineage, not this module: the syscall clips against the
entity's **brush model**, not its `r.mins`/`r.maxs`, as `SV_EntityContact`
does in Q3. The implementation is in `cod_lnxded`, uncited here; only which
shape the test uses rests on this.

That is invisible for a box-shaped brush and large for a shaped one: a brush
cut by an angled plane has a bounding box strictly larger than itself.
VERIFIED, mp_pavlov's lump 4 (a brush's first six sides are its axial bounds;
anything past them an angled plane): of 26 `minefield` brushes, 19 carry extra
non-axial planes (up to 4 on `*25` and `*28`) and 7 are plain boxes. `*28`'s
bounding box is 432.6 x 1663.0 (a fractional extent only an angled cut gives);
`*25` is 696 x 1728 with a diagonal through it, so the brush-free part of its
box is meters wide. A pass testing the bounding box kills a player that far
from the mines, the death reported against vcod on 2026-09-08;
`trigger::box_contacts_hulls` tests the brushes.

The pass walks its candidates in the area tree's order and touches items and
triggers in that one walk, each with a `"touch"` notify on the entity and on
the player ahead of its touch function; a `trigger_damage` sets no touch
function and is never touched (`docs/research/cod11-combat.md` 14.7, "The
touch pass").

### 22.1 The broad phase's contents mask excludes a `trigger_lookat`

VERIFIED: the call at 0x3f925 pushes five arguments, the last the immediate
0x405c0008 at 0x3f918, ahead of 0x400 and the three pointers for the box and
result list. 0x405c0008 has bit 29 (0x20000000) clear. `SP_trigger_lookat`
(0x65df0) stores 0x20000000 into `ent+0x118`, `r.contents`
(`docs/research/cod11-combat.md` 13; `SpectatorClientEndFrame` 0x4078f zeroes
the same word for a spectator).

INFERRED, off the argument being a mask on `r.contents`: `trap_EntitiesInBox`
never returns a `trigger_lookat`, so the touch pass cannot reach one, and the
only reader of the classname is `G_CheckForPreventFriendlyFire`'s aim trace
(section 20, "`trigger_lookat` sets no cursor hint"). Both readings say the
kind fires off an aim trace, not contact.

VERIFIED, `crates/server/src/game/trigger.rs`: `trigger::touched` skips a
`LookAt` row, so vcod notifies no `"trigger"` on contact either. The aim trace
that fires one is 23.1; a live run against ours is 23.7.

### 22.2 `isTouching` is the same contact

VERIFIED, `tools/re/dump_builtins.py`: the `entity_methods` row `istouching`
points at 0x59eb8. In `game.mp.i386.so`:

- VERIFIED: it compares `ent+0xfc` against 0 on receiver and argument
  (0x59ef2, 0x59fa4, 0x59fb1) and calls `trap_EntityContact` at 0x5a09c and
  `trap_EntityContactCapsule` at 0x5a081 (`tools/re/annotate_func.py`, both
  `R_386_PC32`), choosing on `ent+0xf5 & 2` (0x5a06c).
- INFERRED, control flow, `ent+0xfc` read as `r.bmodel` (Q3's
  `entityShared_t`): when the receiver is no brush model, the box is its
  `r.currentOrigin + r.mins/maxs` and the contact is against the argument;
  when it is one, the two swap. Only two brush models give a plain box
  overlap.
- INFERRED, as in section 22: the contact clips against the brushes. So
  `_minefields.gsc`'s `self istouching(trigger)`, the re-check half a second
  after the click, measures the player against the minefield's brushes, as
  the touch pass does.

vcod: `trigger::entity_contact` tests the touch pass's box against the
trigger's brushes. Trap: two boxes overlap where the brushes do not. On
`mp_hurtgen`, whose minefield `*2` is an L of axial brushes, a bot that clipped
the corner at (5888, -64) and walked on along the brush's edge was still inside
`*2`'s box and died up to 200 units from the mines: all four `mp_hurtgen`
deaths over 96 seeds of `bot-navigation.md`'s detector.

The capsule variant, `game.mp.i386.so` and `cod_lnxded` (1.1d):

- VERIFIED: the side tested at 0x5a06c is the boxed one (`ebx`), whose
  `r.currentOrigin + r.mins/maxs` (`+0x134`, `+0x100`, `+0x10c`) make the two
  vectors pushed at 0x5a079-0x5a080; the argument is the other entity.
  `ent+0xf5 & 2` is `r.svFlags & 0x200` (`r.svFlags` at `+0xf4`), the flag
  `ClientConnect` stores on every client (0x4258f, `cod11-player-clip.md`).
  A client's side goes to `trap_EntityContactCapsule`, anything else to
  `trap_EntityContact`.
- VERIFIED: the traps are syscalls 0x35 (`trap_EntityContact`, 0x63aac) and
  0x3b (`trap_EntityContactCapsule`, 0x63acc). In `cod_lnxded`'s switch
  (table at 0x80d4eac) they land at 0x8088188 and 0x80881c0, two copies of one
  body differing only in the last argument pushed (0 at 0x80881ac, 1 at
  0x80881e4). Each takes the entity's clip handle from 0x8091fc4, pushes its
  `+0x134` and `+0x140` (origin, angles), a mask of -1, the two vectors and
  0x80cdfc8 twice as start and end, calls 0x8058978, and returns the byte at
  `ebp-0x1`, the last of the 48-byte trace at `ebp-0x30`: start-solid
  (`cod11-combat.md` 3.2).
- INFERRED, from that argument list (Q3's `SV_EntityContact`, a
  `CM_TransformedBoxTrace` whose start is its end): a position test of the
  box, or with the flag the capsule made of it, against the entity's model.
  The capsule arm of the brush clip is `cod11-player-clip.md` 2.1's and
  `cod11-mantle.md`'s ("The player is a capsule"): an axial side sees the
  capsule as its box, an angled side the nearer sphere pushed out by the
  radius. Against an axial brush the two agree; against a 45-degree side a
  player's box reaches 15 sqrt 2 = 21.2 into it, his capsule 15.
- VERIFIED: 0x8091fc4 returns the model's handle (0x804bac8 on `+0x8c`) when
  `r.bmodel` is set, else `CM_TempBoxModel` (0x804baf8) on the entity's
  `r.mins`, `r.maxs`, `r.contents` and `svFlags & 0x200`. INFERRED: with
  neither side a brush model, a client's `istouching` of another client is
  capsule against capsule.

vcod tests a client's side as a capsule against a trigger's brushes
(`trigger::capsule_contacts_hulls`) and a client against a client as capsule
against capsule (`trigger::capsules_overlap`). Two other non-brush sides stay
a box overlap, which matches retail for a capsule against a box (all axial
sides). VERIFIED, a grep of the stock paks' `maps/MP` scripts: their
`istouching` calls are a player against a trigger (`sd.gsc`, `re.gsc`,
`_minefields.gsc`), never player against player.

The capsule pair, `cod_lnxded` (1.1d):

- VERIFIED: `CM_TempBoxModel` (0x804baf8) stores the bounds and contents and
  answers handle 0x1fe for a capsule, 0x1ff for a box. VERIFIED, decompile:
  the trace's position test (0x8056310) sends 0x1fe, when the stored contents
  meet the mask, to 0x805475c for a capsule trace and 0x8054a38 for a box. A
  side whose `r.contents` is 0 (a dead or spectating client) is never touched.
- VERIFIED, decompile of 0x805475c: the model's capsule is its box's centre,
  radius `min(half x, half z)`, sphere offset `half z - radius`. A hit is any
  of the four sphere-centre pairs nearer than the two radii squared (strict),
  or the centres' z gap within the sum of the two offsets (both ends
  inclusive) with the xy gap squared under the radii squared. Unlike Q3's
  `CM_TestCapsuleInCapsule`, whose cylinder test compares the top against
  `p1 <= z <= p2` and never passes, this tests the centres' gap.
- INFERRED: the trace's own capsule is built from the receiver's box as Q3's
  `CM_Trace` does (0x8056310's `local_f4` / `local_1c` setup has that shape).

### 22.3 A client's `r.mins`/`r.maxs` follow the stance

The box the touch pass and `isTouching` read on a client's side is the one his
last cmd's pmove left, not the standing 70.

VERIFIED, `tools/re/annotate_func.py`: `ClientThink_real` copies the
`pmove_t`'s box into the entity ahead of `ClientEvents`, `trap_LinkEntity` and
`G_TouchTriggers`:

| Build | `pmove_t` | `Pmove` | Copy from | Into | At | Then |
|---|---|---|---|---|---|---|
| 1.1d | `ebp-0xf8` | 0x40466 | `ebp-0x38..ebp-0x24` | `ent+0x100..+0x114` | 0x40539-0x40569 | `ClientEvents` 0x40589, `trap_LinkEntity` 0x40595, `G_TouchTriggers` 0x405b3 |
| 1.5b | `ebp-0x118` | 0x3993d | `ebp-0x58..ebp-0x44` | `ent+0x104..+0x118` | 0x39a3f-0x39a7e | `trap_LinkEntity` 0x39ab9, `G_TouchTriggers` 0x39ad6 |
| UO 1.51b | `ebp-0x138` | 0x41a80 | `ebp-0x60..ebp-0x4c` | `ent+0x108..+0x11c` | 0x41b99-0x41bd8 | `trap_LinkEntity` 0x41c13, `G_TouchTriggers` 0x41c30 |

INFERRED, the Q3 lineage and the offset (`pmove_t + 0xc0` on 1.1d and 1.5b,
`+ 0xd8` on UO, right after the touch list): the copied vectors are `pm.mins`
and `pm.maxs`, which `PM_CheckDuck` sizes to the stance
(`cod11-player-clip.md`; 70, 50 or 30 high).

- VERIFIED, 1.1d: `ClientSpawn` writes `playerMins`/`playerMaxs` (standing)
  into `ent+0x100..+0x114` at 0x42775-0x427b7 before its own think. VERIFIED:
  1.5b writes the same six words through GOT slots at 0x3cae3-0x3cb3c, UO at
  0x44f5f-0x44fb8. INFERRED: those slots are `playerMins`/`playerMaxs`.
- VERIFIED: `SpectatorThink` writes nothing to `ent+0x100..+0x11c` on any of
  the three, so a spectator keeps `ClientSpawn`'s standing box.
- VERIFIED: `player_die` stores 30 into `r.maxs[2]` between its unlink and
  link: 1.1d `.rodata` 0x743f0 (30.0) into `ent+0x114` at 0x49d6d-0x49d73
  (`trap_UnlinkEntity` 0x49d68, `trap_LinkEntity` 0x49d7d); 1.5b the immediate
  0x41f00000 into `ent+0x118` at 0x470d5 (calls 0x470cd, 0x470e5); UO into
  `ent+0x11c` at 0x4fd0b (0x4fd03, 0x4fd1b). A dying player is linked 30 high
  whatever his stance until his next cmd's think copies `pm.maxs`.
- VERIFIED: `PM_CheckDuck`'s `pm_type > 5` arm sets `pm.maxs[2]` from the
  playerstate's own maxs, not the stance: 1.1d `ps+0x338` at 0x317f6-0x317fc,
  1.5b `ps+0x338` at 0x2352c-0x23532, UO `ps+0x570` at 0x2807e-0x28084, after
  the arm's shared copy of `ps` mins and maxs x, y (1.1d 0x317a7-0x317dd).
  INFERRED: that word is `ClientSpawn`'s standing 70; the live rows below read
  70.
- VERIFIED live (2026-10-21, `client-probes/probe_istouching` on mp_carentan,
  one `--probe-uo-weapon direct` client on probe_legs' floor; 1.1d, 1.5b and
  UO 1.51b logged the same lines, committed in
  `crates/server/tests/fixtures/istouching/`): the map's trigger_hurt brush,
  moved so its bottom face sits `v` over the player's feet, reads
  `isTouching` both ways for `v` up to the box height inclusive: 70 standing,
  50 crouched, 30 prone, 30 in the frame `suicide` ran (crouched), 70 from the
  next frame on and after the dead body's keys let go. Its top face touches
  from 0 at the feet up, its -x face out to 15 inclusive, in every stance and
  dead. A dead client still touches: 22.2's contents check bites only on a
  temp box model, and the client is the trace's box here.
- VERIFIED live, same day, UO only: a client crouched and then holding prone
  stayed crouched for 2 s (`pm_flags` 0x48002 decoded, no prone bit), so a UO
  prone row starts from standing.

VERIFIED, `crates/server/src/game/trigger.rs`: vcod holds the height per
client (`GameHost::client_height`, mirrored off the sim after each cmd and set
by the `spawn` builtin's think; `ClientSim::box_height`:
`PlayerState::box_height` alive, 70 once a cmd ran dead, `player_die`'s 30
until then, which `GameHost::die` also links with) and builds
`GameHost::client_box` from it; `trigger::touched` and
`trigger::entity_contact` read that box. `istouching_ab` replays the probe on
all three and matches retail line for line.

## 23. The aim trace, linking, objectives and the HUD tweens

The trace that fires a `trigger_lookat`, the link that pins a planting player
to a bomb, the objective table behind the compass icons, and the three hudelem
tweens. `game.mp.i386.so`, addresses module-relative.

### 23.1 The aim trace and `isLookingAt`

VERIFIED: `G_RunFrame` (0x50478) calls `Scr_RunCurrentThreads` at 0x50653 and
`ClientEndFrame` per in-use client at 0x50abd. INFERRED, off that order: an
`isLookingAt` read in script frame N answers off the trace `ClientEndFrame` ran
at the end of frame N-1.

VERIFIED: `ClientEndFrame` (0x40e98) compares `client+0x20ec` against 2
(0x40ebe) and sessionstate (`client+0x20d0`) against 3 (0x40ed1) and 2
(0x40f24), calls `SpectatorClientEndFrame` at 0x40f30, and calls
`G_CheckForPreventFriendlyFire` (0x4110d) and `G_CheckForCursorHints`
(0x41119). INFERRED, off the `jne` at 0x40ec5 and the jumps to the epilogue at
0x40f1f and 0x40f35: it returns at once unless `client+0x20ec` reads 2;
sessionstate 3 takes the intermission arm, 2 the spectator one; only the
fall-through reaches the two calls.

`G_CheckForPreventFriendlyFire` (0x4f88c):

| Step | Label | Address |
|---|---|---|
| store 0 to `client+0x2260` | VERIFIED | 0x4f8a1 |
| compare byte `ent+0x172` against 0; `jne` to the epilogue 0x4f9b7 (INFERRED: non-zero returns having done nothing) | VERIFIED | 0x4f8ab, 0x4f8b2 |
| `CalcMuzzlePoints(ent, &out)` | VERIFIED | 0x4f8c0 |
| `CalcMuzzlePoints` (0x693f4) reads the aim angles at `client+0x220c`, `+0x2210` and the view height at `client+0xc8` | VERIFIED | 0x69424, 0x6942d, 0x6941b |
| it calls `G_AddLean`, then rewrites each muzzle component through `fistp`/`fild` under control word `| 0xc00` (truncate toward zero) | VERIFIED | 0x6947d; 0x6948c, 0x694b4, 0x694dc |
| end = muzzle + forward * 8192.0 (`.rodata` 0x7547c) | VERIFIED | 0x4f8fe..0x4f931 |
| `trap_LocationalTrace` with mask 0x20000001, then 0x22802001 | VERIFIED | 0x4f949, 0x4f96e |
| after each, entity-number word against 0x3fd; both `ja`s to the epilogue | VERIFIED | 0x4f951, 0x4f976; 0x4f957, 0x4f97c |
| hit entity's classname word (`g_entities[n]+0x176`, stride 0x314) against `scr_const+0x98`; `jne` to the epilogue | VERIFIED | 0x4f995, 0x4f99c |
| store the entity pointer to `client+0x2260`, `G_Trigger(trigger, ent)` | VERIFIED | 0x4f9aa, 0x4f9b2 |

INFERRED, off the branches: the lean is added first, so the trace starts at the
leaned eye truncated on every axis; a hit number above 0x3fd (world or no hit)
ends the function; the last two steps run only on a classname match.

VERIFIED: the sixth argument to both traces is a priority-map pointer. The two
`R_386_32` relocations at 0x4f8ea and 0x4f8fa sit in the immediates of the
stores at 0x4f8e7 (`riflePriorityMap`) and 0x4f8f7 (`bulletPriorityMap`), both
into the slot the traces push. VERIFIED: the function tests the held weapon at
`client+0xb0` (0x4f8d7) and compares `BG_GetInfoForWeapon`'s record `+0x2c0`
against 0 (0x4f8ee). INFERRED: `riflePriorityMap` when the weapon and its
`+0x2c0` are non-zero, `bulletPriorityMap` otherwise.

VERIFIED: `scr_const+0x98` is `Scr_AllocString("trigger_lookat")` (string at
0x7620c), filled by `GScr_LoadConsts` (0x58550).

VERIFIED: both traces push the same `start` (`lea esi,[ebp-0x58]`, 0x4f93d)
and `end` (`lea edi,[ebp-0x3c]`, 0x4f92b), and `*ent` (`s.number`, the dword
at `ent+0`) as pass entity (0x4f943, 0x4f968). The classname compare at
0x4f995 reads the entity number the second trace wrote to `[ebp-0x8]`
(0x4f97e).

VERIFIED, the bits: 0x22802001 is 0x20000000 (`trigger_lookat` contents,
22.1), 0x02000000 (`CONTENTS_BODY`, a playing client's end frame,
`docs/research/cod11-player-clip.md` 1.2), 0x00800000, 0x2000 and
`CONTENTS_SOLID`; no `CONTENTS_CORPSE` (0x04000000), 0x10 or 0x20.
0x20000001 is a subset.

INFERRED, off those: the first trace decides whether anything solid or a
lookat is reached before the world, the second whether a body stands in front.
The aimer's own body stops neither. A live, playing body elsewhere on the
segment ends the second trace on a client whose classname is not
`trigger_lookat`, so nothing fires and `client+0x2260` stays 0; a corpse, dead
player or spectator (contents 0 or CORPSE) does not. The first trace changes
the answer only by ending the function early. The second is locational, so a
body stops it where the posed bones do, ranked through the priority map a
bullet from the held weapon takes (`docs/research/cod11-combat.md` 3.1), not
across the link box.

VERIFIED: `G_GetActivateEnt`'s visibility trace (cursor hint and use key pick)
is a `trap_Trace` with mask 0x11 (`docs/research/cod11-items.md` 2.1), no
BODY bit. INFERRED: a body in the line gates neither of them, only the lookat.

VERIFIED, `G_Trigger` (0x656f0): tests `Scr_IsSystemActive(1)` (0x6570b),
compares the counter at `level+0x29ec` against 0x100 (0x65714), calls
`Scr_AddEntity(toucher)` and `Scr_Notify(trigger, scr_const+0x92, 1)`
(0x6571f, 0x65732), and at 0x65740..0x65772 appends a 12-byte record at
`level+0x1dec + n*12` (the two entity numbers and the two `+0x300` script
handles) and increments the counter. `scr_const+0x92` is `"trigger"`
(0x761e9). INFERRED, off the `je` at 0x6570d and `jne` at 0x65719: a 0 answer
returns; the notify runs when the counter reads exactly 0x100 (the overflow
path); the append otherwise (the normal path).

VERIFIED: `G_RunFrame` walks that queue at 0x50578..0x5064d, calling
`Scr_Notify` with `"trigger"` at 0x505f6, calls `Scr_RunCurrentThreads` at
0x50653 and zeroes the counter at 0x50665. INFERRED, off the handle compares
and the `jne` at 0x5065f back to 0x50547: the notify is raised per record whose
two `+0x300` handles still match their entities, and walk and thread run repeat
until a pass adds no record. INFERRED, no time or count gate on either path: a
`trigger_lookat` notifies once every server frame the aimer keeps looking.

VERIFIED: `Touch_Multi` (0x65a18) and `hurt_touch` (0x64dc4) call `Scr_Notify`
themselves (0x65a5b, 0x64e25), not `G_Trigger`. VERIFIED: `G_RunFrame` writes
`level.time` (`level+0x1e8`) from its argument at 0x50499; the drain and its
`Scr_RunCurrentThreads` sit at 0x50578..0x50653. INFERRED, no jump from the
drain back past 0x50499: the write precedes the drain. VERIFIED, the plant and
defuse fixtures: the progress bar's `scaleStartTime` (`scaleOverTime` takes it
from `level.time`, 23.4) equals the `serverTime` of the first snapshot
carrying it on all three holds (83800, 86800, 114600), though the use cmd that
started each travelled between frames (`st` 83766 and 86766 on the plant).
INFERRED: a thread a touch wakes during `ClientThink` runs no earlier than the
next `G_RunFrame`, on its `level.time`, whether through `G_Trigger`'s queue or
straight `Scr_Notify`.

VERIFIED: `G_RunFrame` calls `Scr_SetTime` with `level+0x1e8` at 0x5070c.
`Scr_SetTime` (0x6c494) is a stub calling through the `.bss` slot at 0xc113c,
an engine-supplied function. The `gettime` builtin (0x5d088) answers
`level+0x1e8` through `Scr_AddInt`, not the engine's clock. INFERRED, off
0x5070c sitting past the drain's `Scr_RunCurrentThreads` (0x50653) and the
plant completing at the bar's `scaleStartTime` + 5000 on retail's capture and
ours (100 `wait 0.05`s from the frame the trigger woke `bomb_think`): a thread
the drain wakes on frame T reads T from `getTime()` though the script clock
gets T later in the frame, and its first `wait 0.05` resumes on T + 50, so the
late hand-off moves no `wait` a frame early.

VERIFIED, `isLookingAt` (0x4576c): compares the receiver number against 0x3ff
(0x45776) and the client pointer against 0 (0x4578a), calls `Scr_Error` at
0x457a5 and 0x457c2, takes its argument through `Scr_GetEntity(0)` (0x457d4)
and hands `Scr_AddInt` the compare of `client+0x2260` with the argument
(0x457e4). INFERRED, off the `ja` at 0x4577c and `jne` at 0x45791: an
out-of-range number or missing client is the error. INFERRED, that being the
whole function: it runs no trace and no cone; it reads the last
`G_CheckForPreventFriendlyFire` result.

Three retail captures on mp_carentan, one run, settle the rest. Server half
`crates/server/tests/fixtures/triggers/mp_carentan-sd-lookat.txt`; client
halves `crates/server/tests/fixtures/playerstate/mp_carentan-sd-plant-attacker.txt`
and `-sd-defuse-defender.txt`. The probe's `getTime()` and the clients'
`serverTime` are one clock, so a fire pairs with a snapshot by equality.

VERIFIED, the lookat fixture:

- mp_carentan spawns exactly one `trigger_lookat`, entity 170, the same after
  each of the run's four `InitGame:` boundaries.
- While the aim reaches the trigger the notify comes every 50 ms (20/s at the
  tick): 444 fires in four contiguous runs of 30, 61, 152 and 201; all 440 gaps
  inside read exactly 50; the only larger gaps (3050, 3050, 4550 ms) are the
  sweep aimed off the trigger.
- 444 `PROBE fire` lines against 443 `PROBE looking`; every fire but one has a
  `looking` at the same `getTime()`. The exception is the last, 124600, the
  frame the defuse completed.

VERIFIED, the defender fixture's `[phase sweep]` paired with the lookat
fixture, from a standing eye 20.4 units horizontally from the charge and 60.1
above it, 15 stations:

| Offsets | Fired |
|---|---|
| aim, `pitch -8`, `pitch +8`, `yaw -15`, `yaw -8`, `yaw +8`, `yaw +15`, `yaw +30` | every frame from within 100 ms of entering to the end of the 1.5 s window (28 of the aim station's 30, 29 of `pitch -8`'s 31, 29 of `yaw -15`'s 30, every other one full) |
| `pitch -45`, `pitch -30`, `pitch -15`, `pitch +15`, `pitch +30`, `yaw -30` | never |

VERIFIED, the defender fixture: `pitch +30` and `pitch +45` both settle at
`viewangles[0]` 87.9, the engine's pitch clamp at 16000 short units, 16.7
degrees below the 71.2 base aim: one data point, offset +16.7.

VERIFIED, same pairing: `pitch -30`, `pitch +15` and `pitch +45` each carry
exactly two fires inside their first 100 ms and none after. INFERRED: those
are the previous station's fires, arriving while the new view walks to the
server.

VERIFIED, same pairing: the defuse icon on the wire tracks the fires frame for
frame. The 64x64 shader element enters the defender's HUD array in the
snapshot whose `serverTime` equals the first fire's `getTime()` at all four
leading edges. Three trailing edges lag one snapshot (fires end 93400, 99450,
111550; icon gone at 93450, 99500, 111600). The fourth does not: last fire and
the icon's last snapshot are both 124600. INFERRED, off that being the defuse
frame and `sd.gsc` destroying the icon there: the completion takes the element
off the wire on the fire's frame; a sweep that merely looks away leaves it a
snapshot.

The station origin is `-190.9, 2459.0, -21.9` (the sweep's own traces'
`origin`), the charge `-176.0, 2473.0, -22.0` (the objectives column's slot
0). The fixture's `# station` line carries the same x and y with z `-20.9`,
where the probe stood before the sweep's first snapshot. VERIFIED, off those
and a 60-unit view height: aim yaw 43.2 and pitch 71.2 down (the fixture's
settled `viewangles` at the aim station), range 63.5. A 16-unit trigger there
subtends 7.2 degrees half-width to a face and 10.1 to a corner.

INFERRED, off that half-width against the stations that fired: pitch offsets
behave as an angular test (8 in, 15 out); yaw offsets do not. At 71 degrees
down the ray crosses the trigger's 16x16 footprint, so a yaw offset slides the
crossing sideways by about twice the horizontal reach times the half-angle's
sine, about 10.6 units at 30 degrees, inside a box whose corner reach is 11.3.
INFERRED, a slab test of the ray against the bounds (`-8..8` in x and y,
`0..16` in z off the charge origin): the yaw window runs -26.2 to +27.1 off
the aim and the pitch window -16.7 to +9.8, so `yaw +30` sits 3.0 outside and
`yaw -30` 3.8. That gives the direction of the asymmetry, not its size: the
slab test disagrees on `yaw +30` (fired, ray 0.99 clear of the box) and
`pitch -15` (quiet, ray through the far top corner), both within about a unit
of the boundary.

Measured on vcod, `crates/server/tests/sd_plant_ab.rs` against the three
fixtures: with the aim trace starting at the eye truncated as
`CalcMuzzlePoints` does, and the charge where `getPlant` puts it (23.6), all 15
stations fire or stay quiet as retail's, `pitch -15` and `yaw +30` included.
INFERRED: the slab test's unit was that truncation, which moves the eye from
`-190.9, 2459.0, 38.1` to `-190, 2459, 38`.

VERIFIED, an earlier run not kept in the repo: the planter stood 88 units out
along the defender-to-bomb line, the whole sweep logged zero fires, and fires
began once that client dropped. INFERRED, off that and the second trace's mask
and classname compare: a live body between eye and trigger stops the fire;
this capture's probe teleports the planter away for that reason.

vcod: `trigger::aim_trace` takes the world with the second mask's world bits
(0x802001) and the static models, the lookat the segment enters first, then
every other live, playing client's body (`GameHost::client_bodies`, refreshed
from the sims just before the pass, so a `setOrigin` or re-anchor that frame
moves it) through the `trace_bodies` a bullet and a blast use, with the aimer
as pass entity and the held weapon's priority map. A body whose bones the
segment scores short of the lookat's entry answers no lookat. Not modelled:
script models, which the second mask's 0x2000 meets through their 0x2080
contents (`docs/research/cod11-combat.md` 2.7). VERIFIED, stock `sd.gsc`: the
planted charge is one, and the plant moves `bombtrigger` to its origin; whether
its mesh reaches past the trigger's box toward an aimer is not measured. No
retail run with a body stepping in and out of the line exists beyond the
earlier run above.

### 23.2 `linkTo`, `unlink`, `enableLinkTo`

VERIFIED, `linkTo` (0x59cc4): resolves the receiver to `g_entities[n]`
(0x59cd7), type-checks argument 0 as an entity (0x59d0b, 0x59d1d), tests bit
0x20 of the **receiver**'s byte `ent+0x17d` (0x59d37), carries the error
`"entity (classname: '%s') does not currently support linkTo"` (0x76b80),
passes the empty string at 0x76629 as tag (0x59d97), calls
`G_EntLinkTo(child, parent, tag)` (0x59daf), and reads two vectors and calls
`G_EntLinkToWithOffset` (0x59ded). INFERRED, off the branches: the error fires
when the bit is clear, the empty tag is a call with no further argument, two
arguments reach `G_EntLinkTo` and four `G_EntLinkToWithOffset`.

VERIFIED, a byte scan of every `0x17c`/`0x17d` store: five places write
`ent+0x17d` bit 0x20: `G_SpawnItem` (0x4e7b9), `G_SpawnTurret` (0x52f65),
`InitScriptMover` (0x6038d), `enableLinkTo` (0x5d6a0), and `ClientSpawn`,
which writes the dword `ent+0x17c = 0x2000` at 0x4276b (high byte 0x20). The
last is why `sd.gsc`'s `other linkTo(self)` works on a player without
`enableLinkTo`. VERIFIED: `maps/MP/gametypes/sd.gsc` in `pak5.pk3` calls
`linkTo` at its two plant sites and `enableLinkTo` nowhere; `re.gsc` is the
only other stock MP script using either.

VERIFIED, `enableLinkTo` (0x5d5d0): tests the bit (0x5d602), carries
`"entity already has linkTo enabled"` (0x76ce0); tests `ent+0x4` and byte
`ent+0x161` against 0 (0x5d61b, 0x5d621), reads `ent+0x1fc` and `ent+0x200`,
compares the classname against `trigger_multiple` (0x7647e, `strcasecmp` at
0x5d658), carries `"entity (classname: '%s') does not currently support
enableLinkTo"` (0x76d20); writes `ent+0x1fc = level.time`,
`ent+0x200 = Think_GeneralLink` and sets the bit (0x5d68b..0x5d6a0).
INFERRED, off the branches: the first error when the bit is already set; the
second unless `ent+0x4` and `ent+0x161` both read 0 and, when `ent+0x1fc` or
`ent+0x200` is non-zero, the classname is `trigger_multiple`; the stores run
only after every check.

VERIFIED, `G_RunClient` (0x40660): tests `client+0x21d8` against 0 (0x406a3)
and the link record `ent+0x2e4` against null (0x406b0); compares sessionstate
`client+0x20d0` against 1 and stores 1 or 7 to `ps.pm_type` (`client+0x4`)
(0x406c2..0x406d0); calls `G_SetFixedLink(ent, 2)` (0x406d9),
`G_SetOrigin(ent, ent+0x134)` (0x406e9), `G_SetAngle(ent, ent+0x140)`
(0x406fc); writes 1 to `ent+0xc` and `ent+0x30`; calls `trap_LinkEntity`
(0x40713); copies `ent+0x134..0x13c` into `ps.origin` (`client+0x14..0x1c`,
0x40718..0x40742). INFERRED, off 0x406a3 and 0x406b0: that block runs once
`client+0x21d8` reads 0 and the record is non-null, writing `pm_type` 7 when
sessionstate reads 1, else 1. VERIFIED: 0x40747..0x40755 decrement a
`pm_type` of 1 or 7 back to 0 or 6. INFERRED: the decrement runs when there is
no record.

VERIFIED: `G_SetFixedLink` (0x66540)'s mode-2 arm (0x666d0) runs
`MatrixTransformVector` over the offset at `record+0x34` with the parent's
axis into `ent+0x134..0x13c`, writing no angles; the mode-0 arm (0x66630)
writes both.

UNVERIFIED: CoDExtended's `shared.h:698` names 1 `PM_NORMAL_LINKED` and 7
`PM_DEAD_LINKED` (community names); the values and the decrement are read out
of the module.

VERIFIED, a byte scan of both encodings over `.text`: `ps.pm_type` is compared
against 1 at exactly seven sites: `PM_UpdateLean` (0x32c63),
`PM_AdjustAimSpreadScale` (0x38622, 0x38736), `G_TouchTriggers` (0x3f8a9),
`ClientEvents` (0x3fec9), `ClientEndFrame` (0x4124d),
`G_GetNonPVSFriendlyInfo` (0x42cad). None is in `PmoveSingle`, which reaches
`pm_type` 1 through a jump table (below).

The link record, re-anchor and failure messages on non-client entities are
`docs/research/cod11-movers.md` 15; `enableLinkTo`'s think, a linked turret and
a tag parent's model change are its 16.

VERIFIED, `unlink` (0x5d594) calls `G_EntUnlink` (0x680d4), which calls
`G_SetOrigin` and `G_SetAngle` with the entity's own origin and angles
(0x680f9, 0x68109), unhooks it from `parent+0x2e8` through `record+0x4`
(0x68118..0x6814c), clears `ent+0x2e4`, releases the tag string and frees the
0x70-byte record (0x68156..0x68167). INFERRED, the branch ahead: all of it
runs only when a record exists. VERIFIED (`objdump -R`): `ClientSpawn` calls
`G_EntUnlink` on the spawning client (0x426f1); those two are the only callers
of the unlink half, and `linkTo` the only caller of either link half.

`PmoveSingle` compares `pm_type` against no immediate 1 but dispatches on it.
VERIFIED: 0x341b2..0x341c1 load `ps.pm_type`, decrement it, compare against 6
with a `ja` to 0x34274, and jump through `[ecx*4 + 0x70ce8]`; the seven dwords
at `.rodata` 0x70ce8 read 0x34220, 0x341e6, 0x34200, 0x341c8, 0x34456,
0x34274, 0x34220. INFERRED, off that indexing: `pm_type` 1 and 7 share the arm
at 0x34220, and `pm_type` 0 takes the `ja` to 0x34274, the default branch the
mantle doc's "Frame flow of the normal-move path" walks.

VERIFIED, the 0x34220 arm: stores 0x3ff into `ps+0x54` (`groundEntityNum`,
0x34222); zeroes the `pml` words at +0x30 and +0x2c (0x34229, 0x34233; the
mantle doc reads +0x2c as `pml.walking`); calls `PM_UpdateAimDownSightFlag`
(0x3423d), `PM_UpdatePlayerWalkingFlag` (0x34242), fn 0x316f4 (0x34247), fn
0x32a44 (0x3424c), `PM_Weapon` (0x34251); tests `ps+0x81 & 0xc0` (0x3425d)
with a `je` to the epilogue 0x34456 (0x34264); calls fn 0x322c8,
`PM_Footsteps` (0x3426a); ends in a `jmp` to 0x34456 (0x3426f). The
`trap_SnapVector(ps.velocity)` call at 0x34451 sits just before 0x34456.
VERIFIED: the arm calls none of fn 0x30778, the ground trace fn 0x30474,
`PM_LadderMove` (0x33944), `PM_WalkMove` (0x2f258) or `PM_AirMove`
(0x2f03c), which the default arm dispatches between. INFERRED: `PM_Footsteps`
runs only when that bit test is non-zero; a linked client's cmds run no walk,
air or ladder move and no ground trace, and its velocity is neither integrated
nor snapped.

What the arm's calls reach (`tools/re/annotate_func.py`):

- VERIFIED: fn 0x32a44 counts `ps+0x10` down by `pml.msec` and at zero clears
  `pm_flags` 0x100, 0x200 and 0x2000 (0x32a60), and counts `ps+0x6c` and
  `ps+0x74` down to 0 (0x32a7d-0x32abc): Q3's `PM_DropTimers`.
- VERIFIED: fn 0x322c8 (`PM_Footsteps`,
  `docs/research/player-model-anim-system.md`) selects the legs anim and calls
  `PM_FootstepEvent` at 0x32482 and 0x328bc; `PM_FootstepEvent`'s other caller
  is `PM_StepSlideMove` (relocation at 0x3585d), which also raises
  `EV_STEP_VIEW` (mantle doc, "The step event and the velocity scale"). The
  arm reaches 0x322c8 only past the `ps+0x81 & 0xc0` test, eFlags 0x4000 and
  0x8000, the bits fn 0x316f4 turns into the prone and duck `pm_flags`
  (0x31849-0x31881). INFERRED: those are a mounted turret's bits, so a linked
  planter raises no footstep, step event or legs selection.
- VERIFIED: the one velocity store in fn 0x316f4 (0x31d02, `velocity[2]`) sits
  behind a `groundEntityNum != 0x3ff` test (0x31cdc), and the arm wrote 0x3ff
  at 0x34222 first. INFERRED: a link takes no jump velocity; stance
  transitions and the eye lerp still run.
- VERIFIED: `PM_UpdateAimDownSightFlag` keeps the ADS flag when
  `pml.groundPlane` (pml+0x30) is set or `pm_type` reads 1 (0x37275-0x3727e,
  a register compare); `PM_AdjustAimSpreadScale` takes its airborne arms only
  when `groundEntityNum` is 0x3ff and `pm_type` is not 1 (0x38619, 0x38622;
  0x3872d, 0x38736); `PM_UpdateLean` takes lean input when `groundEntityNum`
  is not 0x3ff or `pm_type` is 1 (0x32af2-0x32afe) and skips its wall-clamp
  trace at `pm_type` 1 (0x32c63). INFERRED: under a link the sight, spread and
  lean behave as on the ground, and the lean is not clamped to walls.

The seven-site scan above looked for memory-operand encodings only; 0x3727e
compares a register loaded from `ps.pm_type` at 0x3723f, and no scan for that
shape was run.

VERIFIED, the plant fixture's `[phase hold1]`: the release frame 85800 reads
`pm_type` 0 with `groundEntityNum` 1023 and the linked velocity; 85850 reads
177 and the first decayed velocity. INFERRED: a frame's cmds see the `pm_type`
the previous frame's `G_RunClient` left, so the unlinking frame's run the
linked arm and the ground reappears on the next.

vcod: `pmove::pmove` takes the linked arm on `PlayerState::linked`, which
`ClientSim::step` sets from the link record before each cmd; the arm clears the
ground and runs the view, sight flags, stance, timers and weapon, and
`update_anims` holds the legs, the strafe condition and the ground edges while
it is set.

VERIFIED, the plant fixture's `[phase hold2]`: `pm_type` reads 1 on all 100
snapshots from the `linkTo` at 86800 to 91750 and 0 on the snapshot before. It
reads 0 at 91800 too, but that frame is the probe's own `unlink()` and
`setOrigin` (`PROBE teleport_planter 1 (-512.00, 2688.00, -16.00)`; the 91800
trace's origin is `-512.0, 2688.0, -15.0`). Only the defuse fixture measures
retail's own unlink latency.

VERIFIED, same phase: a walk input moves a linked client not at all.
`forward=127` on 92 consecutive cmds (`st` 88750 to 90250); across the 31
snapshots spanning them the origin holds at `-192.8, 2457.1, -21.9` to the
tenth and `velocity` reads `0.0, 0.0, 0.0`. INFERRED, off the 0x34220 arm: the
mover ignores the input under `pm_type` 1, and `G_RunClient`'s re-anchor holds
the origin to the parent besides; the capture alone cannot separate the two.

VERIFIED, `[phase hold1]`, the abort: the attacker links while moving; his
`velocity` reads `184.0, 27.0, 0.0` on every linked snapshot from 83800 to
85750 and on the release frame 85800, origin held at `-214.9, 2453.8, -21.9`;
the next snapshot reads `138.0, 20.0, 0.0` and the origin has moved 7.6.
INFERRED, off that, `[phase hold2]`'s zero and the arm (no integration, no
snap): the link freezes the velocity at its link-frame value rather than
zeroing it, the unlinking frame's cmds still run the linked arm, and the mover
resumes from the frozen velocity the frame after. VERIFIED, vcod measurement
(23.7): a model that zeroes it passes the gate's placed clients, who link from
a standstill.

VERIFIED, both fixtures: `groundEntityNum` reads 0x3ff on every linked snapshot
but the first, which keeps the last free frame's ground (177 on plant and
defuse). INFERRED, off the arm's store at 0x34222: the 0x3ff is the linked
arm's write. The first linked snapshot's 177 fits that frame's cmds having run
at `pm_type` 0, before `G_RunClient` wrote the 1; the order inside a frame is
not read.

Measured on vcod's own load of mp_carentan under `sd`, vcod's numbering:
entity 177 is the `script_brushmodel` `*5`; a player box traced down at the
plant spot `-192.8, 2457.1` rests at z `-21.875` on its brush 4281
(`textures/common/clip_metal`, contents 0x280306c0, which meets neither shot
mask, combat doc 2.7); the terrain under it is at `-31.875`. INFERRED, off the
same load numbering the lookat trigger 170, as retail's census reads (23.1),
so spawning in retail's order: retail's 177 is that brush model, and a client
standing on a submodel's brushes reads the submodel's entity as ground.

VERIFIED, the ground trace fn 0x30474: on a walkable hit it calls the
crash-land fn 0x2fd68 when `groundEntityNum` reads 0x3ff (0x306fc, 0x30721),
stores the trace's own entity number (`ebp-0x20`) into `ps+0x54`
(0x3072c-0x30732), and hands any number but 0x3fe to the touch list
(0x3073b-0x30768). vcod: `CollisionWorld::entity_num` reports the entity the
server named for a submodel at spawn (`CollisionWorld::set_model_entity`,
from `spawn_entities_from_string`) and the world's for everything else, and
`pmove`'s ground trace keeps it.

VERIFIED, the fixtures: `pm_flags` holds 262144 across link and unlink, and
`eFlags` moves only at the probe's `setOrigin` teleports (attacker 68750 and
91800; defender 68750, 91800, 130650).

VERIFIED, the plant fixture: link and release land on the snapshot after the
cmd that caused them. Use first travels at `st` 83766 and 83800 reads
`pm_type` 1 with the progress bar on the wire; the release travels at `st`
85766 and 85800 reads `pm_type` 0, bar gone.

VERIFIED, the defuse fixture: `pm_type` reads 1 through the defuse and two
snapshots past the bar's last frame (124600, 124650), 0 at 124700: retail's own
unlink, 100 ms, no probe teleport near.

VERIFIED, `maps/MP/gametypes/sd.gsc` in `pak5.pk3`: the plant's success branch
destroys the three HUD elements, `delete()`s both bombzones and returns
without `unlink()`; the abort branch calls `other unlink()`. INFERRED, off that
and `G_EntUnlink`'s callers: a successful planter stays linked until the
`delete()` of his bombzone frees the record.

VERIFIED, two earlier runs not kept in the repo: `setOrigin` on a still-linked
player is undone by the next frame's re-anchor; `unlink()` first takes effect
at once and the new origin holds. So the probe unlinks the planter before
moving him.

VERIFIED: `setorigin` is `player_methods[10]` (0x43480), in no other table.
Its body reads one vector through `Scr_GetVector`, stores it into `ps.origin`
(`client+0x14..0x1c`), adds 1.0 to z (`fld1` at 0x4351d), xors 0x8 (the
teleport bit) into `ps.eFlags` (`client+0x80`, 0x43531), copies `ps.origin`
into `r.currentOrigin` (`ent+0x134..0x13c`), and calls `trap_UnlinkEntity`,
`BG_PlayerStateToEntityState` and `trap_LinkEntity`; no store to
`ps.velocity`. INFERRED, the branch at 0x434a5: an entity with no client is a
script error. VERIFIED, the plant fixture: the teleport frame 68750 reads
`origin` `-512.0, 2688.0, -15.0` for the probe's `(-512, 2688, -16)`, `eFlags`
16 where the frame before read 24, `velocity` zero.

### 23.3 The objective table and its builtins

VERIFIED, `objective_delete`'s seven stores (0x5e0d6..0x5e112) and the 7-dword
copy in `G_UpdateObjectiveToClients` (0x511f4): `level+0x20` holds 16 records
of 28 bytes:

| Offset | Field |
|---|---|
| `+0` | state |
| `+4..+0xc` | origin |
| `+0x10` | entNum |
| `+0x14` | teamNum |
| `+0x18` | icon |

The wire form is `docs/protocol-1.1.md`, block 4; state travels as three raw
bits, enough for every value below.

VERIFIED: `ObjectiveStateIndexFromString` (0x5e008) maps `scr_const+0x24`
(`"empty"`, 0x75fbd) to 0, `+0x42` (`"invisible"`, 0x76050) to 2, `+0x14`
(`"current"`, 0x75f8f) to 4, and fails with 0 on anything else
(0x5e011..0x5e046); the error string at 0x770e0 lists the same three.

VERIFIED, a scan of all 295 `scr_const` relocations for addend 0:
`objective_current` (0x5a6ac) is the only writer of the fourth value, 1, and
`scr_const+0x0` (filled from `"active"`, 0x75f2f) is referenced once, by that
store. INFERRED: 1 is the "active" state, unreachable from script by name.

VERIFIED, `objective_add` (0x5a2d0): needs at least two parameters
(`Scr_Error` 0x77000 at 0x5a2ed); range-checks the index 0..15 (0x5a304);
detaches the slot's entity, clearing `eFlags` (`ent+0xf4`) bit 0x10 and
writing `entNum = 0x3ff` (0x5a356, 0x5a35d); inlines the three-name state map
(0x5a374..0x5a396), storing at `+0` (0x5a3c9); takes `Scr_GetVector(2)` and
writes each component truncated toward zero into `+4`, `+8`, `+0xc` (0x5a407,
0x5a42f, 0x5a461); writes `entNum = 0x3ff` (0x5a45a); runs the icon name
through `G_ShaderIndex` into `+0x18` (0x5a4da); writes `teamNum` 0 (0x5a4e2).
An icon name with a character the scan rejects, or longer than 63, is a param
error (0x76f80 at 0x5a49c, 0x76fc0 at 0x5a4b3). INFERRED, the parameter-count
branches: the vector is read only with a third parameter, the icon with a
fourth, and `teamNum` is stored last on every path.

VERIFIED: `G_ShaderIndex` (0x65ee8) scans configstrings 1 up to 0x7f at
`0x5dc + i` (0x65f2d), so an objective icon is configstring `1500 + n`, the
range and indexer base of the `shaderIndex` hudelem field
(`docs/protocol-1.1.md`, block 5).

VERIFIED:

| Builtin | Address | Writes |
|---|---|---|
| `objective_delete` | 0x5e058 | range-checks; detaches the entity (0x5e0c0, 0x5e0c7); state, origin, teamNum, icon 0, `entNum = 0x3ff` (0x5e0d6..0x5e112) |
| `objective_state` | 0x5a4f4 | mapped state at `+0` (0x5a59b); the same detach (0x5a5bf, 0x5a5c6), INFERRED only for a new state of 0 or 2 |
| `objective_icon` | 0x5a5d8 | `G_ShaderIndex` at `+0x18` (0x5a69f) |
| `objective_position` | 0x5e128 | three truncated components at `+4`, `+8`, `+0xc` (0x5e1d4, 0x5e1fc, 0x5e224) |
| `objective_onentity` | 0x5e230 | detaches the previous entity (0x5e298, 0x5e29f), sets the new one's `eFlags` 0x10 (0x5e2b3), its number at `+0x10` (0x5e2bc) |
| `objective_team` | 0x5e2c8 | `scr_const+0x4` (`"allies"`) 2, `+0x8` (`"axis"`) 1, `+0xf8` (`"none"`, 0x764c8) 0 into `+0x14` (0x5e327, 0x5e33c, 0x5e351); param error otherwise (0x77140 at 0x5e378) |

VERIFIED: the conversion in `objective_add` and `objective_position` is a
`fistp` under control word `| 0xc00` (0x5a3ec, 0x5a414, 0x5a43c; 0x5e1b9,
0x5e1e1, 0x5e209), truncation toward zero, then a `fild` into the float slot.
INFERRED: the slot holds the truncated integer as a float. VERIFIED, the plant
fixture at 91800: slot 0 reads `-176, 2473, -22` after the plant, while the
planter's origin reads `-192.8, 2457.1, -21.9`. INFERRED, off
`_utility::getPlant`'s fallback trace from `self.origin + (16, 16, 10)`: the
charge's x is `-176.8`, truncated to `-176` (nearest would give `-177`).

VERIFIED: `objective_current` (0x5a6ac) is variadic: it reads
`Scr_GetNumParam` indices, range-checks each, marks them in a 16-entry local
map (0x5a703), and stores 4 (0x5a726) and 1 (0x5a735) into record state words.
INFERRED, the loop and branches: after marking it walks all 16 records, writing
4 into every marked one and 1 into every unmarked one that read 4.

VERIFIED: the per-client filter exists twice. The exported
`G_UpdateObjectiveToClients` (0x51160) loops over clients and the 16 records,
tests state against 0, compares `teamNum` with the client's
`clientState.team` (`client+0x217c`), stores 0 at `client+0x3e8 + i*28` and
copies 7 dwords there (0x511c0..0x511f4). INFERRED, the branches: it skips a
client not in use, writes the 0 when state is 0 or `teamNum` is non-zero and
differs from the client's team, and copies the record otherwise. VERIFIED:
nothing calls it (no relocation, no resolved direct call). `G_RunFrame` carries
the identical loop inlined at 0x50977..0x50a51, ahead of its
`HudElem_UpdateClient` and `ClientEndFrame` passes; that copy is the live one.

VERIFIED,
`crates/server/tests/fixtures/playerstate/mp_carentan-sd-plant-attacker.txt`
and `-sd-defuse-defender.txt`: before the plant slots 0 and 1 both read state
4, `teamNum` 0, `entNum` 0x3ff, slot 0 at `-146, 2490, 16` with icon 8, slot 1
at `1792, 2080, 20` with icon 11, and both reach the axis client as well as the
allied one. VERIFIED, `maps/MP/gametypes/sd.gsc` in `pak5.pk3`: no
`objective_team` call, so that is the filter passing `teamNum` 0.

VERIFIED, both fixtures at 91800 (plant completes): slot 0 reads state 4, icon
14, origin `-176, 2473, -22`; slot 1 reads state 0 with icon 11 and origin
`1792, 2080, 20` still on the wire. VERIFIED, sd.gsc: the script ran
`objective_delete(0)`, `objective_delete(1)`, then `objective_add(0,
"current", bombtrigger.origin, "gfx/hud/hud@bombplanted.tga")`;
`objective_delete` zeroes origin, teamNum and icon in the level record.
INFERRED, off the inlined filter writing only the state dword for state 0: a
deleted slot reaches a client as state 0 over the six stale dwords its own
copy held, so slot 1's icon and origin survive a delete on the wire. VERIFIED,
the defender fixture at 124600 (defuse completes): slot 0 goes to state 0 the
same way, icon 14 and the charge's origin still present.

### 23.4 The three tweens

VERIFIED: each addresses its hudelem record as `g_hudelems + i*124` (`shl 5`
and `sub`, scaled by 4, e.g. 0x4c7f6..0x4c7fc), compares its time against 0 and
a cap, and carries two param-error strings: messages at 0x7495e, 0x74971,
0x748f0, 0x74902, 0x74990, 0x749a2; caps at 0x74984, 0x74ad0, 0x74adc, all
60.0. INFERRED, off the branches: each refuses a time of 0 or less or above
60.0 s with a param error.

VERIFIED: each converts its argument to ms as `time * 1000.0 + 0.5` truncated
toward zero under the x87 control-word swap; multipliers at 0x74988, 0x74ad4,
0x74ae0 read 1000.0 and addends at 0x7498c, 0x74ad8, 0x74ae4 read 0.5, so the
conversion rounds to the nearest ms.

VERIFIED, the stores:

| Tween | Address | Start time = `level.time` | Duration ms | `from` copies | Target |
|---|---|---|---|---|---|
| `scaleOverTime(time, width, height)` | 0x4bd34 | `+0x44` scaleStartTime (0x4bdea) | `+0x48` scaleTime (0x4be1b) | `+0x30` into `+0x3c` fromWidth, `+0x34` into `+0x40` fromHeight (0x4be1e, 0x4be24) | new width, height at `+0x30`, `+0x34` (0x4be2a, 0x4be2d) |
| `fadeOverTime(time)` | 0x4c720 | `+0x24` fadeStartTime (0x4c7ad) | `+0x28` fadeTime (0x4c7db) | `+0x1c` into `+0x20` fromColor (0x4c7de, 0x4c7e1) | script writes the colour after |
| `moveOverTime(time)` | 0x4c7ec | `+0x54` moveStartTime (0x4c879) | `+0x58` moveTime (0x4c8a7) | `+0x4` into `+0x4c` fromX, `+0x8` into `+0x50` fromY (0x4c8aa, 0x4c8b3) | script writes `x`, `y` after |

VERIFIED: `scaleOverTime` tests its parameter count against 3 and carries the
error 0x74920 (0x4bd5e). INFERRED, the branch and the addresses: a count other
than three is the error, and the copies precede the new size's stores, so
`fromWidth`/`fromHeight` take the old size.

INFERRED, each reading the live field, not an interpolated value: a restarted
tween's `from` is the server-side target of the tween it interrupts, not what
the client had animated to, so a tween restarted mid-flight makes the client
jump.

VERIFIED, `docs/protocol-1.1.md`'s block 5 table (entry numbers from 6): the
twelve fields sit at wire indices 14 (`fromColor`), 15 (`fadeStartTime`), 16
(`fadeTime`), 17 (`scaleStartTime`), 18 (`scaleTime`), 19 (`fromHeight`), 22
(`fromWidth`), 23 (`moveStartTime`), 24 (`moveTime`), 25 (`fromX`), 26
(`fromY`), 27 (`duration`); every record offset above matches its offset
column.

VERIFIED,
`crates/server/tests/fixtures/playerstate/mp_carentan-sd-plant-attacker.txt`:
a plant puts three elements on the attacker's wire, read as
`type:shader:width:height:fromWidth:fromHeight:scaleStartTime:scaleTime:x:y`:

| Element | Plant | Defuse (`-sd-defuse-defender.txt`) |
|---|---|---|
| icon | `3:6:64:64:0:0:0:0:320:345` | shader 7 |
| bar background | `3:2:292:12:0:0:0:0:320:385` | same |
| bar | `3:3:288:8:0:8:<start>:5000:176:385` | `3:3:288:8:0:8:114600:10000:176:385` |

`<start>` is the `level.time` of the frame the plant began. Only the bar
tweens: `fromWidth` 0, `fromHeight` 8 against a live width of 288 is
`scaleOverTime(level.planttime, level.barsize, 8)` after
`setShader("white", 0, 8)`.

VERIFIED, both fixtures: the bar's `scaleTime` is the action's whole duration.
The plant's bar starts at 86800 and the objective slots move at 91800; the
defuse's starts at 114600 and slot 0 goes to state 0 at 124600.

VERIFIED, the plant fixture: an aborted plant carries no progress. The first
hold ran from 83800 to a release at 85766; the second hold's bar enters the
wire at 86800 with `scaleStartTime` 86800 and the full 5000.

VERIFIED, the plant fixture against the wire: these elements travel in the
playerstate's archived HUD array, not the current one; reading `hud_current`
alone shows an empty column through a whole plant at `pm_type` 1. Both
committed client fixtures head their `hud=` column "unarchived HUD array", the
probe's old label (writer corrected in 17e43b0); a recapture's header reads
differently.

VERIFIED, `maps/MP/gametypes/sd.gsc` in `pak5.pk3`: `bomb_think` creates the
defuse icon on any `"trigger"` notify from a defender who `isOnGround()`, its
progress loop allows `distance(other.origin, self.origin) < 64`, and
`check_bomb` destroys the icon once `distance(self.origin, trigger.origin) <
32` stops holding. INFERRED, off the two radii: between 32 and 64 units the
icon is created and destroyed with no `wait` between, never reaching a
snapshot; a capture from 47 units out saw no icon.

### 23.5 `useButtonPressed`, `isOnGround`, `isAlive`

VERIFIED: `PlayerCmd_useButtonPressed` (0x44e70) answers
`client+0x21e8 & 0x40` (0x44ed2). `ClientThink_real` writes `client+0x21e8`
from the cmd's `buttons` byte at `client+0x20f4` on every cmd, on the
intermission path (0x3fff1) and the live path (0x40129), after copying the old
value to `+0x21ec` (0x3ffde). INFERRED: the builtin reads the last cmd's bits,
never an OR over the server frame.

VERIFIED: `PlayerCmd_isOnGround` (0x45014) answers `ps.groundEntityNum`
(`client+0x54`) `!= 0x3ff` (0x45076).

VERIFIED: `isAlive` (0x5cf8c) compares `Scr_GetType` against 7 and
`Scr_GetPointerType` against 0xd (0x5cf9f, 0x5cfb1), stores 0 as its answer at
0x5cfbb, and compares `health` (`ent+0x230`) against 0 (0x5cfcf); it reads no
sessionstate and no client pointer. INFERRED, the branches: a non-type-7 or
non-0xd argument answers 0, any other `health > 0`, so a spectator whose health
is above 0 answers true, as does any non-client entity with health.

### 23.6 Where `getPlant` puts the charge

VERIFIED, `maps/MP/_utility.gsc` in `pak5.pk3`: `getPlant` builds its start as
`self.origin + (0, 0, 10)` and its first trace origin as that plus
`vectorScale(anglesToForward(self.angles), 11)`. It runs `bulletTrace` 18 units
down from the first origin, then from the start, each under
`if(trace["fraction"] < 1)` returning the trace's `position` and
`orientToNormal(trace["normal"])`. Failing both, it traces 1000 down from six
origins (those two and the start plus `(±16, ±16, 0)`), keeps
`besttraceposition` under a strict `trace["fraction"] < besttracefraction`,
and builds the angles from `trace["normal"]` after the loop. INFERRED, off the
text: the first 18-unit hit wins, the fallback keeps the earliest of equal
fractions, and the fallback's angles come from the last trace run, not the
winner.

VERIFIED: `ClientThink_real` stores 0 into all three `angles` components
(`ent+0x140`, 0x405e2, 0x405ec, 0x405f6) and `ps.viewangles[1]`
(`client+0xc4`) into the yaw (0x40600..0x40606), and calls `G_TouchTriggers`
at 0x405b3. INFERRED, the addresses: the yaw store follows the zeroing, both
after the `G_TouchTriggers` call. INFERRED, the branches: that block runs once
per cmd on the live path; a `sessionstate` 2 client leaves through
`SpectatorThink` (0x4001d, then the jump to the exit at 0x40022) and a 3 one
through the intermission arm (0x4000a) before it. INFERRED: a planter's
`self.angles` is his view yaw with the pitch dropped, the first trace's
direction.

INFERRED, off the plant test in `crates/server/src/game/script.rs`, which runs
the stock `getPlant` from the fixture's planter origin and yaw against the
map's collision and matches slot 0 (23.3): on retail's plant neither 18-unit
trace hits (the floor under the planter is clip with no shot contents), and
the fallback's `(+16, +16)` trace wins on the flak88 `script_model` beside the
zone (`docs/research/cod11-combat.md` 2.7), putting the charge near
`(-176.8, 2473.1, -22.96)`, 23 units from the planter.

### 23.7 The probe pair against ours

Three live runs of the retail recipe (`client-probes/probe_lookat`'s README
section) against `vcod-server` on 2026-09-22, not kept in the repo. Ours reads
scripts only from `.pk3` archives, so `probe_lookat.gsc` and its `.txt` went
into a `zzz_` pak in a scratch game directory, the server run with
`--gametype probe_lookat --set probe_teleport=1`. Retail's side is the
committed plant, defuse and lookat fixtures unless stated.

VERIFIED, vcod measurement (run 1): five defects the placed-client gate cannot
reach, all fixed since: `getCvar("mapname")` read `""` (no teleports ran);
`setOrigin` on a player was missing (23.2); the bomb's `radiusDamage` handed
the killed callback an undefined attacker and `sd.gsc:782`'s
`attacker getEntityNumber()` aborted it (`docs/research/cod11-combat.md`
14.2); a planter linking while walking read `velocity` 0; and the unlinking
frame had already moved him 7.6 units (retail holds 184,27 under the link and
moves the frame after the release, 23.2). VERIFIED, vcod measurement (run 1):
the defender, still walking when the post-plant teleport landed him, kept his
velocity, slid past the probe's 28-unit stop and circled the zone until the
fuse ran out; VERIFIED (`setorigin` at 0x43480, 23.2), retail's `setOrigin`
writes no velocity either.

Run 3 against retail, every row VERIFIED both sides (vcod measurement; retail
fixtures, or retail run 9's defender probe log, not kept in the repo):

| Measure | Result |
|---|---|
| plant icon | 5700 ms into the approach on both |
| link and bar | first snapshot after the first use cmd, `pm_type` 1, bar on the same snapshot, both |
| abort's link | ours holds `183.4, 27.0, 0.0`, origin fixed, release frame the same, then `138.3, 20.3`, `103.0, 15.1`, `76.0, 11.2`; retail `138, 20`, `103, 15`, `75, 12` |
| durations | plant completes 5000 ms after its link frame; defuse bar and link 100 ms into the probe's defuse phase, complete 10000 ms later; block 4 slot 0 state 4 icon 14 at the charge, slot 1 state 0 after the plant, slot 0 state 0 after the defuse; same on retail |
| bar fields | `scaleStartTime` = first bar snapshot's `serverTime`, `scaleTime` 5000 / 10000, `fromWidth` 0, `fromHeight` 8, both |
| unlink | defender unlinks 100 ms after the defuse's completion frame, both (23.2) |
| lookat | 444 fires and 444 `isLookingAt` answers, every fire with an answer at its `getTime()`, 440 of 443 gaps 50 ms (rest the sweep's dead stations), defuse 201 fires (first 100 ms after start, last on completion); retail 444 fires, 440 gaps of 50 ms, 201 across the defuse |
| sweep | each of the 15 stations fires or stays dark as retail's |
| log | `A;1;allies;vcod;bomb_plant`, `A;0;axis;vcod;bomb_defuse`, `W;axis;vcod`, `L;allies`, same order as retail |
| bomb | entity 179; `loopSound` to the `bomb_tick` configstring at the plant, 0 at the defuse; same entity and transitions on retail run 9 |
| events and commands | as a set, ours carries retail run 9's sound events, configstring changes and server commands one for one (`MP_bomb_plant` twice on the attacker, `MP_bomb_defuse` on the defender's own ring, the announcer `s` commands) |

VERIFIED, vcod measurement (run 2): with the defender 1.5 units from retail's
station, `pitch -15` fired and `yaw +30` did not, the two stations 23.1's slab
model misses by about a unit; run 3, standing closer, matched both. INFERRED:
those sit on the trigger's edge; the difference is the station, not the trace.

Differences run 3 found, each since closed:

| Run 3 difference | Retail | Closed by |
|---|---|---|
| first linked frame `groundEntityNum` 1023 | 177 (23.2) | the linked arm and the submodel's entity number (23.2). VERIFIED, vcod measurement (`sd_plant_ab.rs`, 2026-10-16): the gate compares `groundEntityNum` on every trace, no row allowed; the release window's first frame reads 1023 as on retail once the replay leads in with hold1's last three frames of use (ours runs that frame's cmds linked); replayed from the placement it read 177 |
| hold2's last trace | `pm_type` 0 | VERIFIED, vcod measurement (`sd_plant_ab.rs`, 2026-10-16): with `probe_teleport 1` on ours too, `pm_type` 0 as retail; the gate compares `pm_type`, ground and the linked hold (to 0 units) on every trace of plant and defuse |
| fractional `velocity` (`183.4, 27.0`) | whole | VERIFIED: `PmoveSingle`'s default arm calls `trap_SnapVector` on `ps.velocity` (`ps+0x20`) at 0x34451; the snap and the walk's accel floor it exposed are in (`docs/research/cod11-mantle.md`, "The tail of the default arm", "The walk's accel floor"). INFERRED: the missing snap is why the two servers' defenders parted after the match-start restart (retail's velocity 1.3 degrees off the view, ours on it). Not re-run against ours |
| post-plant teleport one frame after completion; a `PROBE looking` beside the defuse's last fire | teleport on the completion frame; fire and no `looking` | INFERRED: thread order in a frame. Retail runs the threads the lookat queue woke before the timed waits due that frame (the drain's `Scr_RunCurrentThreads` precedes `Scr_SetTime`, 23.1), resuming the plant's `wait 0.05` loop ahead of the probe's older poller; vcod stepped every runnable thread in one pass, start order. vcod now runs the queue's rounds as retail (`cod11-gsc-language.md`, "Engine notifies and the trigger queue"). VERIFIED, `crates/server/tests/sd_plant_ab.rs` 2026-10-20 (plant cmds replayed with `probe_teleport 1`): the teleport lands on retail's frame 91800 at `-512.0, 2688.0, -15.0`, ours at the plant spot the frame before, next frame z -15.75 where retail's trace reads -15.8 (the gate's `teleport lands` row). The `looking` is not re-measured: the gate counts fires per station only |
| every configstring change, ascending, before every command | `d` and `s` in script order (`d 531`, `d 532`, `s 8`, `d 533` at the plant; run 9) | configstrings go in write order off `ConfigStrings::set`'s log (`cod11-map-cycle.md` 4.5). INFERRED: no command in these runs names a configstring set after it |
| restart with no `d 13` or `d 12`, its `d 3` `t` 0 | carries them | `cod11-map-cycle.md` 8.2: vcod sends `d 13 70050` and an ambient `t` of 70050 |
| no `sv_referencedPaks`/`sv_referencedPakNames` in systeminfo | carries both (run 9) | the server writes both (`Server::level_paks`, `cod11-server-handshake.md`, "The four cvars") |

## 24. `attach` and `detach`

VERIFIED, `G_EntAttach` (0x66190), `G_EntDetach` (0x67ed0) and the builtins
at 0x5d334 (`attach`), 0x59bac (`detach`), 0x5d3ec (`detachAll`): an entity
carries six attachment slots, model indices at `gentity_t+0x2ec` (bytes) and
tags at `+0x2f2` (script string ids, shorts).

- `attach(model[, tag])` first calls `G_EntDetach` with the same pair; a hit is
  the script error `model '%s' already attached to tag '%s'` (string 0x76aa0),
  raised after the detach took the old pair off. VERIFIED, disassembly
  0x5d39c..0x5d3bb. VERIFIED live, `probe_attach_twice`/
  `probe_attach_twice_notag` on retail 2026-10-17: the message quotes the
  strings as the script spelt them (`'XMODEL/crate_misc2a'`, `'TAG_A'`); a
  missing tag reads `''`.
- `G_EntAttach` takes the first slot whose model byte is 0 and returns 0 when
  all six are taken, before indexing anything; the builtin then raises
  `maximum attached models exceeded` (0x76ae0). VERIFIED, 0x66190 and
  0x5d3d1; VERIFIED live, `probe_attach_slots`.
- The model indexes through `G_ModelIndex`'s inlined gate
  (`cod11-gsc-language.md` section 9); the tag is stored through
  `SL_GetLowercaseString` and indexes nothing. VERIFIED, 0x66297. VERIFIED
  live: `getAttachTagName` reads `tag_a` back for an attach on `TAG_A`.
- VERIFIED, the call sites: the tag takes a configstring (`G_TagIndex`,
  0x66fa8, 108 + index) only in `ClientEndFrame`'s own-body arm, once per
  attachment of the player's own entity every frame (1.1 inlined, call at
  0x413f6; UO `G_UpdateClientInfo` 0x52c81). INFERRED, no other caller of
  `G_TagIndex`: a script model's tags never reach the configstring table, and
  a player's tag goes out the first frame his own body ends, not at the
  attach.
- `G_EntDetach` matches the first slot whose tag id equals the lower-cased
  argument's and whose model name equals the argument without case, clears it
  and closes up the slots behind it, `ignoreCollision` bits with them.
  VERIFIED, UO 0x8818a (1.1's decompile stops at the `Scr_SetString`).
  VERIFIED live, `probe_attach_slots`: after detaching the second of four (on
  `Tag_B`), `getAttachSize` reads 3 and the old third and fourth read back at
  1 and 2.
- `detach(model[, tag])` with no match prints `Current attachments:` and a
  `model: '%s', tag: '%s'` line per slot, then raises `failed to detach model
  '%s' from tag '%s'`. VERIFIED, 0x59c33..0x59cb4 and the strings at 0x76b01,
  0x76b17, 0x76b40; VERIFIED live, `probe_detach_missing`.
- `getAttachModelName`, `getAttachTagName` (and UO's
  `getAttachIgnoreCollision`) take an index below 6 whose slot holds a model;
  anything else is `Scr_ParamError(0, "bad index")`. VERIFIED, UO 0x67e71 and
  0x67f6a. VERIFIED live on 1.1d, `probe_attach_badindex` and
  `probe_attach_badtag`: index 1 of a one-attachment entity dies with
  `bad index`.

vcod (`crates/server/src/game/builtins/attach.rs`): the list is the compacted
slot array, `attach` checks the pair and the count in retail's order, `detach`
is one builtin for all three protocols, and `client_attachments`
(`game/script.rs`) indexes a playing or dead client's tags each frame from
`ClientEndFrame`'s pass (`server/cod1_frame.rs`), so the `d` is queued ahead of
the frame's snapshots. Trap: indexing them in the snapshot build, after the
frame's writes went out, sends the first snapshot naming a new tag a message
ahead of its string; UO's ctf flag icon (`TAG_HELMETSIDE`), whose root bone
has no name, was then grafted as a new root for a frame
(`cod11-client-soak.md`, the UO sweep;
`two_clients::an_attachments_tag_reaches_the_client_before_its_snapshot`).
`crates/server/tests/semantics_ents.rs` `probe_attach_matches_retail` replays
the six probes.

## 25. `addTestClient`

VERIFIED, the relocations at 0x5f6ee and 0x5f6fb: `addTestClient()`
(`functions[103]`, 0x5f6e8) is `trap_AddTestClient` then `Scr_AddEntity` of
what it returns, or nothing. The engine half is `SV_AddTestClient`
(cod_lnxded 0x808753c, syscall 0x41):

- VERIFIED, 0x8087548..0x80875ea: it takes the first `CS_FREE` slot from 0,
  builds
  `connect "\cg_predictItems\1\cl_anonymous\0\handicap\100\color\4\head\default\model\multi\snaps\20\rate\5000\name\bot%d\protocol\%d"`
  from a process-wide counter (0x80e2f9c, 0 in `.data`) and the protocol,
  makes a bot address (type 0) whose port is that counter, counts it up and
  runs `SV_DirectConnect`.
- VERIFIED, both functions: `SV_DirectConnect`'s reconnect check walks every
  slot, free ones included, and refuses a base-address match whose
  `lastConnectTime` is within `sv_reconnectlimit` seconds
  (0x80855e6..0x8085655); a bot address compares by port
  (`NET_CompareBaseAdr` 0x8080e60, type 0). INFERRED: the counter's first bot,
  port 0, matches every never-used slot (address zero) and is refused for the
  server's first 3 s; every later bot's port no slot carries. VERIFIED live,
  `probe_testclient`: a call at the first `wait 0.05` returns undefined and
  the next, past `wait 5`, is `bot1`.
- VERIFIED, 0x808769d..0x8087767: when the connect took, it sets
  `client_t+0x5a8f4` (the bot flag: no reliable commands, no timeout, no
  snapshot), sends the gamestate, sets `CS_ACTIVE`, the gentity and
  `lastUsercmd` (zeroed) and calls `ClientBegin`.
- VERIFIED live, `probe_testclient` on retail 2026-10-17: the connect callback
  runs inside the call up to its `waittill("begin")`, `sessionstate`
  `spectator`; the call returns the entity (number 0, name `bot1`); the
  `begin` waiters run only once the calling thread reaches its `wait`, two
  clients' in reverse connect order (`begin 1` before `begin 0`).
- VERIFIED, 0x40660: a test client sends no usercmds and `G_RunClient` thinks
  only under `g_synchronousClients`, so nothing moves it. VERIFIED live:
  spawned 200 over a spawn point it reads `dz 195.993` at the spawn and a
  second later, `isOnGround` 0.
- VERIFIED, pak5: no stock MP script calls it; `addBotClients` is commented
  out in every stock gametype (dm, tdm, sd, re).

vcod (`builtins/client.rs` `add_test_client`): the host connects the slot and
runs the callback inside the call, queues the `begin` notify, and
`Server::install_test_client` gives the slot an active `Client` with no
netchan and no bot brain. The counter lives in `Server` and survives a map
change. Not modelled: a slot a UDP client once held (address no longer zero,
so retail's port-0 bot is not refused there), and the test client across a
map change.

## Open, and worth a probe

- Answered: same-deadline waits resume newest-queued first
  (`cod11-gsc-language.md`, `probe_wait0_yield`), which 23.7's thread order
  needed.
- Whether `Scr_FindField` searches only the radiant fields (section 7).
- Whether `Scr_AddFields` really reads `radiant/keys.txt` (section 6).
- The name-to-index direction of the three HUD enum tables (section 5). The
  tables are a data read; that index 0 is `left`, not `right`, rests on the
  helper at 0x4af80. A probe writing each name and reading it back settles all
  three.
- Type 6, the `(0, f, 0)` angle form, is unused by all three server tables; it
  may exist for the client's field tables (`cgame_mp_x86.dll` not checked).
