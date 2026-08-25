# CoD 1.1 event table and effect mapping

Everything below is read out of CoD 1.1 binaries and stock assets. Where a claim
could not be settled from the binaries it is marked `UNVERIFIED` with the live
check that would settle it.

## Read this first: the root `cgamex86.dll` is the wrong module

CoD 1.x ships two client-game modules:

| Path (relative to the 1.1 install) | Module | `EV_*` table |
|---|---|---|
| `cgamex86.dll` | single-player cgame | 198 entries, `ET_EVENTS = 17` |
| `main/cgame_mp_x86.dll` | multiplayer cgame | 202 entries, `ET_EVENTS = 12` |

The two enums agree on ids 0 to 172 and diverge from 173 up: SP has
`EV_SOUND_ALIAS_NOTIFY` at 173 and `EV_GRENADE_SUICIDE` at 197; MP has neither
and instead appends `EV_DROPWEAPON`, `EV_ITEM_RESPAWN`, `EV_ITEM_POP`,
`EV_PLAYER_TELEPORT_IN/OUT`, `EV_OBITUARY` at 196 to 201. Every impact and
explosion event sits above 172, so using the SP table would put every combat
event off by one.

vcod is a multiplayer spectator client, so `main/cgame_mp_x86.dll` is the
authority and this document uses it. The 202-entry table is confirmed
byte-for-byte identical in all four MP modules plus the Linux dedicated server's
game module, which is what puts the numbers on the wire.

### Evidence sources

| File | md5 | Used for |
|---|---|---|
| `cgame_mp_x86.dll` (1.1) | `4912169a9eb22b404f95c52863a5feb6` | event dispatch, effect/tracer/muzzle-flash selection |
| `game_mp_x86.dll` (1.1) | `25e2fcfe02ca0c46f4e9ad2530d50691` | EV table cross-check |
| `CoDMP.exe` (1.1) | `753fbcabd0fdda7f7dad3dbb29c3c008` | `entityState_t` / `playerState_t` netfield tables (client side) |
| `game.mp.i386.so` (1.1d Linux dedicated server) | `de8947beb6f86fbfb46f5adfaab3d3ed` | who writes which field (full symbols), `bytedirs`, `ByteToDir`/`DirToByte` |
| `pak5.pk3 : fx/iw_impacts.csv` | none | the (impact type, surface) to `.efx` table |
| `cgame_mp_x86.dll` (1.5) | `075e2af18aeaf2aeaf2a75ce22db683a` | 1.5 diff |
| `cgamex86.dll` (1.1) | `84c8eba1be0297361b34318557186268` | SP module, only to establish it is the wrong one |

Addresses are virtual, image base `0x30000000` for the cgame DLLs,
`0x20000000` for the game DLLs, `0x00400000` for `CoDMP.exe`, file-relative for
the Linux `.so`. Decompilation is Ghidra 12 headless (`analyzeHeadless`);
`tools/re/ExportDecomp.java` dumps every function with a name and address
marker.

The two entry points everything hangs off:

- `CG_EntityEvent` @ `0x3001dc10` prints `"CG_EntityEvent:%s\n"` with
  `eventnames[event]`, then a range ladder plus a jump-table switch on the event id.
- `CG_EntityPreEvent` @ `0x3001e820` prints
  `"ent:%3i  preevent:%3i CG_EntityPreEvent:%s\n"`, switch covering ids 159 to 195
  (`lea eax,[edi-0x9f]; cmp eax,0x24`).
  All bullet-impact handling lives here, not in `CG_EntityEvent`.

---

## 1. The CoD 1.1 multiplayer `EV_*` enum

`eventnames[]` is a 202-entry array of `char*` at VA `0x30077040`
(`cgame_mp_x86.dll`), reached through a pointer at `0x30077368`. It is indexed
by the event id: both `CG_EntityEvent` and `CG_EntityPreEvent` print
`eventnames[event]`, and the switch case numbers line up with it exactly (spot
checks: case `0x8f` is `EV_STEP_VIEW` and subtracts `0x80` from `eventParm` to
get a view step; cases `0x92`/`0x94` are `EV_ITEM_PICKUP`/`EV_AMMO_PICKUP` and
index the weapon table by `eventParm`; case `0xc9` is `EV_OBITUARY`). The same
202 names in the same order appear in `game_mp_x86.dll` at `0x2006c8b8` and in
the Linux dedicated server's `game.mp.i386.so` as the exported symbol
`eventnames` at `0x0007b6a8`.

id 0 is `EV_NONE`.

ids 1 to 138 are six groups of 23, one entry per surface type, in the surface
order of section 4 (`default, bark, brick, carpet, cloth, concrete, dirt,
flesh, foliage, glass, grass, gravel, ice, metal, mud, paper, plaster, rock,
sand, snow, water, wood, asphalt`):

