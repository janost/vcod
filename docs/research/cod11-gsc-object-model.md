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
0x03 font         16  0   custom get and set
0x04 alignx       20  0   custom get and set
0x05 aligny       24  0   custom get and set
0x06 color        28  0   custom get and set
0x07 alpha        28  0   custom get and set
0x08 label        44  0   custom set
0x09 sort        108  1
0x0a archived    120  0   custom set
```

`color` and `alpha` share offset 28 and both carry hooks, which pack and
unpack the byte lanes of one packed RGBA word.

`font`, `alignx` and `aligny` are stored as ints but script reads and writes
them as names, so for these three the type column is the storage, not what
script sees.

VERIFIED that the field takes a name, from the shipped assets: `startGame` in
`maps/MP/gametypes/dm.gsc` (pak5) writes `level.clock.alignX = "center"` on a
HUD element, and `dm` is a stock gametype on the stock maps, so a field that
took nothing but an int would break the round clock on every retail MP map.
VERIFIED, as a data read, that each of the three carries a set and a get hook
and which name table it points at; both tables below come straight out of the
module.

| field | setter | getter | name table | names, in index order |
|---|---|---|---|---|
| `font` | 0x4c2f8 | 0x4c320 | 0x7de04 | `default`, `bigfixed`, `smallfixed` |
| `alignx` | 0x4c350 | 0x4c378 | 0x7de10 | `left`, `center`, `right` |
| `aligny` | 0x4c3a8 | 0x4c3d0 | 0x7de1c | `top`, `middle`, `bottom` |

INFERRED FROM DECOMPILATION, the mechanism: each setter is a four-line wrapper
around a shared helper at 0x4af80 taking `(elem, record, names, count)`, and
the helper `strcmp`s the script's string against the name table, stores the
matching index, and otherwise builds a message from the whole table and calls
`Scr_Error`. Each getter indexes the same table back to a string through
`Scr_AddString`. The name-to-index direction is not measured live; a probe
that sets each name and reads it back would settle it.

`label`'s hook (0x4c400) is a setter only. INFERRED that it takes a localized
string rather than a name from a table: it has no name-table argument and no
call into the 0x4af80 helper.

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
   `SP_` function. Five of those functions free the entity again, so the
   block ends up with none (section 13).
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

   A type-8 `model` value that is not a brush model goes through
   `G_ModelIndex`, VERIFIED: the call at 0x61505 relocates against that
   symbol and the byte it returns is stored at `ent+0x175`. So **the entity
   lump fills the model configstring block before any script runs**, in lump
   order, and the script's own `precacheModel`/`setModel` calls continue from
   wherever that left off. That conclusion is VERIFIED independently by the
   committed retail captures: `mp_carentan-dm.txt` opens the model block with
   map props (`xmodel/static_vehicle_german_truck` and the rest of the lump)
   and reaches the team player models only after them. That is why our model
   block is offset against both captures: we store the name without indexing
   it.
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
concept and the runtime entity is inert, and `SP_info_null` frees it outright
(section 13). `mp_target_location` shares
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

`getEntArray()` with **no** arguments is a second form of the same builtin.
`Scr_GetEntArray` opens with a parameter-count call at 0x61989 and branches
on the result: zero parameters take the path at 0x61997, which is the same
slot walk with the field compare dropped, so it returns every entity in use
in the same ascending order. `maps/mp/gametypes/_gameobjects.gsc::main` is
the stock caller, and it is reached from every gametype's
`Callback_StartGameType`, so a host that only implements the keyed form kills
that thread on the first line. This one is read from the disassembly, not
measured by a probe.

The corpus passes six distinct keys: `targetname` (1183 call sites across both
builtins), `classname` (131), `script_noteworthy` (31), `target` (5), `export`
(2) and `team` (1). Two of those are `radiant/keys.txt` entries and one
(`team`) is neither an engine field nor a radiant key, so the lookup has to go
through the same three-way field resolution as a plain `.name` read rather
than a special case for the engine table.

## 11. Entity numbering, VERIFIED

`G_InitGame` (0x4fb14) sets `level.num_entities = 72` at 0x4fdc6. `G_Spawn`
(0x667e0) checks the free list first (section 14) and only bumps
`level.num_entities` when it is empty; a bump that would hand out 0x3fe,
which is `ENTITYNUM_WORLD` (1022), goes to `G_Error` instead.
`Scr_GetEntArray` reads the same `level+0xc`.

A previous pass read that 0x3fe compare as the point where `G_Spawn` starts
looking for freed slots. It is the other way round: the free-list check is
unconditional and comes first, at 0x667e9, and 0x3fe is the exhaustion
error. Section 14 has the structure and the live measurement.

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
  BSP entity lump order. Numbering runs from 72 up. It is *not* one entity per
  block with a classname: five classnames free the entity their `SP_`
  function was handed and the slot is reused at once, so those blocks
  consume no number (sections 13 and 14).
- A Radiant key that is neither an engine field nor in `radiant/keys.txt` is
  dropped at load. Stage 2 should drop it too, not stash it, or scripts will
  see fields retail does not have.
- 216 builtin names is the ceiling for the host's dispatch, and the
  five-table split with its lookup order is the shape to copy. The 37 names
  `mp_pavlov` needs are a subset of it.

## 13. Five classnames consume their block without an entity, VERIFIED

An earlier version of section 12 said the lump produces a `gentity_t` for
every block with a classname whether or not the classname is in `spawns`.
That is wrong. `G_CallSpawn`'s third case runs the record's `SP_` function
after the entity is built, and four of those functions are nothing but
`G_FreeEntity(self)`:

| function | address | classnames it serves |
|---|---|---|
| `SP_info_null` | 0x531cc | `info_null`, `func_group` |
| `SP_light` | 0x53204 | `light` |
| `SP_misc_model` | 0x53224 | `misc_model` |
| `SP_corona` | 0x53274 | `corona` |

Each body is identical: push the argument, one `R_386_PC32` call relocated
against `G_FreeEntity` (at 0x531d9, 0x53211, 0x53231, 0x53281), return. No
branch, no condition. `python3 tools/re/dump_builtins.py <game.mp.i386.so>
spawns` marks these five rows `frees`; the check is a scan of the function's
`.rel.text` relocations for `G_FreeEntity`, so it covers all 27 records
whatever a map happens to contain.

Freeing the entity does not leave a hole, because `G_Spawn` hands the slot
straight back out (section 14). A block with one of these classnames
therefore consumes no entity number at all.

### The live counts

`probe_spawnfree`, a throwaway gametype script, logged
`getEntArray(<classname>, "classname").size` for all 27 `spawns` classnames
on the retail 1.1d Linux dedicated server (2026-08-29, `sv_maxclients 8`).
Lump counts are from the shipped BSPs. Every classname whose live count is
below its lump count is one of the five above; every other classname's live
count equals its lump count exactly.

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

The remaining 14 `spawns` classnames reported 0 live on all three maps and
appear in no stock MP map's entity lump, so the live measurement says
nothing about them: `func_door`, `func_static`, `func_rotating`,
`func_bobbing`, `func_pendulum`, `func_group`, `func_door_rotating`,
`trigger_once`, `target_location`, `mp_target_location`,
`misc_teleporter_dest`, `misc_turret`, `misc_spawner`, `trigger_damage`.
The relocation scan above covers them, and it puts only `func_group` in the
freeing set. `misc_teleporter_dest`'s `SP_` function (0x5321c) is an empty
body, which is a keep, not a free.

### The arithmetic checks out on every stock map

Number of the first entity spawned after the map load, retail against
`72 + (blocks - 1 - freeing blocks)`:

| map | lump blocks | freeing blocks | predicted | retail |
|---|---|---|---|---|
| mp_pavlov | 345 | 117 | 299 | 299 |
| mp_chateau | 1176 | 1030 | 217 | 217 |
| mp_powcamp | 1065 | 764 | 372 | 372 |
| mp_railyard | 533 | 328 | 276 | 276 |

mp_pavlov's figure is `# probe_ents` in
`crates/gsc/tests/fixtures/semantics/retail-captures.txt`, whose three
script-spawned entities are 299, 300 and 301 behind the map's four
`script_origin`s at 73, 74, 75 and 298. The other three are
`probe_spawnfree`'s `spawn1` line. No stock MP map's lump has a block
without a classname, so the block count is the whole story.

