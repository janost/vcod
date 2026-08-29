# CoD 1.1 gsc entity object model

What a script object is on the server side: which field names the engine backs
with real state, which ones fall through to a per-object script store, which
BSP classnames get an engine spawn function, and where the builtin tables
actually live. This is the measurement pass that stage 2 of the gsc gameplay
program depends on.

Evidence rules as everywhere in this directory. VERIFIED means I read it out
of the binary or the shipped assets myself; INFERRED means I read it off
disassembly without a live test and said so. The module throughout is
`game.mp.i386.so`, the 1.1d Linux dedicated server's MP game module, which
carries a full dynamic symbol table. Addresses are module-relative (image base
0), which is what `nm -D` and `objdump -d` print for it.

Two tools in `tools/re/` reproduce every table below in a second:

```
python3 tools/re/dump_script_fields.py game.mp.i386.so all
python3 tools/re/dump_builtins.py     game.mp.i386.so all
```

## 1. One trap that cost the previous pass three wrong claims

Pointers stored in `.data` read as zero in the file. The module is a shared
object, so `SP_info_null`, `ScriptEntCmd_MoveTo` and every other address in a
static table arrives as an `R_386_32` relocation in `.rel.data` with the
in-place word left at 0. Read the raw dwords and every function pointer looks
null.

That artifact produced three claims in `cod11-gsc-language.md` section 7 that
are wrong, all now corrected there: that a handful of builtin records carry a
null function pointer, that `getent` and `getentarray` are among them, and
that the `move*`/`rotate*` builtins sit in an uncharacterised `.rodata`
whitelist with a constant zero second field. Nothing has a null function
pointer, and the movers are an ordinary dispatch table in `.data`. Both
dumpers resolve `.rel.data` and print the symbol.

## 2. Struct sizes, VERIFIED

- `gentity_t` is 0x314 (788) bytes. `Scr_GetEntityField` 0x6282f and
  `Scr_SetEntityField` 0x6264f both index the entity array with
  `imul $0x314`.
- `gentity_t.client` is at +0x158. Both accessors test it for null before
  routing a client field.
- A HUD element is 124 bytes. `Scr_GetHudElemField` 0x4c003 computes
  `(n * 32 - n) * 4`.

Field offsets in the tables below are byte offsets into those structs.
Three of them cross-check against unrelated code, which is how I know the
tables are being read at the right stride:

- `origin` at 308 = 0x134, and `G_CallSpawn` passes `ent + 0x134` to
  `G_SetOrigin` (0x6168d).
- `angles` at 320 = 0x140, passed to `G_SetAngle` (0x6169d).
- `model` at 373 = 0x175, and the type-8 branch of `G_ParseField` writes a
  byte to `0x175(%edi)` (0x6150a).

## 3. The field tables, VERIFIED

Four functions register script fields. Three walk a static array; the fourth
does not.

| function | address | table | stride | records |
|---|---|---|---|---|
| `GScr_AddFieldsForEntity` | 0x62400 | 0x78e68 | 16 | 34 |
| `GScr_AddFieldsForClient` | 0x41700 | 0x72ed4 | 20 | 13 |
| `GScr_AddFieldsForHudElems` | 0x4bf84 | 0x744e0 | 20 | 11 |
| `GScr_AddFieldsForRadiant` | 0x62470 | none, see section 6 | | |

Each walk ends at a record with a null name pointer. Record layout:

```c
struct entity_field  { char *name; int offset; int type; void (*set)(); };            // 16
struct object_field  { char *name; int offset; int type; void (*set)(); void (*get)(); }; // 20
```

A non-null `set` or `get` replaces the generic conversion for that one field.
`Scr_GetEntityField` never consults a getter, so entity fields read through
the generic path unconditionally; the client and HUD tables use both hooks.

Each registering function filters on `type` and skips records it does not
accept, so a skipped record is not a script field at all. Entity and HUD
elements accept 0 to 5, 7 and 8; clients accept 0 to 5 and 7. Exactly one
record in the three tables is skipped: entity `light`, type 9, offset 0. It is
a Radiant-only key that never reaches script.

The index still advances across a skipped record, so entity field id 0x0e is
permanently unused.

## 4. The storage type enum, VERIFIED

`Scr_GetGenericField` (0x6248c) switches on `type` through a jump table at
0x7968c. Read in index order:

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

