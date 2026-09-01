# clientState_t wire format (CoD 1.1)

Research notes from disassembly of `cod_lnxded` (the 1.1d Linux dedicated server) and `game.mp.i386.so` (its 1.1 MP game module, full dynamic symbol table). Everything here is confirmed from binaries or assets unless marked otherwise.

## The third snapshot stream

CoD 1.1 replaced Q3's per-client `CS_PLAYERS` configstring with a third delta-compressed stream in the snapshot: `clientState_t`. There is no per-client configstring in CoD 1.1.

`SV_WriteSnapshotToClient` (`cod_lnxded` vaddr `0x808df70`) writes:

```
byte  svc_snapshot (7)         0x808df7f
long  serverTime               0x808df97
byte  deltaNum                 0x808dfa7
byte  snapFlags                0x808dfee
<playerState delta>            0x808e006 / 0x808e03e
<packet entities>              0x808e0fa / 0x808e11c / 0x808e133
bits(1023, 10)                 0x808e15e   entity terminator
<clients>: repeated
  [1 bit][6-bit clientIndex][clientState delta]
                               0x808e1fd / 0x808e216 / 0x808e22d
bit 0                          0x808e24f   client terminator
```

The entity sentinel logic uses `9999` internally as the "no more" marker, same shape as Q3's `SV_EmitPacketEntities`.

On an uncompressed frame retail still writes the full clients block: `SV_WriteSnapshotToClient` (`cod_lnxded`) zeroes its locals at `0x808e043` and passes NULL from-state into every client delta (`0x808e216`, `0x808e22d`), so each entry encodes against zero. In the generic delta writer at `0x807c4c8`, `lc` is computed as the last differing field regardless of `force`; `force` only suppresses entry omission when nothing changed (bare epilogue at `0x807c8fa` versus the fall-through that writes an entry).

## clientState_t layout

Netfield table at vaddr `0x080d2058` in `cod_lnxded`, 22 fields. Struct size `0x5C` = 92 bytes (confirmed: `bzero $0x5c` at `0x807f766`).

```c
typedef struct clientState_s {   // 92 bytes
    int  clientIndex;            // 0x00  not a netfield; sent as the 6-bit index
    int  team;                   // 0x04  2 bits
    int  modelindex;             // 0x08  8 bits  -> configstring 268 + modelindex
    int  attachModelIndex[6];    // 0x0C  8 bits each -> configstring 268 + idx
    int  attachTagIndex[6];      // 0x24  5 bits each -> configstring 108 + idx
    char name[32];               // 0x3C  8 x 32-bit netfields name[0..28]
} clientState_t;
```

`MSG_WriteDeltaClient` (`cod_lnxded 0x807f758`) calls the generic delta writer with `(numFields=0x16, indexBits=6, table=0x80d2058, writeLeadingBit=1)`. For comparison, entities use `(0x3b, 10, 0x80d1760, 0)`.

`tools/re/dump_field_table.py` extracts the table from the binary; `crates/common/src/net/fields_v1.rs` was generated the same way.

## `team`, and where a clientState comes from

The game module owns the whole struct. `vmMain` case 17
(`game.mp.i386.so 0x50f18`) returns `&level.clients[num] + 0x2178`, with
`gclient_t` at stride `0x22c4`. The engine's snapshot ring is at
`cod_lnxded ds:0x83b67c4`, stride `0x5c`, and `SV_WriteSnapshotToClient`
reads its entries at `0x808e1bc` and `0x808e1e6`; the fill at `0x808f404`
pushes export index `0x11` and `rep movs` `0x17` dwords from what comes
back (`0x808f41c`). INFERRED (control flow): that is the engine asking the
game module for a client's `clientState_t`, which places the struct at
`gclient + 0x2178`. `ClientUserinfoChanged` (`0x420f4`) writing the
sanitized name into `gclient + 0x21b4` with a 32-byte `Q_strncpyz` matches
`name[32]` at `0x3C` of that struct and pins the base independently.