## 14. `G_Spawn` drains a free list before it bumps the counter, VERIFIED

`level` carries a singly linked FIFO of freed entities: head at `level+0x10`,
tail at `level+0x14`, next pointer at `gentity+0x304`.

`G_Spawn` (0x667e0) reads the head at 0x667e9 and, when it is not null,
takes that entity (0x668c8), pops it, and clears the tail too if the list
is now empty. Only an empty list reaches the counter path at 0x6688b, and
only that path can hit the 0x3fe `G_Error`.

`G_FreeEntity` (0x66948) appends at the tail (0x66bb3), after a guard at
0x66bb1 that skips entities whose index is 71 or below, so the 0..71
reserved range never enters the list. It also bumps a per-entity generation
counter at `gentity+0x300`, which is what stale script handles are checked
against.

Measured on mp_chateau (2026-08-29): after the map load the first spawn was
217 and the second 218. Deleting the first and spawning again gave 219, not
217; a fourth spawn gave 220. After `wait 0.5` the next spawn was 217, the
deleted entity's number, and the one after it 221, continuing from the
high-water mark. Two facts fall out of that:

- A freed slot **is** handed back out, ahead of the counter, in FIFO order.
- `delete()` does not free the entity when it is called. The `delete` entity
  method (0x5da14) notifies, unlinks, clears the touch and use handlers, and
  sets `think = G_FreeEntity` with `nextthink = level.time + 100`
  (0x5da9a, 0x5daa4). The free happens a tenth of a second later, off the
  entity think. During the map load the `SP_` functions call `G_FreeEntity`
  directly, so there is no such delay there.