| Base id | Group | Range |
|---|---|---|
| 1 | `EV_FOOTSTEP_RUN_<SURF>` | 1-23 |
| 24 | `EV_FOOTSTEP_WALK_<SURF>` | 24-46 |
| 47 | `EV_FOOTSTEP_PRONE_<SURF>` | 47-69 |
| 70 | `EV_JUMP_<SURF>` | 70-92 |
| 93 | `EV_LANDING_<SURF>` | 93-115 |
| 116 | `EV_LANDING_PAIN_<SURF>` | 116-138 |

So `EV_FOOTSTEP_RUN_DEFAULT = 1`, `EV_FOOTSTEP_RUN_ASPHALT = 23`,
`EV_JUMP_DEFAULT = 70`, `EV_LANDING_PAIN_ASPHALT = 138`. `CG_EntityEvent`'s
range ladder uses exactly these boundaries (`< 0x18`, `< 0x2f`, `< 0x46`,
`< 0x5d`, `< 0x74`, `< 0x8b`), which is independent confirmation of the group
sizes.

ids 139 to 201 in full:

| id | name | id | name | id | name |
|---|---|---|---|---|---|
| 139 | `EV_FOLIAGE_SOUND` | 140 | `EV_STANCE_FORCE_STAND` | 141 | `EV_STANCE_FORCE_CROUCH` |
| 142 | `EV_STANCE_FORCE_PRONE` | 143 | `EV_STEP_VIEW` | 144 | `EV_WATER_TOUCH` |
| 145 | `EV_WATER_LEAVE` | 146 | `EV_ITEM_PICKUP` | 147 | `EV_ITEM_PICKUP_QUIET` |
| 148 | `EV_AMMO_PICKUP` | 149 | `EV_NOAMMO` | 150 | `EV_EMPTYCLIP` |
| 151 | `EV_RELOAD` | 152 | `EV_RELOAD_FROM_EMPTY` | 153 | `EV_RELOAD_START` |
| 154 | `EV_RELOAD_END` | 155 | `EV_RAISE_WEAPON` | 156 | `EV_PUTAWAY_WEAPON` |
| 157 | `EV_WEAPON_ALT` | 158 | `EV_PULLBACK_WEAPON` | 159 | `EV_FIRE_WEAPON` |
| 160 | `EV_FIRE_WEAPONB` | 161 | `EV_FIRE_WEAPON_LASTSHOT` | 162 | `EV_RECHAMBER_WEAPON` |
| 163 | `EV_EJECT_BRASS` | 164 | `EV_MELEE_SWIPE` | 165 | `EV_FIRE_MELEE` |
| 166 | `EV_MELEE_HIT` | 167 | `EV_MELEE_MISS` | 168 | `EV_FIRE_WEAPON_MG42` |
| 169 | `EV_FIRE_QUADBARREL_1` | 170 | `EV_FIRE_QUADBARREL_2` | 171 | `EV_BULLET_TRACER` |
| 172 | `EV_SOUND_ALIAS` | 173 | `EV_BULLET_HIT_SMALL` | 174 | `EV_BULLET_HIT_LARGE` |
| 175 | `EV_BULLET_HIT_CLIENT_SMALL` | 176 | `EV_BULLET_HIT_CLIENT_LARGE` | 177 | `EV_GRENADE_BOUNCE` |
| 178 | `EV_GRENADE_EXPLODE` | 179 | `EV_ROCKET_EXPLODE` | 180 | `EV_ROCKET_EXPLODE_NOMARKS` |
| 181 | `EV_MOLOTOV_EXPLODE` | 182 | `EV_MOLOTOV_EXPLODE_NOMARKS` | 183 | `EV_CUSTOM_EXPLODE` |
| 184 | `EV_CUSTOM_EXPLODE_NOMARKS` | 185 | `EV_RAILTRAIL` | 186 | `EV_BULLET` |
| 187 | `EV_PAIN` | 188 | `EV_CROUCH_PAIN` | 189 | `EV_DEATH` |
| 190 | `EV_DEBUG_LINE` | 191 | `EV_PLAY_FX` | 192 | `EV_PLAY_FX_DIR` |
| 193 | `EV_PLAY_FX_ON_TAG` | 194 | `EV_FLAMEBARREL_BOUNCE` | 195 | `EV_EARTHQUAKE` |
| 196 | `EV_DROPWEAPON` | 197 | `EV_ITEM_RESPAWN` | 198 | `EV_ITEM_POP` |
| 199 | `EV_PLAYER_TELEPORT_IN` | 200 | `EV_PLAYER_TELEPORT_OUT` | 201 | `EV_OBITUARY` |

The switch covers ids 139 to 201 (`lea eax,[edi-0x8b]; cmp eax,0x3e`). Six of
them land on the `"Unknown event: '%s'"` warning at `0x3001e703`, i.e. the retail
MP client does nothing with them (walked the jump table at `0x3001e72c` through
the index byte table at `0x3001e7dc`): `EV_BULLET_TRACER` (171),
`EV_MOLOTOV_EXPLODE` (181), `EV_MOLOTOV_EXPLODE_NOMARKS` (182),
`EV_CUSTOM_EXPLODE` (183), `EV_CUSTOM_EXPLODE_NOMARKS` (184), `EV_BULLET` (186).
Another ten reach a bare `break`, i.e. handled only in `CG_EntityPreEvent` or
not at all: 150, 165, 167, 173, 174, 175, 176, 187, 188, 195.