`team` sits at `gclient + 0x217c`. Two places write it. The client script
field `sessionteam` (client field table `0x72ed4`, id `0xc001`, marked
"fully custom get and set") has a setter at `0x41838` that compares the
assigned constant string against four `scr_const` slots and stores an
integer; the getter at `0x418fc` maps the same four back. `scr_const`'s
slot names come from `GScr_LoadConsts` (`0x58550`), which fills the 128
`u16` string ids in order.

| `.sessionteam` | `scr_const` offset | store site | `clientState.team` |
|---|---|---|---|
| `none` | `0xf8` | `0x4189e` | 0 |
| `axis` | `0x08` | `0x41858` | 1 |
| `allies` | `0x04` | `0x4186d` | 2 |
| `spectator` | `0x7c` | `0x41889` | 3 |

INFERRED (control flow, `0x418b0`-`0x418ce`): any other string falls through
to `Scr_Error`, so those four are the only assignable values and script
cannot put a fifth number on the wire.

The other writer is `ClientConnect` (`0x4246c`), which bzeroes the
`gclient_t` and then stores 3 at `0x42516`. INFERRED (control flow): a
client therefore reads as `spectator` from the moment it connects until
script assigns `.sessionteam`. A scan of the module for `+0x217c` finds no
third store.

VERIFIED live (local retail 1.1d dedicated, mp_carentan, 2026-09-01, two
`--net-probe --probe-team` clients with separate qports reading each other's
roster):

- a client that never answers the team menu: `team` 3;
- tdm, one client answering `allies` and one `axis`: 2 and 1, and both
  clients' snapshots agree on the pair;
- dm, a client that answered `allies`: `team` 0, because `dm.gsc`'s
  `spawnPlayer` sets `.sessionteam` back to `"none"` on every spawn. dm puts
  every player on team 0 whatever the menu answered.

The client uses this for name colour and friend/foe: `killfeed::team_color`
already colours 1 axis and 2 allies, and the scoreboard groups on it.
`ScriptRuntime::client_team` is vcod's mapping and the roster build in
`crates/server/src/server.rs` sends it.

## entityState vs clientState

- `entityState_t` carries `clientNum` (8 bits, offset 144) and `index` (9 bits, offset 140; CoDExtended calls it `modelindex`).
- For `ET_PLAYER`/`ET_CORPSE`, the visual (body model + attachments) comes from `clientState[es.clientNum]`.
- For non-player entities, the model is `configstring(268 + es.index)`.
- `entityState` has no attach fields; all attachments live in `clientState`.

## Configstring map

All bases confirmed from `game.mp.i386.so` write sites. Each helper does `lea BASE(%ebx)` with `%ebx` the 1-based index, then `trap_SetConfigstring(index, name)` (syscall 27, verified at `0x63668`).

| Range | Contents | Evidence |
|---|---|---|
| 0 | serverinfo | |
| 1 | systeminfo | |
| 2 | `"cod"` | `SP_worldspawn 0x61d3e` |
| 3 | ambient track `n\<alias>\t\<fadems>` | `GScr_AmbientPlay 0x5ae96` |
| 7 | delimited list of all weapon names | `BG_SetupWeaponInfo 0x368ca` |
| 8 | registered-items bitstring | `SaveRegisteredItems 0x4efa0` |
| 21..28 | status icons | `GScr_GetStatusIconIndex 0x5c7c8`, `xor %ebx,%ebx`, `lea 0x15(%ebx)`, `cmp $0x7,%ebx` |
| 29..43 | head icons | `GScr_GetHeadIconIndex 0x5c840`, `xor %ebx,%ebx`, `lea 0x1d(%ebx)`, `cmp $0xe,%ebx` |
| 44+ | locations (NOT players) | `target_location_linkup 0x646e4/0x6472e` |
| 109..139 | tag names -> `attachTagIndex` | `G_TagIndex 0x66fda`, `lea 0x6c`, `cmp $0x20` |
| 269..523 | models -> `modelindex`, `attachModelIndex` | `G_ModelIndex 0x66f09`, `lea 0x10c`, `cmp $0xff` |
| 525..779 | sound aliases | `G_SoundAliasIndex` |
| 781..843 | effects | `G_EffectIndex` |
| 1101..1115 | shellshock | `G_ShellShockIndex` |
| 1180..1211 | script menus | `GScr_GetScriptMenuIndex 0x5c73c`, `xor %ebx,%ebx`, `lea 0x49c(%ebx)`, `cmp $0x1f,%ebx` |
| 1212+ | hint strings | `G_GetHintStringIndex` |
| 1245..1499 | localized strings | `G_LocalizedStringIndex 0x65e30`, `mov $0x1,%ebx`, `lea 0x4dc(%ebx)`, `cmp $0x100,%ebx` |
| 1501..1627 | shaders | `G_ShaderIndex 0x65ee8`, `mov $0x1,%ebx`, `lea 0x5dc(%ebx)`, `cmp $0x80,%ebx` |

