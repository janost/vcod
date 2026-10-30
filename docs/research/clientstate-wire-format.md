# clientState_t wire format (CoD 1.1)

Sources: `cod_lnxded` (the 1.1d Linux dedicated server) and `game.mp.i386.so` (its 1.1 MP game module, full dynamic symbol table). Facts are VERIFIED from those binaries or the assets unless a line carries its own label.

## The third snapshot stream

CoD 1.1 drops Q3's per-client `CS_PLAYERS` configstring. A snapshot carries a third delta-compressed stream instead, `clientState_t`, and no configstring holds per-client data.

`SV_WriteSnapshotToClient` (`cod_lnxded` 0x808df70) writes:

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

The entity loop uses `9999` internally as its end marker, the shape of Q3's `SV_EmitPacketEntities`.

An uncompressed frame still writes the full clients block. `SV_WriteSnapshotToClient` zeroes its locals at 0x808e043 and passes a NULL from-state into every client delta (0x808e216, 0x808e22d), so each entry encodes against zero. The generic delta writer (0x807c4c8) computes `lc` as the last differing field whatever `force` says; `force` only stops an unchanged entry being omitted (the bare epilogue at 0x807c8fa against the fall-through that writes an entry).

## clientState_t layout

Netfield table at 0x080d2058 in `cod_lnxded`, 22 fields. Struct size 0x5C (92) bytes (`bzero $0x5c` at 0x807f766).

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

`MSG_WriteDeltaClient` (`cod_lnxded` 0x807f758) calls the generic delta writer with `(numFields=0x16, indexBits=6, table=0x80d2058, writeLeadingBit=1)`. Entities use `(0x3b, 10, 0x80d1760, 0)`.

`tools/re/dump_field_table.py` extracts the table; `crates/common/src/net/cod1/fields_v1.rs` came from it.

## `team`, and where a clientState comes from

The game module owns the struct. `vmMain` case 17 (`game.mp.i386.so` 0x50f18) returns `&level.clients[num] + 0x2178`, `gclient_t` stride 0x22c4. The engine's snapshot ring sits at `cod_lnxded` `ds:0x83b67c4`, stride 0x5c; `SV_WriteSnapshotToClient` reads it at 0x808e1bc and 0x808e1e6, and the fill at 0x808f404 pushes export index 0x11 and `rep movs` 0x17 dwords from the result (0x808f41c). INFERRED (control flow): the engine asks the game module for each client's `clientState_t`, which puts the struct at `gclient + 0x2178`. VERIFIED: `ClientUserinfoChanged` (0x420f4) writes the sanitized name to `gclient + 0x21b4` with a 32-byte `Q_strncpyz`, matching `name[32]` at 0x3C and pinning the base independently.

`team` is at `gclient + 0x217c`, with two writers.

1. The client script field `sessionteam` (client field table 0x72ed4, id 0xc001, "fully custom get and set"). Its setter (0x41838) compares the assigned constant string against four `scr_const` slots and stores an integer; the getter (0x418fc) maps them back. `GScr_LoadConsts` (0x58550) fills the 128 `u16` string ids in order, which names the slots.

   | `.sessionteam` | `scr_const` offset | store site | `clientState.team` |
   |---|---|---|---|
   | `none` | 0xf8 | 0x4189e | 0 |
   | `axis` | 0x08 | 0x41858 | 1 |
   | `allies` | 0x04 | 0x4186d | 2 |
   | `spectator` | 0x7c | 0x41889 | 3 |

   INFERRED (control flow, 0x418b0-0x418ce): any other string falls through to `Scr_Error`, so script cannot put a fifth value on the wire.

2. `ClientConnect` (0x4246c) bzeroes the `gclient_t` and stores 3 at 0x42516. INFERRED (control flow): a client reads `spectator` from connect until script assigns `.sessionteam`. VERIFIED: a scan of the module for `+0x217c` finds no third store.