### How an event id reaches the client

Two paths, both in `cgame_mp_x86.dll`:

- Event entities. `CG_CheckEvents` @ `0x3001ea10`:
  `if (es.eType > 12) { event = es.eType - 12; }`. So `ET_EVENTS = 12` and the
  event rides in `es.eType`, with `es.eventParm` used directly. In the SP
  module the same constant is 17; do not copy it from there.
- Ordinary entities. A 4-deep ring: `es.eventSequence` (offset 164) counts
  up, `es.events[i & 3]` (168 to 180) holds the id, `es.eventParms[i & 3]`
  (184 to 196) the parm. The cgame replays at most the last 4
  (`if (seq - lastSeq > 4) lastSeq = seq - 4`) and copies `eventParms[i]` into
  `es.eventParm` before dispatching. Server side this is `G_AddEvent`
  @ `game.mp.i386.so 0x67ca4`, which writes `ent->s.eventSequence` at `+0xa4`,
  `events[]` at `+0xa8`, `eventParms[]` at `+0xb8`, matching the netfield
  offsets exactly.
- Player-state events use the same ring inside `playerState_t`:
  `eventSequence` 132, `events[4]` 136/140/144/148, `eventParms[4]`
  152/156/160/164 (`G_AddEvent`'s `ent->client != NULL` branch).
- `CG_CheckPreEvents` @ `0x3001eb10` runs the identical ring over
  `cent->nextState` (`centity_t + 0xf0`) instead of `currentState`, one
  frame early. That is why impacts are handled in `CG_EntityPreEvent`.

`centity_t` is 0x228 (552) bytes, array base `0x3020db98 - 0x20`;
`currentState` at +0, `nextState` at +0xf0, `lerpOrigin` at +0x1f8.
`sizeof(entityState_t) == 0xf0` (240), confirmed by the `imul reg,reg,0xf0`
sites in `CoDMP.exe`.

---

## 2. Which state fields each combat-relevant event populates

`entityState_t` offsets are the netfield offsets. The client-side table in
`CoDMP.exe` @ `0x005419d0` (59 entries) is identical in order, offset and bit
width to the one `crates/common/src/net/fields_v1.rs` was generated from, so
vcod's decoded field names apply directly. The ones that matter here:

| Offset | Field | Bits |
|---|---|---|
| 0 | `number` (not networked) | none |
| 4 | `eType` | 8 |
| 24/28/32 | `pos.trBase` (the origin) | float |
| 92/96/100 | `origin2` | float |
| 116 | `otherEntityNum` | 10 |
| 136 | `surfType` | 8 |
| 144 | `clientNum` | 8 |
| 160 | `eventParm` | 8 |
| 200 | `weapon` | 6 |
| 216 | `scale` (a union slot, see below) | 8 |
| 84 | `time` | 32 |
| 104/108 | `angles2[0]`, `angles2[1]` | float |

Offset 216 is named `_union.scale` in the `cod_lnxded` table and `scale` in
`CoDMP.exe`. For impact and explosion events it carries a second direction
byte, not a scale. vcod's parser already decodes it under the name `scale`.

Per event, source (`game.mp.i386.so`) and consumer (`cgame_mp_x86.dll`):

### `EV_BULLET_HIT_SMALL` (173) / `EV_BULLET_HIT_LARGE` (174)

Emitted as a temp entity (`eType = ET_EVENTS + event`) by two sites, identical
in shape:

- hitscan: `Bullet_Fire_Extended` @ `0x68a97` to `0x68b05`
- missile: `G_RunMissile` @ `0x54112` to `0x54165`

```
te = G_TempEntity(impactPoint, EV_BULLET_HIT_SMALL);   // LARGE if weapon->[0x2c0]
te->s.eventParm      (160) = DirToByte(surfaceNormal);
te->s.scale          (216) = DirToByte(reflectedDir);
te->s.surfType       (136) = (trace.surfaceFlags >> 20) & 0x1f;
te->s.otherEntityNum (116) = attacker/owner entity number;
```

`pos.trBase` is the impact point (written by `G_TempEntity`). `weapon` is not
set on the temp entity and the client never reads it for these events. The
small/large choice is made server-side from a per-weapon flag, so the client
does not need `weapon` either.

Consumer, `CG_EntityPreEvent` case 173/174 @ `0x3001e890`:

```
ByteToDir(es.eventParm, dirA);     // surface normal
ByteToDir(es.scale,     dirB);     // reflected
CG_BulletHitWall(event=eax, pos=cent->lerpOrigin, surfType=ebx=es.surfType,
                 attacker=es.otherEntityNum, dirA, dirB);   // 0x30039640
```

A flesh hit reaches everyone else as one of these with `surfType == 7`: the
player-hit path (`game.mp.i386.so 0x43ab5` to `0x43af1`) hardcodes
`te->s.surfType = 7` and sets the gentity's `svFlags` byte at `+0xf5 |= 0x20`
with `r.singleClient = victim` (send-to-all-but-victim).

### `EV_BULLET_HIT_CLIENT_SMALL` (175) / `EV_BULLET_HIT_CLIENT_LARGE` (176)

`game.mp.i386.so 0x43b1c` to `0x43b78`:

```
te->s.surfType       (136) = 7;                   // always flesh
te->s.otherEntityNum (116) = attacker entity number;
te->s.clientNum      (144) = victim's clientNum;
te->r.svFlags        (244) = 0x800;               // single-client delivery
te->r.singleClient   (248) = victim's clientNum;
```

No direction bytes. Consumer, `CG_EntityPreEvent` case 175/176 @ `0x3001e8d9`:
`CG_BulletHitFlesh(pos=cent->lerpOrigin, surfType=es.surfType, attacker=es.otherEntityNum)`
@ `0x300396f0`. It spawns no effect at all, only the per-surface sound alias
(same tables as the wall path) and the tracer/whiz-by path of section 6.

VERIFIED live (2026-08-24, 51.195.89.86:28960, 100s capture during active
combat): 175/176 never arrived while 173 with `surfType == 7` appeared 38
times. The single-client delivery reading is confirmed; vcod can ignore
175/176 entirely. Flesh hits reach spectators as ordinary 173/174 events
with the flesh surfType.

### `EV_GRENADE_BOUNCE` (177), `EV_GRENADE_EXPLODE` (178), `EV_ROCKET_EXPLODE` (179) / `_NOMARKS` (180)

`CG_EntityEvent` @ `0x3001dc10`, cases `0xb1`, `0xb2`, `0xb3`/`0xb4`:

```
ByteToDir(es.eventParm, dir);                             // 0x3001e491, 0x3001e4ec
CG_PlayQueuedSound(ENTITYNUM_WORLD=1022, cent->lerpOrigin);   // alias table[es.surfType]
Effect(table[es.surfType], cent->lerpOrigin, dir);        // syscall 0xe4
// explode also: Effect(weapon[es.weapon].projExplosionEffect, ...) and its sound
```

Only one direction byte here, `es.eventParm`, the surface normal. These read
`pos.trBase`/`lerpOrigin`, `eventParm` (160), `surfType` (136) and, for the
explode variants, `weapon` (200). The per-surface sound alias tables are
`0x301d5d7c` (bounce) and `0x301d5dd8` (explode), also indexed by `surfType`.
`_NOMARKS` differs only by setting the global no-marks flag at `0x3020c9c0`,
which suppresses the decal.

### `EV_FIRE_WEAPON` (159) / `EV_FIRE_WEAPONB` (160) / `EV_FIRE_WEAPON_LASTSHOT` (161) / `EV_FIRE_WEAPON_MG42` (168) / `EV_FIRE_QUADBARREL_1,2` (169/170)

`CG_FireWeapon` @ `0x30038b70` reads `es.weapon` (200), `es.eType` (4) and
`es.number` (0). Position for a turret flash is `cent->lerpOrigin`; for a player
the flash is placed on the model's `tag_flash` bone in `CG_AddPlayerWeapon`
@ `0x30036cf0`. `EV_FIRE_WEAPON_MG42` additionally shakes the view with fixed
constants (`0.05f, 100, 100.0f`). The quadbarrel variants call `CG_FireWeapon`
twice with barrel indices 0/1 and 2/3.

### `EV_PLAY_FX` (191) / `EV_PLAY_FX_DIR` (192) / `EV_PLAY_FX_ON_TAG` (193)

`CG_PlayFx` @ `0x3001db20`: `effectId = es.eventParm` (must be `1..63`),
position `cent->lerpOrigin`. `EV_PLAY_FX_DIR` first does
`ByteToDir(es.scale (216), dir)` (`0x3001e6ce`) and passes it. `EV_PLAY_FX_ON_TAG`
(`0x3001db80`) resolves the tag through configstring `844 + es.eventParm`.
Persistent `ET_FX` entities use the same effect array but take the id from
`es.scale` (216) and the direction from `es.origin2` (92 to 100).

### Others worth knowing

| Event | Fields read |
|---|---|
| `EV_SOUND_ALIAS` (172) | `es.eventParm` -> configstring `524 + parm`; position `es.pos.trBase` (`cent+0x18`) |
| `EV_STEP_VIEW` (143) | `es.eventParm - 128` is the signed view-step in units; `es.clientNum` must equal `cg.clientNum` |
| `EV_ITEM_PICKUP` (146) / `EV_AMMO_PICKUP` (148) | `es.eventParm` is the weapon index (1..69) |
| reload / raise / putaway (151 to 158, 162) | `es.weapon` indexes the weapon table (stride 0x198) |
| `EV_MELEE_HIT` (166) | `es.otherEntityNum` (116) is the sound emitter |
| `EV_EARTHQUAKE` (195) | `es.angles2[0]` = scale, `es.time` = duration ms, `es.angles2[1]` = radius (`CG_EntityPreEvent` case `0xc3`) |
| `EV_RAILTRAIL` (185) | `es.pos.trBase` = start, `es.origin2` = end, `es.attackerEntityNum` (120) = shooter, `es.dmgFlags` (220) = 2. Debug only (`g_debugBullets`). |
| `EV_OBITUARY` (201) | handler `0x3001d6c0`; field usage not decoded, out of scope for effects |

Events 140 to 143 (`EV_STANCE_FORCE_*`, `EV_STEP_VIEW`) and 149 are single-client:
the cgame prints
`"Event %s just for client %i was sent to other clients"` when
`es.clientNum != cg.clientNum`.

---

## 3. Direction / normal encoding on impact events

Directions are a byte index into a 162-entry unit-vector table, the classic
Quake `bytedirs`.

- Server: `DirToByte` @ `game.mp.i386.so 0x3d5ec` loops `dl` over `0..0xa1`
  (0 to 161) taking the best dot product; `ByteToDir` @ `0x3d66c` looks up
  `bytedirs[b]` (12-byte stride) and falls back to entry 0 when `b > 161`.
  Table `bytedirs` at `0x7d540`.
- Client: `ByteToDir` @ `cgame_mp_x86.dll 0x300399b0`, table at `0x300740d0`.
  Out of range (`b < 0 || b >= 162`) yields `(0,0,0)`, not entry 0.

The client and server tables are bit-identical to each other, and identical to
ioq3's `bytedirs[NUMVERTEXNORMALS]` in `code/qcommon/q_math.c` (compared all 162
triples, zero mismatches beyond 1e-6). Entry 0 is `(-0.525731, 0.0, 0.850651)`,
entry 161 is `(-0.688191, -0.587785, -0.425325)`.