## 15. The item registry (`bg_itemlist`), configstring 8, VERIFIED

`bg_itemlist` (`.data` 0x7b9d8, stride 0x30, `bg_numItems` = 70 at `.rodata`
0x70804, `python3 tools/re/dump_itemlist.py`) carries a compiled-in
classname for only five of its 70 rows: `item_ammo_stielhandgranate_open`
(65), `item_ammo_stielhandgranate_closed` (66), `item_health_small` (67),
`item_health` (68), `item_health_large` (69). Index 0 is blank. Indices
1-64 hold placeholder classnames `emptyitem_"w01"` .. `emptyitem_"w64"`; a
grep of the binary finds no `mp40_mp`-shaped string anywhere in it, so a
real weapon classname reaches its slot only at runtime, from the mounted
paks' weapon files, in an order the static dump cannot recover.

**R1 (VERIFIED against two live captures):** a weapon's item index is its
1-based index into configstring 7's weapon list
(`crate::configstrings::WEAPON_LIST`). Decoding
`crates/server/tests/fixtures/configstrings/mp_{pavlov,carentan}-dm.txt`
line 13 (configstring 8) by `SaveRegisteredItems`' packing (below) and
naming each set bit through `WEAPON_LIST` accounts for every bit with no
leftovers and no cross-theatre contamination:

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

Pavlov (Russian v German) sets Russian and German weapons only; Carentan
(American v German) sets American and German only. A wrong ordering would
scatter names across theatres.