Type 6 is the yaw-only angle form. No record in any of the three tables uses
it, and both entity and HUD registration reject it, so on the server it is
dead in 1.1 MP.

## 5. Field ids and how one namespace covers two objects

Client fields are tagged. `GScr_AddFieldsForClient` ORs 0xC000 into the index
before registering (`or $0xc0,%ah` at 0x41740), so client field ids run
0xC000 to 0xC00C while entity and HUD ids are plain indices.

`Scr_GetEntityField` and `Scr_SetEntityField` both start by testing
`id & 0xC000 == 0xC000`. On a hit they take `ent->client`, mask the id down
with `and $0x3f,%bh` and hand off to `Scr_GetClientField` / `Scr_SetClientField`.
On a miss they index the entity table directly.

So there is one flat field namespace per script object kind, resolved by name
at registration time, and the tag is what lets `self.score` on a player and
`self.origin` on the same entity go to two different structs through one
opcode. An entity with no client and a client-tagged field id is a runtime
error, not a silent undefined.

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

`speed` and `time` share offset 480, `count` and `shard` share 592, `_color`
and `color` share 668. Those are aliases, not a misread.

The three shared setters at 0x6262c on `classname`, `model` and `spawnflags`
are the same function, which is the "read-only after spawn" guard.

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

`pers` is type 7, an object handle, which is how `self.pers["team"]` works
without the engine knowing anything about the key.

### HUD element fields, 0x744e0

```
 id  name        off  type
0x00 x             4  0
0x01 y             8  0
0x02 fontscale    12  1
0x03 font         16  0
0x04 alignx       20  0
0x05 aligny       24  0
0x06 color        28  0   custom get and set
0x07 alpha        28  0   custom get and set
0x08 label        44  0
0x09 sort        108  1
0x0a archived    120  0
```

`color` and `alpha` share offset 28 and both carry hooks, which pack and
unpack the byte lanes of one packed RGBA word.

## 6. Radiant fields come from a file, not a table

`GScr_AddFieldsForRadiant` (0x62470) is three instructions:
`Scr_AddFields("radiant", ".txt")`. There is no static array.

`radiant/keys.txt` ships in the stock `pak4.pk3`, 7303 bytes, and its own
header states the syntax as one line per key, a type keyword of `float`,
`int` or `string`, then the key name. It holds 113 keys: 56 int, 43 string,
14 float. 99 of them are the `script_*` family the SP campaign scripts read
(`script_delay`, `script_noteworthy`, `script_exploder` and so on). The other
14 are `delay`, `count`, `radius`, `spawnflags`, `export`, `dontdropweapon`,
`dontdrawoncompass`, `target`, `targetname`, `groupname`, `name`,
`weaponinfo`, `vehicletype` and `ambient`.

Six of those 14 are also entity field names (`delay`, `count`, `radius`,
`spawnflags`, `target`, `targetname`), and stage 1 of `G_ParseField` runs
first, so the radiant entry for them never fires during a map load.

INFERRED: `Scr_AddFields` is an engine callback reached through the syscall
pointer at 0xc10a4, so what it does with the two arguments is not visible in
this module. The name, the extension and the file that exists at that path
make "load `radiant/keys.txt` and register each line as a typed script field"
the only reading I can construct, but I have not watched it happen.

## 7. What the map load actually does with an entity block

`G_CallSpawn` (0x615e4) runs three passes over the classname, in this order.

1. No `classname` spawn var at all. `G_Printf` a warning and return. No
   entity is created.
2. `bg_itemlist`, stride 0x30, matched with `strcmp`. Hit means `G_Spawn`,
   apply every spawn var, `G_SetOrigin`, `G_SetAngle`, `G_SpawnItem`.
3. `spawns` (section 8), stride 8, matched with `strcmp`. Hit means `G_Spawn`,
   apply every spawn var, `G_SetOrigin`, `G_SetAngle`, then call the record's
   `SP_` function.
4. Neither table matched. `G_Spawn`, apply every spawn var, `G_SetOrigin`,
   `G_SetAngle`, and nothing else.

Case four is the one the object model rests on. An unknown classname still
produces a live `gentity_t` with all of its Radiant keys applied. That is why
`mp_deathmatch_spawn`, which is in neither table, survives the map load for
`_spawnlogic.gsc` to find with `getentarray`.

Both classname searches use `strcmp`, so a classname match is case-sensitive.
Key names are not, see below.