So for `EV_BULLET_HIT_SMALL`/`LARGE`:

- `es.eventParm` = byte dir of the surface normal -> drives the
  `*_normal` effect (decals face this).
- `es.scale` (offset 216) = byte dir of the incoming direction reflected off
  the surface -> drives the `*_reflect` effect.

`fx/iw_impacts.csv` states the same in prose: *"The 'normal' effect is played at
the impact point facing the surface normal... The 'reflect' effect is played at
the impact point facing the direction of impact reflected off the surface."*

There is no raw-vector variant for impacts. Raw `origin2` vectors are used only
by `EV_RAILTRAIL` (end point) and `ET_FX` entities (direction).

vcod has to embed the 162 vectors. They can be regenerated from the shipped
binary rather than transcribed from ioq3: read 162 * 3 little-endian `f32` at
file offset `0x740d0` of `main/cgame_mp_x86.dll` (VA `0x300740d0`, `.data`,
which is mapped 1:1 with the file at `0x6e000`/`0x3006e000`).

---

## 4. Surface-type numbering

`es.surfType` is a 0-based index into a 23-entry list, and it is the same
index the six footstep/jump/landing event groups are built on.

| # | name | # | name | # | name |
|---|---|---|---|---|---|
| 0 | `default` | 8 | `foliage` | 16 | `plaster` |
| 1 | `bark` | 9 | `glass` | 17 | `rock` |
| 2 | `brick` | 10 | `grass` | 18 | `sand` |
| 3 | `carpet` | 11 | `gravel` | 19 | `snow` |
| 4 | `cloth` | 12 | `ice` | 20 | `water` |
| 5 | `concrete` | 13 | `metal` | 21 | `wood` |
| 6 | `dirt` | 14 | `mud` | 22 | `asphalt` |
| 7 | `flesh` | 15 | `paper` | | |