**The packing (VERIFIED against both captures):** `SaveRegisteredItems`
(`.text` 0x4ef08) walks `0..bg_numItems`, accumulates `1 << (i & 3)` into a
nibble, and every fourth item emits one lowercase hex character (`+0x30`
under 10, `+0x57` at or above), flushing the trailing partial nibble the
same way. `bg_numItems` = 70 makes an 18-character string.

**M1, is index 0 unconditional?** `ClearRegisteredItems` (`.text` 0x4eecc)
resolves, via its relocations, to `__bzero(itemRegistered, 0x400)` — a
whole-array zero with no special case for index 0. Index 0's classname is
blank, so no `precacheItem` call can ever name it either. `RegisterItem`
(`.text` 0x4e504, mirrored here as `Items::register`) is the only function
that writes `itemRegistered` (`.bss` 0x18e0e0); its four call sites all
pass a computed argument, never a literal 0, but two are inside
`BG_GivePlayerWeapon` (`.text` 0x36a38, call sites at +0x4f and +0xe1),
which runs for every weapon a player is given, including the default "no
weapon" slot at index 0. INFERRED FROM DECOMPILATION that this is the
actual trigger (not single-stepped); VERIFIED that both retail captures set
bit 0 with no per-map factor in common besides the `dm` gametype.
`Items::new` reproduces this by seeding index 0 registered unconditionally.

**M2, does registering a weapon also register its alt-fire mode?** Yes,
and it is engine/weapon-definition behaviour, not a placed-entity
artifact — Task 8 owns these bits. In `RegisterItem`, after the item's own
bit is set, it checks `bg_itemlist[index]`'s `giType` field (offset 0x20)
against `1` (`IT_WEAPON`); on a match it calls `BG_GetInfoForWeapon`
(`.text` 0x3ac68, resolved via `readelf -r`) for the weapon's definition,
reads a "next" index at offset 0x2fc of that definition, registers it
directly (bypassing the "already registered" short-circuit), precaches two
of its model fields (offsets 0x188 and 0x31c) through `G_ModelIndex`
(`.text` 0x66ed8), and repeats until the next index is 0 or loops back to
where the chain started. `BG_GivePlayerWeapon` (0x36a38, `.text` 0x36b0a)
reads the same offset-0x2fc field and explicitly registers it too when a
player is given a weapon, which is why this is weapon-definition data and
not something a map entity supplies. INFERRED FROM DECOMPILATION that
offset 0x2fc specifically carries the alt-fire link (not single-stepped);
VERIFIED that every `*_semi_mp`/`*_slow_mp` bit in both captures above sits
at the `WEAPON_LIST` index immediately after its base weapon's bit
(`bar_mp`/`bar_slow_mp`, `fg42_mp`/`fg42_semi_mp`, `mp44_mp`/
`mp44_semi_mp`, `ppsh_mp`/`ppsh_semi_mp`, `thompson_mp`/
`thompson_semi_mp`), which is the adjacency rule `Items::register`'s
`alt_weapon_index` derives from `WEAPON_LIST` rather than a second
hardcoded pairing table.

**A gap this task does not close:** the same `RegisterItem` also precaches
two further model fields for *every* item, at offsets 0x8 and 0xc of its
`bg_itemlist` record — `item_health`'s offset-0x8 field is the compiled-in
string `xmodel/health_medium`, matching the model retail's capture has
queued at the corresponding model configstring slot. `Items::register`
only reproduces the registration bit and the alt-fire link, as scoped; a
weapon's own offset-0x8/0xc/0x188/0x31c model strings are runtime data from
the weapon file this crate does not parse yet, so closing this needs that
parser.

## 16. The builtins the stock `dm` bootstrap reaches

Running the shipped `dm.gsc` to completion at map load needs four builtins
past the precache and cvar families. Each row is what retail's function does;
the last column is what the server does today.

VERIFIED throughout: every address, table and index below, the string
literals, the numeric constants, and the pool geometry (1024 records of 124
bytes, from the `index <= 0x3ff` loop bound and the 0x7c stride). Each
"retail" cell says what the code at that address *does*, which is
disassembly reasoning, so those are marked INFERRED; nothing in this section
was measured against a running retail server.