`MAX_CONFIGSTRINGS = 2048`. `CS_MODELS = 268` independently corroborated by CoDExtended `src/script.c:955` (`SV_SetConfigstring(i + 268, name)`).

The status-icon, head-icon and script-menu indexers start their scan loop at
`i = 0` (`xor %ebx,%ebx`); the localized-string and shader indexers start at
`i = 1` (`mov $0x1,%ebx`). Applying one convention to all five previously
put the first three ranges one slot too high.

Configstring 44 is the locations base. Nothing in the table is a per-player block.

## Entity types

From CoDExtended `shared.h:445`:

```
ET_GENERAL 0, ET_PLAYER 1, ET_CORPSE 2, ET_ITEM 3, ET_MISSILE 4,
ET_MOVER 5, ET_PORTAL 6, ET_INVISIBLE 7, ET_SCRIPTMOVER 8,
ET_UNKNOWN 9, ET_FX 10, ET_TURRET 11, ET_EVENTS 12
```

`ET_EVENTS` is 12, not Q3's 13.

Whether CoD1 MP actually spawns `ET_CORPSE` entities is unconfirmed; in stock gametype scripts the death animation plays on the player entity itself and `_teams::model()` restores the model on respawn.

VERIFIED live (two populated servers, 2026-08-24): `eType == 2` is `ET_CORPSE` and `eType == 3` is `ET_ITEM`, matching CoDExtended's `shared.h`. Earlier I had guessed from spectating alone that eType 2 looked item-like; the wire shows otherwise. I logged `(eType, entity number, index, clientNum)` for every eType-2/3 entity on both servers:

- `eType == 2`: `index` is always 0, `clientNum` is a real, distinct connected player slot (0-63) whose `clientState` resolves to that player's actual body + attachments (e.g. entity 65 -> clientNum 13 -> `playerbody_american_airborne` with that player's head/helmet/gear). This is `ET_CORPSE`: a dead player's corpse entity keyed by which client died.
- `eType == 3`: `index` is a small positive number (5, 9, 19-23, 31, ...) that indexes configstring 7's weapon list, `clientNum` is a constant 254, not a valid client slot, so a sentinel. Every in-range index resolved through the weapon file to a real `worldModel` and drew (`kar98k_mp` -> `weapon_kAr98`, `mp40_mp` -> `weapon_MP40`, `mp44_mp` -> `weapon_mp44`, `thompson_mp` -> `weapon_thompson`, `m1carbine_mp` -> `weapon_M1Carbine`, all live-verified). This is `ET_ITEM`.
- One recurring out-of-range index (68, against a 32/39-entry CS7 list) is not a weapon. Inferred: a non-weapon pickup (ammo/health/flag) indexed through a different registry, with configstring 8's registered-items bitstring the likely candidate. It is outside the dropped-weapon world-model path and resolves to nothing with one warning.

`crates/client/src/entities.rs` routes `ET_CORPSE = 2` and `ET_ITEM = 3` on this basis.