Four independent confirmations:

1. Wire source. Both impact emitters compute
   `s.surfType = (trace.surfaceFlags >> 20) & 0x1f`, a 5-bit field in bits
   20 to 24 of the shader's `surfaceFlags`
   (`Bullet_Fire_Extended 0x68af2`, `G_RunMissile 0x54152`).
2. `PM_FootstepEvent` @ `game.mp.i386.so 0x31fe4` takes the same 5-bit field
   and emits `event = base + surfType` where base is `1` (run, `lea ecx,[edx+1]`),
   `0x18`=24 (walk, `lea ecx,[edx+0x18]`) or `0x2f`=47 (prone,
   `lea ecx,[edx+0x2f]`), exactly the group bases of section 1. `surfType == 0`
   on a ground trace emits `EV_NONE` (a shader with no surfaceparm makes no
   footstep).
3. `CG_BulletHitWall` @ `0x30039683` special-cases `surfType == 7`. When the
   blood cvar is off it swaps the effect for `fx/impacts/flesh_hit_noblood.efx`
   and clears the reflect effect. Index 7 is `flesh`.
4. `fx/iw_impacts.csv` lists exactly these 23 names in exactly this order,
   and the nine per-surface effect handle tables in `.data` sit exactly
   `23 * 4 = 0x5c` bytes apart: `0x301d6190`, `0x301d61ec`, `0x301d6248`,
   `0x301d62a4`, `0x301d6300`, `0x301d635c`, `0x301d63b8`, `0x301d6414`,
   `0x301d6470` (last one ends at `0x301d64cc`).