### Applying one key/value pair

The spawn-var applier is the unnamed function at 0x61400, which I will call
`G_ParseField`. It is two stages:

1. Walk the entity field table with `Q_stricmp`, so a Radiant key matches an
   engine-backed field name case-insensitively. On a hit, convert the value
   string according to the field's type and write it into the `gentity_t`.
   `%f %f %f` through `sscanf` for a vector, `strtol` for an int, `strtod` for
   a float, `G_NewString` plus an interned id for a string. A `model` value
   starting with `*` is a brush model, and its number goes to `ent+0x8c`
   instead.
2. On a miss, call `Scr_FindField(key, &type)`. A nonzero result means the key
   is a registered script field. Convert the value by that type (1 gives
   `Scr_AddString`, 4 gives `strtod` then `Scr_AddFloat`, 5 gives `strtol`
   then `Scr_AddInt`) and store it with `GScr_SetDynamicEntityField`.
3. Neither. The key/value pair is dropped silently.

So the split stage 2 needs is three-way, not two-way. Engine-backed fields
land in the C struct. Registered script fields land in the entity's script
object. Everything else in the BSP entity lump is discarded at load and is
not visible to script at all.

INFERRED: `Scr_FindField` is also an engine callback (0xc10a8), so I cannot
see whether its search covers only the radiant fields or every registered
field including client and HUD names. Stage 2 should assume the narrow
reading and measure it with a probe if a map ever depends on the wide one.

## 8. The `spawns` classname table, VERIFIED

0x7eb30, 8-byte records `{char *name; void (*spawn)(gentity_t *);}`, ends at a
null name. 27 entries:

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

`func_group` deliberately maps to `SP_info_null`; grouping is a compile-time
concept and the runtime entity is inert. `mp_target_location` shares
`SP_target_location`, and `misc_mg42` and `misc_turret` share `SP_turret`.
`worldspawn` is absent because `G_SpawnEntitiesFromString` handles entity 0
before this loop.

None of the MP gametype spawn classnames (`mp_deathmatch_spawn`,
`mp_teamdeathmatch_spawn`, the `mp_*_intermission` markers) are in the table,
which is the case-four path above.

## 9. Builtins are five tables, VERIFIED

This closes the "record struct never fully resolved" gap from
`cod11-gsc-language.md` section 7, which now points here.

| table | address | stride | count | walked by |
|---|---|---|---|---|
| `functions` | 0x7e508 | 12 | 106 | `Scr_GetFunction` 0x5c15c |
| player methods | 0x733dc | 8 | 46 | `Player_GetMethod` 0x448e4 |
| scriptent methods | 0x78d40 | 8 | 12 | `ScriptEnt_GetMethod` 0x60f60 |
| hudelem methods | 0x749b4 | 8 | 14 | `HudElem_GetMethod` 0x4be38 |
| entity methods | 0x7ea00 | 8 | 38 | unnamed, 0x5c1c0 |

216 builtins in total. Every walk is a linear `strcmp` over a hardcoded count,
not a null terminator, so the counts above are the loop bounds
(0x69, 0x2d, 0x0b, 0x0d, 0x25 respectively, all inclusive compares).

`functions` records are `{char *name; void (*fn)(); int developer;}` and the
method records are `{char *name; void (*fn)();}`. That difference in stride is
the whole reason a uniform 8-byte scan of `[0x7e508, 0x7eb30)` garbles the
first table and lands on 144 (106 plus 38) instead of the real figure.

`developer` is 1 on exactly three entries: `print`, `println` and `assert`.
CoDExtended's `src/script.c` declares the same struct as
`SCRIPTFUNCTION { name, function, developer }` and the same
`Scr_GetFunction(const char **fname, int *fdev)` signature, which is an
independent read on both the field name and the out-parameter.

`Scr_GetFunction` also writes the table's own spelling back through `*fname`
(0x5c199), so the engine canonicalises a builtin name on lookup.

`Scr_GetMethod` (0x5f724) tries player, then scriptent, then hudelem, then
falls back to the entity methods, first hit wins. A name in an earlier table
shadows the same name in a later one. `spawn`, `iprintln` and `iprintlnbold`
each appear both in `functions` and in the player methods, which is the
`self iprintln(...)` versus bare `iprintln(...)` split.