A corpse resolves through `clientState[clientNum]` every frame, so its body lives only as long as the dead client's roster entry keeps a `modelindex`. VERIFIED live (S&D server 20.203.98.136:28960, 2026-08-26, probe corpse-lifecycle log, 1 Hz sampling): corpses stay in snapshots for up to ~20 s (observed 20150 ms three times, likely the body-queue/round wipe; shorter 1-13 s entries are PVS churn, the same entity number leaves and re-enters the spectator's snapshot), and a dead client's roster entry can drop entirely (`clientState` removed) while its corpse is still present. Rendering a corpse through the live roster therefore blanks it the moment the dead player hits limbo or disconnects, the "bodies vanish after a second" symptom. vcod caches the last roster-resolved visual per entity and lets a corpse fall back to it, and seeds a fresh corpse entity's anim channels from the dead player's entity (`entities.rs`) so the death clip keeps its phase instead of replaying.

## ET_MOVER and inline BSP submodels

`crates/common/src/bsp.rs` lump 27 (`models`) is a 48-byte-stride array; besides
the bounds (0..24) and brush range (40..48) it carries a triangle-soup range at
bytes 24..32: `firstTriangleSoup` (u24) / `numTriangleSoups` (u28). Confirmed
by dumping mp_carentan, mp_pavlov and mp_harbor. The world model (index 0)
always claims soups `[0, totalSoups)` in single-model maps, and in mp_harbor
the world claims 1492 of 1498 while submodels 2, 8, 9 partition the remaining
six contiguously. Bytes 32..40 (u32/u36) are a second, larger range, left
unparsed; it looks like a collision-AABB range, since mp_carentan's world
claims 2290 of them against 1263 soups. Submodel vertices are stored local to
the entity's `origin` key where one exists (subtracted at compile time), and in
map space when it doesn't (no `origin` key means "origin 0", so local space and
map space coincide). Either way the live entity transform places them
correctly with no extra re-centering. Most submodels are `trigger_*` volumes
with brushes and no surfaces; every submodel that does carry surfaces belongs
to a `script_brushmodel` entity. Surface-bearing submodel counts across stock
maps: mp_stanjel 57, mp_powcamp 5, mp_harbor/mp_ship 3, several others 1-2,
eight maps 0.

In every capture so far, no CoD 1.1 multiplayer server sent a drawable one.
`resolve_visual` only produces `EntityVisual::Submodel(n)` when an entity's
model configstring starts with `*` (the Q3/RTCW convention for "inline BSP
model n"). Every configstring in the gamestate (available before any client
joins a team, so this doesn't need a live snapshot) was scanned for a leading
`*` on the local 1.1d dedicated server across four stock maps with known
drawable submodels (mp_harbor, mp_powcamp, mp_ship, mp_depot) and on three
live populated servers: zero hits every time. Cross-checked from the other
direction on the two busy live servers with a per-entity `eType` histogram:
zero `ET_MOVER` (5) ever appeared, and all `ET_SCRIPTMOVER` (8) entities
(26 seen) resolved through ordinary xmodel names, never `*N`. The retail MP
game module never registers an inline brushmodel in `CS_MODELS`. The draw
path stays in `entities.rs` and `bsp.rs` because the lump carries usable
data, but only unit tests exercise it; no real server traffic seen so far
does. A mod or singleplayer map that does register `*N` would be the first
real test of it.

## Where each fact came from

| Source | What it gave |
|---|---|
| CoDExtended `src/shared.h` | reverse-engineered `entityState_t`/`playerState_t`, entity type enum |
| `cod_lnxded` (1.1d Linux dedicated server) | netfield tables, `SV_WriteSnapshotToClient` |
| `game.mp.i386.so` (1.1 MP game module, full dynamic symbol table) | configstring write sites |
| RTCW-MP `src/game/bg_animation.c` and neighbours | direct ancestor of CoD1's anim system |
| the game's `pak*.pk3` files | assets |