The CSV comment matches point 2 on the meaning of index 0: *"'default'. Default
is only used if the code cannot figure out the correct surface type."*

---

## 5. `(event, weapon, surfType)` to `.efx`

There are no hardcoded per-surface `.efx` paths in the client. The only
literal `.efx` string in `cgame_mp_x86.dll` is
`fx/impacts/flesh_hit_noblood.efx` (registered in `CG_RegisterGraphics`
@ `0x30020da0`, handle stored at `0x301d64cc`). Everything else is data-driven
from three places.

### 5a. Impacts and explosions: `fx/*.csv`

`CG_LoadImpactEffects` @ `0x3001a7e0` enumerates `fx/*.csv` (syscall `0x13`
with `"fx"`, `".csv"`), sorts the names, parses each with these nine column
keys, and stores one effect handle per surface type into these tables:

| CSV impact type | handle table | consumed by |
|---|---|---|
| `bullet_small_normal` | `0x301d6190` | `EV_BULLET_HIT_SMALL`, spawned along `eventParm` dir |
| `bullet_small_reflect` | `0x301d61ec` | `EV_BULLET_HIT_SMALL`, spawned along `scale` dir |
| `bullet_large_normal` | `0x301d6248` | `EV_BULLET_HIT_LARGE`, `eventParm` dir |
| `bullet_large_reflect` | `0x301d62a4` | `EV_BULLET_HIT_LARGE`, `scale` dir |
| `grenade_bounce` | `0x301d6300` | `EV_GRENADE_BOUNCE` |
| `grenade_explode` | `0x301d635c` | `EV_GRENADE_EXPLODE` |
| `rocket_explode` | `0x301d63b8` | `EV_ROCKET_EXPLODE`, `EV_ROCKET_EXPLODE_NOMARKS` |
| `molotov_explode_normal` | `0x301d6414` | (`EV_MOLOTOV_*` is unhandled in MP) |
| `molotov_explode_reflect` | `0x301d6470` | (unhandled) |

Later-alphabetical CSVs override earlier ones. A missing (type, surface) cell is
a hard error at load: *"%i missing entries in effect CSV files"*.

Stock CoD 1.1 ships exactly one, `fx/iw_impacts.csv` in `main/pak5.pk3`
(same file in the 1.5 install). It holds one block per impact type, each 25
lines: a header, a comment, and 23 rows in the surface order of section 4,
each row an `.efx` path or blank. Read it from the pk3 rather than from a
transcription; the facts the rest of this document rests on are these:

- The bullet columns name effects by size and surface
  (`fx/impacts/small_brick.efx`, `fx/impacts/large_brick.efx`,
  `fx/impacts/metalhit_small.efx` and so on), with
  `fx/impacts/default_hit.efx` for the surfaces that have no effect of their own
  (default, bark, carpet, cloth, mud, paper in both sizes, and several more in
  the large column only) and `fx/impacts/flesh_hit.efx` for row 7 (`flesh`) in
  both sizes. Several surfaces share one effect (concrete, plaster and asphalt
  all use `small_concrete.efx` for small hits).
- Both `bullet_small_reflect` and `bullet_large_reflect` are blank for every
  surface in stock 1.1, so in practice only the normal-facing effect plays for a
  bullet impact. `grenade_bounce` is blank for every surface too.
- `grenade_explode` and `rocket_explode` are populated for every surface
  (`grenade_explode` default is `fx/explosions/grenade2.efx`, `rocket_explode`
  default is `fx/impacts/newimps/v_blast1.efx`), with surface-specific
  variants for snow, water, dirt/grass/gravel and wood.

There is no `weapon` axis on impacts: the weapon only selects small-vs-large
server-side, and adds its own `projExplosionEffect` on top for
`EV_GRENADE_EXPLODE` / `EV_ROCKET_EXPLODE`.

`CG_BulletHitWall` @ `0x30039640` picks the small pair of handle tables
(`bullet_small_normal`, `bullet_small_reflect`) when the event is 173 and the
large pair otherwise, and reads the normal and reflect handles at `[surfType]`.
When the blood cvar (value at `0x3029812c`) is 0 and `surfType == 7`, the normal
handle becomes `flesh_hit_noblood` and the reflect handle is cleared. It then
plays the per-surface sound alias through `CG_PlayQueuedSound(ENTITYNUM_WORLD,
pos)` from the alias tables at `0x301d5eec` (small) and `0x301d5f48` (large),
spawns the normal effect along dirA and the reflect effect along dirB through
syscall `0xe4` (each only when its handle is non-zero), and tail-calls
`CG_Tracer(attacker, pos, surfType, "tag_flash")` (section 6).