VERIFIED live (retail 1.1d dedicated, mp_carentan, two `--net-probe --probe-team` clients with separate qports reading each other's roster):

- a client that never answers the team menu: `team` 3;
- tdm, one `allies` and one `axis`: 2 and 1, the same in both clients' snapshots;
- dm, a client that answered `allies`: `team` 0. `dm.gsc`'s `spawnPlayer` sets `.sessionteam` back to `"none"` on every spawn, so every dm player is team 0.

The client uses `team` for name colour and friend or foe (`hud/killfeed.rs` `team_color`: 1 axis, 2 allies) and the scoreboard groups on it. vcod-server's mapping is `ScriptRuntime::client_team` (`server/src/game/script.rs`), which the roster build in `server.rs` sends.

## entityState vs clientState

- `entityState_t` carries `clientNum` (8 bits, offset 144) and `index` (9 bits, offset 140; CoDExtended calls it `modelindex`).
- `ET_PLAYER` and `ET_CORPSE` take their body model and attachments from `clientState[es.clientNum]`.
- Other entities take their model from `configstring(268 + es.index)`.
- `entityState` has no attach fields; every attachment lives in `clientState`.

## Configstring map

Every base comes from a `game.mp.i386.so` write site. Each helper does `lea BASE(%ebx)` with `%ebx` the index, then `trap_SetConfigstring(index, name)` (syscall 27, 0x63668).

| Range | Contents | Evidence |
|---|---|---|
| 0 | serverinfo | |
| 1 | systeminfo | |
| 2 | `"cod"` | `SP_worldspawn` 0x61d3e |
| 3 | ambient track `n\<alias>\t\<fadems>` | `GScr_AmbientPlay` 0x5ae96 |
| 7 | delimited list of all weapon names | `BG_SetupWeaponInfo` 0x368ca |
| 8 | registered-items bitstring | `SaveRegisteredItems` 0x4efa0 |
| 21..28 | status icons | `GScr_GetStatusIconIndex` 0x5c7c8, `xor %ebx,%ebx`, `lea 0x15(%ebx)`, `cmp $0x7,%ebx` |
| 29..43 | head icons | `GScr_GetHeadIconIndex` 0x5c840, `xor %ebx,%ebx`, `lea 0x1d(%ebx)`, `cmp $0xe,%ebx` |
| 44+ | locations (not players) | `target_location_linkup` 0x646e4 / 0x6472e |
| 109..139 | tag names -> `attachTagIndex` | `G_TagIndex` 0x66fda, `lea 0x6c`, `cmp $0x20` |
| 269..523 | models -> `modelindex`, `attachModelIndex` | `G_ModelIndex` 0x66f09, `lea 0x10c`, `cmp $0xff` |
| 525..779 | sound aliases | `G_SoundAliasIndex` |
| 781..843 | effects | `G_EffectIndex` |
| 1101..1115 | shellshock | `G_ShellShockIndex` |
| 1180..1211 | script menus | `GScr_GetScriptMenuIndex` 0x5c73c, `xor %ebx,%ebx`, `lea 0x49c(%ebx)`, `cmp $0x1f,%ebx` |
| 1212+ | hint strings | `G_GetHintStringIndex` |
| 1245..1499 | localized strings | `G_LocalizedStringIndex` 0x65e30, `mov $0x1,%ebx`, `lea 0x4dc(%ebx)`, `cmp $0x100,%ebx` |
| 1501..1627 | shaders | `G_ShaderIndex` 0x65ee8, `mov $0x1,%ebx`, `lea 0x5dc(%ebx)`, `cmp $0x80,%ebx` |

`MAX_CONFIGSTRINGS` is 2048. CoDExtended `src/script.c:955` (`SV_SetConfigstring(i + 268, name)`) corroborates `CS_MODELS = 268`.

The status-icon, head-icon and script-menu indexers start their scan at `i = 0` (`xor %ebx,%ebx`); the localized-string and shader indexers at `i = 1` (`mov $0x1,%ebx`). One convention for all five puts the first three ranges a slot too high.

Configstring 44 is the locations base. No range is a per-player block.

## Entity types

From CoDExtended `shared.h:445`:

```
ET_GENERAL 0, ET_PLAYER 1, ET_CORPSE 2, ET_ITEM 3, ET_MISSILE 4,
ET_MOVER 5, ET_PORTAL 6, ET_INVISIBLE 7, ET_SCRIPTMOVER 8,
ET_UNKNOWN 9, ET_FX 10, ET_TURRET 11, ET_EVENTS 12
```

`ET_EVENTS` is 12, not Q3's 13.

VERIFIED live (two populated servers, logging `(eType, entity number, index, clientNum)` for every `eType` 2 and 3 entity): the numbering matches `shared.h`.

- `eType` 2 is `ET_CORPSE`. `index` is always 0 and `clientNum` a real connected slot (0-63) whose `clientState` resolves to that player's body and attachments (entity 65 -> clientNum 13 -> `playerbody_american_airborne` with his head, helmet and gear).
- `eType` 3 is `ET_ITEM`. `index` is a small positive number (5, 9, 19-23, 31, ...) into configstring 7's weapon list, and `clientNum` a constant 254 sentinel. Every in-range index resolved through its weapon file to a `worldModel` and drew (`kar98k_mp` -> `weapon_kAr98`, `mp40_mp` -> `weapon_MP40`, `mp44_mp` -> `weapon_mp44`, `thompson_mp` -> `weapon_thompson`, `m1carbine_mp` -> `weapon_M1Carbine`).
- One recurring index (68, against a 32 or 39 entry CS 7 list) is not a weapon. INFERRED: a non-weapon pickup (ammo, health, flag) from another registry, likely configstring 8's registered items. It falls outside the dropped-weapon path and resolves to nothing with one warning.

`crates/client/src/entities.rs` routes `ET_CORPSE = 2` and `ET_ITEM = 3` on this basis.

VERIFIED live (retail 1.1d dedicated, mp_carentan tdm, the `--save-grenade` and `--probe-grenade` captures in `crates/server/tests/fixtures/playerstate/`): `eType` 4 is `ET_MISSILE`, carried by a thrown `fraggrenade_mp`. It is the only `eType` 4 entity in either capture (one weapon, one map). `weapon_grenadelauncher_fire` builds a missile the same way and its caller is unverified (`cod11-combat.md` 11.3), so a second kind is not ruled out. A frag's fields in flight and on its detonation frame:

| Field | Value |
|---|---|
| `pos` | `TR_GRAVITY` (5): `trTime` the throw's server time, `trBase` the release point, `trDelta` the launch velocity. Each bounce rewrites all three; at rest `TR_STATIONARY` (0), zero delta. |
| `apos` | `TR_LINEAR` (2), the tumble: `trDelta` a per-throw draw near `(720, 0, 360)` deg/s, give or take 45. At rest `TR_STATIONARY`, zero delta, holding the yaw and roll reached and pitch 0. |
| `weapon` | the thrower's configstring 7 index (8 for the frag on mp_carentan) |
| `index` | 0. No model travels: `s.eFlags` 0x03000000 sits above the 24-bit netfield. |
| `eFlags` | 0 in flight, 256 on the explode frame |
| `eType` | flips to 0 on the explode frame, so an `eType == 4` filter drops the explode |

The events ride the missile's own ring: `EV_GRENADE_BOUNCE` (177) per bounce and `EV_GRENADE_EXPLODE` (178) with the downward trace's normal byte as parm, each written to `events[eventSequence & 3]` before the sequence increments. The entity stays a few frames past the explode, then leaves.

No stock MP server in the corpus sends `eType` 5, 6, 7, 9, 10 or 12, so those names are CoDExtended's and unverified.

A corpse resolves through `clientState[clientNum]` every frame, so its body lasts only while the dead client's roster entry keeps a `modelindex`. VERIFIED live (a public S&D server, corpse lifecycle log at 1 Hz): corpses stay in snapshots up to about 20 s (20150 ms three times, likely the body queue or round wipe; 1-13 s entries are PVS churn, the entity leaving and re-entering), and a dead client's roster entry can drop while his corpse is still present. Drawing corpses off the live roster blanks them once the player hits limbo or disconnects. `entities.rs` caches the last roster-resolved visual per entity for a corpse to fall back on, and seeds a fresh corpse's anim channels from the dead player's entity so the death clip keeps its phase.

## ET_MOVER and inline BSP submodels

Lump 27 (`models`, `crates/common/src/bsp.rs`) has a 48-byte stride:

| Bytes | Field |
|---|---|
| 0..24 | bounds |
| 24..32 | `first_soup` (u24), `num_soups` (u28): triangle soup range |
| 32..40 | `partitions`: a run in lump 23, the patches and terrain the model clips (`bsp-ibsp59-format.md`) |
| 40..48 | `first_brush`, `num_brushes` |

VERIFIED (dumps of mp_carentan, mp_pavlov, mp_harbor): the world model (index 0) claims soups `[0, totalSoups)` in single-model maps; in mp_harbor it claims 1492 of 1498 and submodels 2, 8, 9 split the other six contiguously. mp_carentan's world claims 2290 partitions against 1263 soups. Submodel vertices are local to the entity's `origin` key where it has one (subtracted at compile time) and in map space otherwise (no key means origin 0), so the entity transform places them either way. Most submodels are `trigger_*` volumes with brushes and no surfaces; every submodel with surfaces belongs to a `script_brushmodel`. Surface-bearing submodels per stock map: mp_stanjel 57, mp_powcamp 5, mp_harbor and mp_ship 3, several others 1-2, eight maps 0.

VERIFIED (gamestate scans on the local 1.1d server for mp_harbor, mp_powcamp, mp_ship, mp_depot, and three populated live servers): no configstring starts with `*`, the Q3/RTCW inline-model convention. A per-entity `eType` histogram on the two busy servers showed no `ET_MOVER` (5), and all 26 `ET_SCRIPTMOVER` (8) entities resolved through xmodel names. The retail MP game module never puts an inline model in `CS_MODELS`. A `script_brushmodel` arrives as `eType` 8 with `solid` 0xffffff and its inline model number in `index` (`cod11-movers.md` section 14), which `entities.rs` draws as `EntityVisual::Submodel`. `resolve_visual` keeps a `*N` configstring arm for a mod that registers one.

## Where each fact came from

| Source | What it gave |
|---|---|
| CoDExtended `src/shared.h` | reverse-engineered `entityState_t`/`playerState_t`, entity type enum |
| `cod_lnxded` (1.1d Linux dedicated server) | netfield tables, `SV_WriteSnapshotToClient` |
| `game.mp.i386.so` (1.1 MP game module, full dynamic symbol table) | configstring write sites |
| RTCW-MP `src/game/bg_animation.c` and neighbours | direct ancestor of CoD1's anim system |
| the game's `pak*.pk3` files | assets |