The movers are the scriptent table, ordinary code pointers in `.data`:
`moveto`, `movex`, `movey`, `movez`, `movegravity`, `rotateto`,
`rotatepitch`, `rotateyaw`, `rotateroll`, `rotatevelocity`, plus `solid` and
`notsolid`. `ScriptEnt_GetMethod` sits directly above the entity field table
in `.data`, which is probably why the previous pass mistook the region for a
`.rodata` string list.

## 10. `getEntArray` returns ascending entity number, VERIFIED

The stage 2 handoff recorded this as unproven, because the capture only shows
targetnames and "ascending entity number" was a guess at the mechanism. It is
now measured from both ends.

`Scr_GetEntArray` (0x61980) walks `esi` from 0 to `level.num_entities`
(`level`+0xc), skips a slot whose `inuse` byte at +0x160 is clear, and appends
matches in that order. The keyed branch first checks the entity field table
type at 0x78e70 is 3, the interned-string form, which is why the keys the
corpus actually passes (`targetname`, `classname`, `target`, `teamname`, and
script-defined string keys) are all type 3 or script-defined and no numeric
field is ever used as a key.

The capture confirms it. `probe_ents` ran on `mp_pavlov` and retail returned
the four map `script_origin` entities as auto5, auto4, auto3, auto6, which
looks arbitrary until you read the BSP. `mp_pavlov.bsp`'s entity lump holds
345 blocks, and its four `script_origin` blocks sit at indices 2, 3, 4 and
344, with exactly those targetnames in exactly that order. Entity lump order
gives entity number, entity number gives `getEntArray` order.

The corpus passes six distinct keys: `targetname` (1183 call sites across both
builtins), `classname` (131), `script_noteworthy` (31), `target` (5), `export`
(2) and `team` (1). Two of those are `radiant/keys.txt` entries and one
(`team`) is neither an engine field nor a radiant key, so the lookup has to go
through the same three-way field resolution as a plain `.name` read rather
than a special case for the engine table.

## 11. Entity numbering, VERIFIED

`G_InitGame` (0x4fb14) sets `level.num_entities = 72` at 0x4fdc6. `G_Spawn`
(0x667e0) hands out `level.num_entities` and increments, falling back to a
scan for a freed slot only once the counter reaches 0x3fe, which is
`ENTITYNUM_WORLD` (1022). `Scr_GetEntArray` reads the same `level+0xc`.

So the first entity the map load creates is number 72, whatever
`sv_maxclients` is. That corrects the gameplay design's "allocating upward
from `sv_maxclients`". `MAX_CLIENTS` is 64 on CoD 1.1, and slots 64 to 71 are
reserved and never handed out by `G_Spawn`; I have not established what
reserves them.

`G_SpawnEntitiesFromString` (0x622dc) parses the first entity block, calls
`G_Error` if its classname is not `worldspawn`, runs `SP_worldspawn` on it,
and only then loops `G_ParseSpawnVars` plus `G_CallSpawn` over the rest. The
world is therefore not allocated through `G_Spawn` and does not consume a
number in the 72-and-up range.

## 12. What stage 2 takes from this

- The object handle table needs four kinds, and the field id is the
  discriminator on two of them: a plain index for entity, `| 0xC000` for
  client, a plain index again for HUD elements in their own namespace.
- Field lookup by name is case-insensitive, both at script level and for
  Radiant keys during the map load. Field ids are dense small integers and can
  be a `u16` in the host.
- `getentarray` sees case-four entities, in ascending entity number, which is
  BSP entity lump order. The lump must produce a `gentity_t` for every block
  with a classname, whether or not the classname is in `spawns`, numbered
  from 72 up.
- A Radiant key that is neither an engine field nor in `radiant/keys.txt` is
  dropped at load. Stage 2 should drop it too, not stash it, or scripts will
  see fields retail does not have.
- 216 builtin names is the ceiling for the host's dispatch, and the
  five-table split with its lookup order is the shape to copy. The 37 names
  `mp_pavlov` needs are a subset of it.

## Open, and worth a probe

- Whether `Scr_FindField` searches only the radiant fields. Section 7.
- Whether `Scr_AddFields` really reads `radiant/keys.txt`. Section 6.
- Type 6, the `(0, f, 0)` angle form, is unused by all three server tables. It
  may exist for the client's own field tables; I have not looked at
  `cgame_mp_x86.dll` for this.