| builtin | table | address | retail | ours |
|---|---|---|---|---|
| `placeSpawnpoint` | entity methods 37 | 0x5bedc | INFERRED: point-traces from the origin up 128 (0x5bf45), then down 262144 (0x5bf91) with contents mask 0x2810011, moves the entity to the endpoint, stores the second trace's result word at results+0x28 into `gentity_t+0x7c` (which field that is, is a further inference: the ground entity), prints `WARNING: Spawn point entity %i is in solid at (%i, %i, %i)` when a third trace at the placed position starts solid | both traces, the solid test and the move, against our own solid+playerclip mask; nothing stored for `gentity_t+0x7c`, since our trace carries no entity identity |
| `setClientNameMode` | functions 89 | 0x5f208 | INFERRED: matches the argument against `auto_change` and `manual_change` (`scr_const+0xfc`/`+0xfe`, named by `GScr_LoadConsts` 0x58550, both VERIFIED data reads), stores 0 or 1 in `level+0x210`, `Scr_Error("Unknown mode")` otherwise. The only two readers of `level+0x210` in the module are `ClientUserinfoChanged` (0x421eb) and the name-change path at 0x5ba99, which *is* a VERIFIED data read: the relocation table has exactly four `level+0x210` sites, these two and this function's own pair | recorded on the host, both errors faithful; nothing reads it until clients exist |
| `newHudElem` | functions 77 | 0x4b184 | INFERRED: first free `g_hudelems` record, zeroed but for `fontscale` 1.0 (0x4b19b) and a packed white `color` (0x4b1d9), owner `0x3ff`; `Scr_Error("out of hudelems")` when full | the allocation, the pool size and the failure; `fontscale` seeded, `color` not, since `HUD_FIELDS` has no unpacked representation for it |
| `<hudelem> setTimer` | hudelem methods 2 | 0x4b8e4 | INFERRED: exactly one parameter, seconds to milliseconds with the x87 rounding mode set to round-up (0x4b942), rejects a result not above zero, zeroes +0x30..+0x48 and +0x60/+0x64/+0x68, then writes the element type 4 at +0x0 and the absolute end time `level.time + ms` at +0x5c | the call shape and both errors; nothing recorded and nothing to clear, see below |

The record is a tagged union keyed by the type word at +0x0. `setText`
(0x4c590), `setValue` (0x4c684) and `setTimer` share one prologue that zeroes
the same block, then each writes its own type and payload: 1 with the interned
string at +0x68, 2 with the float at +0x64, 4 with the end time at +0x5c. Not
one of the offsets in that block is in `HUD_FIELDS`, so script can read none
of them back, and `label` — the script-readable field nearby, at +0x2c — is
not in it either. That is why our `setTimer` clears nothing: a script that
sets `.label` and then calls `setTimer` reads the same value back here as on
retail.

`newClientHudElem` (0x4b298) and `newTeamHudElem` (0x4b3d0) allocate from the
same pool with an owner; both need clients, so neither is implemented.

`thread addBotClients()` is commented out in the shipped `dm.gsc`, so nothing
in the stock bootstrap reaches `addTestClient`.

## Open, and worth a probe

- Whether `Scr_FindField` searches only the radiant fields. Section 7.
- Whether `Scr_AddFields` really reads `radiant/keys.txt`. Section 6.
- The name-to-index direction of the three HUD enum tables (section 3). The
  tables themselves are a data read; that index 0 is `left` rather than
  `right` rests on reading the helper at 0x4af80. A probe that writes each
  name and reads it back would settle all three in one run.
- Type 6, the `(0, f, 0)` angle form, is unused by all three server tables. It
  may exist for the client's own field tables; I have not looked at
  `cgame_mp_x86.dll` for this.