Syscall `0xe3` spawns an effect with position only, `0xe4` with position and
direction, `0xe5` on a model tag, `0xde` registers an `.efx` path.

### 5b. Muzzle flash: per-weapon, from the weapon file

`CG_MuzzleFlash` @ `0x30036c90` picks `viewFlashEffect` (weapondef + `0x1f4`)
in first person and `worldFlashEffect` (weapondef + `0x1f8`) otherwise. When the
handle is non-zero it resolves the tag bone by name (syscall `0xdf` on the
refEntity) and spawns the effect on that tag (syscall `0xe5`).

Weapon-definition stride is `0x198`, base `0x301a6940`. The weapon-file keys are
`viewFlashEffect` and `worldFlashEffect` (both appear as strings in the DLL,
alongside `shellEjectEffect`, `lastShotEjectEffect`, `projExplosionEffect`,
`projTrailEffect`). So muzzle-flash selection is `(weapon, first-person?)`,
never `surfType`.

Trigger: `CG_FireWeapon` sets `cent->[0x1e4] = 1`; `CG_AddPlayerWeapon`
@ `0x30036cf0` consumes and clears it, calling `CG_MuzzleFlash` with position
`cent->lerpOrigin` (or the viewmodel origin `0x3020cbac` in first person) and
tag name from the array at `0x3007486c`: `tag_flash`, `tag_flash_2`,
`tag_flash_11`, `tag_flash_22` (indexed by barrel, for the quadbarrel events).
For `ET_TURRET` (`es.eType == 11`) `CG_FireWeapon` calls `CG_MuzzleFlash`
directly.

### 5c. Script effects: configstrings

`CG_ConfigStringModified` @ `0x3002c6b0` registers configstrings
780..843 as effects (`syscall 0xde`, stored at `0x301d1ccc + cs*4`, which is
`0x301d28fc + id*4` for `id = cs - 780`). `EV_PLAY_FX*` and `ET_FX` index that
array with `1..63`, i.e. the `.efx` path is `configstring(780 + id)`.
`crates/common/src/net/protocol.rs` documents the same base. Models are
268..523, shellshock 1100..1115, in the same function.

---

## 6. How tracers are triggered

Tracers are entirely client-side and are derived from the bullet impact
events. There is no per-shot tracer event on the wire in CoD 1.1 multiplayer.

`EV_BULLET_TRACER` (171) exists in the enum but has no case in the MP
`CG_EntityEvent` jump table; it lands on `"Unknown event: '%s'"`. Same for
`EV_BULLET` (186). Neither is emitted by `game.mp.i386.so` on any path found.

The real path is `CG_Tracer` @ `0x30039590`, tail-called by both
`CG_BulletHitWall` (`0x300396dc`) and `CG_BulletHitFlesh` (`0x30039721`) with
`eax = es.otherEntityNum` (the shooter), `esi = impact position`,
arg0 = `surfType`, arg1 = the string `"tag_flash"`. It returns at once when
`cg_tracerchance` (value at `0x301df448`) is `<= 0` or when `CG_GetMuzzlePoint`
@ `0x30039440` cannot resolve `tag_flash` on the shooter entity. Unless the
shooter is the entity being viewed first person, it rolls
`rand() < cg_tracerchance * RAND_MAX` and on success calls `CG_SpawnTracer`
@ `0x30038f30` with the muzzle point and the impact position; `CG_SpawnTracer`
tail-calls the segment setup at `0x30039340`. In every case it then calls
`CG_BulletWhizby` @ `0x30038dc0`, which plays the whiz-by sound near the
`cg.refdef` origin.

`0x30039340` is the shared tracer segment-setup routine, the `len`/`frac`/`endDist`
computation that both `CG_BulletHitWall` and `CG_BulletHitFlesh` reach through
`CG_SpawnTracer` (decompiled against `CoDMP.exe`, see
`docs/research/efx-grammar.md` section R8). Earlier I had it
labelled `CG_BloodSpray`, called only when `surfType == 7`; neither decompile
pass found a flesh-specific branch, so vcod draws the same tracer quad for
every surface type.

`CG_SpawnTracer` allocates a local entity with type 2, start/end and a velocity
scaled by the tracer-speed constant, and randomly back-dates its spawn time by
up to half a frame. The tracer is drawn with the shader `gfx/misc/tracer`
(registered in `CG_RegisterGraphics` @ `0x30020da0`, handle stored at
`0x301d5abc`). Cvars: `cg_tracerchance` (value at `0x301df448`),
`cg_tracerSpeed`, `cg_tracerlength`, `cg_tracerwidth`.

The "viewing this entity first person" suppression is
`(cg.snap->ps.pm_flags & 0x50000) && shooterEnt == cg.snap->ps.clientNum`, see
section 7. For a follow-spectator that means the followed player's own tracers
are suppressed by the retail client; vcod can choose to draw them.

Practical consequence for vcod: to draw tracers, hook `EV_BULLET_HIT_*`, take
`es.otherEntityNum` as the shooter, resolve that entity's muzzle
(`tag_flash` on its held weapon, which the held-weapon assembly already
places), and draw a streak from there to `es.pos.trBase`, gated on a
tracer-chance roll.

---

## 7. Detecting spectator-follow state in `playerState`

Confirmed: `ps.clientNum != our own client number` means we are following
someone.

`SpectatorClientEndFrame` @ `game.mp.i386.so 0x40760` is the whole story. When
`client->sess.spectatorClient >= 0` the server copies the followed player's
whole client block over the spectator's (`rep movsd` with `ecx = 0x834`, so
8400 bytes starting at `ps`, the entire `playerState_t` and then some) and then
patches the flags back in byte `ps+0xe`: it clears bit `0x04` (pm_flags
`0x040000`), sets bit `0x01` (`0x010000`), and sets bit `0x02` (`0x020000`)
when `spectatorClient >= 0` or clears it otherwise.

`ps.pm_flags` is at offset 12 (19 bits), so byte `ps+0xe` holds bits 16 to 23.
Because the whole playerState is copied, `ps.clientNum` (offset 172, 8 bits)
becomes the followed player's client number, and `ps.origin`, `viewangles`,
`weapon`, `weaponstate`, `legsAnim`/`torsoAnim` and everything else describe the
followed player too. In free-fly spectate the copy does not happen and
`ps.clientNum` stays our own.

Flag summary, all from setters/testers in the server:

| Bit | Meaning | Evidence |
|---|---|---|
| `0x010000` | spectator view | set unconditionally in `SpectatorClientEndFrame` `0x408ea`; tested by `StopFollowing` `0x46a51` |
| `0x020000` | following a specific client (vs free-fly) | set/cleared on `spectatorClient >= 0`, `0x40903`/`0x408fd`; also preserved across the ps copy at `0x408c1` |
| `0x040000` | live in-game player | set in `ClientEndFrame` `0x40fb0`, cleared for spectators `0x407b7`; `GetFollowPlayerState` `0x415dc` refuses to copy a player who does not have it |

The client's own recurring test,
`(ps.pm_flags & 0x50000) != 0 && es.number == ps.clientNum`, therefore reads as
"the view is attached to a player body (either mine, `0x40000`, or one I am
following, `0x10000`) and this event came from that body". That is the
first-person suppression case.

vcod already knows its own client number from the connect handshake, so:

- `ps.clientNum != our_client_num` -> following that client. Confirmed.
- `(ps.pm_flags & 0x010000) != 0` -> spectator view of any kind.
- `(ps.pm_flags & 0x020000) != 0` -> following a specific client rather than
  free-flying.

The netfield table in `crates/common/src/net/fields_v1.rs` gives `pm_flags`
19 bits, so all three flags survive the delta encoding.

---

## 8. 1.1 vs 1.5: the `EV_*` table did not shift

No change. The 202-entry `eventnames` array is byte-for-byte identical
across every multiplayer module checked:

| Module | version | table VA | entries | matches 1.1 MP? |
|---|---|---|---|---|
| `main/cgame_mp_x86.dll` | 1.1 | `0x30077040` | 202 | reference |
| `main/game_mp_x86.dll` | 1.1 | `0x2006c8b8` | 202 | yes |
| `main/cgame_mp_x86.dll` | 1.5 | `0x30079078` | 202 | yes |
| `main/game_mp_x86.dll` | 1.5 | `0x20070d70` | 202 | yes |
| `game.mp.i386.so` | 1.1d Linux | `0x0007b6a8` | 202 | yes |

Only the table's address moved. The surrounding machinery is unchanged too: the
1.5 MP cgame carries the same nine CSV impact-type keys
(`bullet_small_normal` ... `molotov_explode_reflect`), the same
`viewFlashEffect`/`worldFlashEffect` weapon keys and the same `tag_flash` tag,
and the 1.5 install ships the same `fx/iw_impacts.csv` in `main/pak5.pk3`.

The single-player modules also agree between 1.1 and 1.5 (198 entries, identical
order, table at `0x30067f20` in both `cgamex86.dll` files). As established
at the top, the SP enum is a different enum and must not be used.

A constants module written against 1.1 MP will therefore work unchanged against
1.5 MP. The two things that do differ per version and were not surveyed here
are the netfield tables (already handled by `crates/common/src/net/protocol.rs`
being version-parameterised) and the syscall numbers, which are internal to the
retail DLLs and irrelevant to vcod.

---

## Open items

Only one claim in this document was not settled from the binaries: whether a
follow-spectator receives `EV_BULLET_HIT_CLIENT_SMALL/LARGE` (175/176) for the
player being followed. The server marks them `svFlags = 0x800` +
`r.singleClient = victim`, which is single-client delivery keyed on the real
client slot. The live capture in section 2 (2026-08-24, 51.195.89.86:28960)
settled it: 175/176 never arrived, so vcod ignores them and treats
`173/174 with surfType == 7` as the only flesh-hit signal.
