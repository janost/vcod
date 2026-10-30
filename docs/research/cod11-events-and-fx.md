# CoD 1.1 event table and effect mapping

Read out of CoD 1.1 binaries and stock assets. A claim the binaries could not
settle is marked `UNVERIFIED` with the live check that would settle it.

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
explosion event sits above 172, so the SP table puts every combat event off
by one.

vcod is a multiplayer client, so `main/cgame_mp_x86.dll` is the authority
here. The 202-entry table is byte-for-byte identical in all four MP modules
and the Linux dedicated server's game module, which puts the numbers on the
wire.

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
the Linux `.so`. Decompilations come from `tools/re/ExportDecomp.java`
(Ghidra headless), which dumps every function with a name and address marker.

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
`< 0x5d`, `< 0x74`, `< 0x8b`), which confirms the group sizes.

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
them land on the `"Unknown event: '%s'"` warning at `0x3001e703`, so the retail
MP client does nothing with them (the jump table at `0x3001e72c` through the
index byte table at `0x3001e7dc`): `EV_BULLET_TRACER` (171),
`EV_MOLOTOV_EXPLODE` (181), `EV_MOLOTOV_EXPLODE_NOMARKS` (182),
`EV_CUSTOM_EXPLODE` (183), `EV_CUSTOM_EXPLODE_NOMARKS` (184), `EV_BULLET` (186).
Another ten reach a bare `break` (handled only in `CG_EntityPreEvent` or not
at all): 150, 165, 167, 173, 174, 175, 176, 187, 188, 195.

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

### An entity that enters the snapshot

- `CG_ResetEntity` (1.1 cgame `0x3002f840`) runs on an entity whose
  centity `+0x1e0` is 0 or whose `eFlags` 0x8 changed (both tests in its
  caller `0x3002fc80`) (VERIFIED decompile). `+0x1e0` is Q3's
  `currentValid`, which makes the first test "not in the last snapshot"
  (INFERRED, off Q3's `CG_TransitionSnapshot`, which has the same pair).
- It copies the next state over the current and sets the ring's last
  sequence (centity `+0x1e8`, which `CG_CheckEvents` replays from) by
  `eType`: 0 (general) and 4 (missile) to 0; 2 (corpse) to 0 with `eFlags`
  0x800, else to the state's `eventSequence`; 1 (player) and every other
  type to the state's `eventSequence` (VERIFIED decompile). So an entering
  general entity's or missile's ring replays from 0, capped at the last 4,
  and an entering player's does not (INFERRED, off `CG_CheckEvents`' loop).
- 1.5's (`cgame_mp15.dll` `0x30030f30`, ring at `0x3001fca0`) is the same
  switch with the same offsets, and United Offensive's (`0x3003c7c0`, ring
  at `0x300238e0`, the last sequence at `+0x1f0`, the corpse bit 0x400 in
  UO's layout, 0x800 in 1.1's) is the same switch (VERIFIED decompile).
- The case that needs it: a UO tank shell striking within its first step
  is never sent in flight; it enters the snapshot already exploded, `eType`
  0 with `eventSequence` 1 (VERIFIED, `*-uo-tank.bin` entity 164 at 31100,
  and vcod-server's shell into the wall ahead of mp_cassino's Panzer IV).
  `net::events::EventTracker` follows the rule. Skipping every entering
  ring drops the explosion of a tank shell fired at close range.

`centity_t` is 0x228 (552) bytes, array base `0x3020db98 - 0x20`;
`currentState` at +0, `nextState` at +0xf0, `lerpOrigin` at +0x1f8.
`sizeof(entityState_t) == 0xf0` (240), confirmed by the `imul reg,reg,0xf0`
sites in `CoDMP.exe`.

---

## 2. Which state fields each combat-relevant event populates

`entityState_t` offsets are the netfield offsets. The client-side table in
`CoDMP.exe` @ `0x005419d0` (59 entries) is identical in order, offset and bit
width to the one `crates/common/src/net/cod1/fields_v1.rs` was generated from, so
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
byte, not a scale. vcod's parser decodes it under the name `scale`.

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

VERIFIED, `game.mp.i386.so` 0x68aef..0x68b05: the hitscan site's
`otherEntityNum` is the number of `Bullet_Fire_Extended`'s eighth argument
(`[ebp+0x24]`), the inflictor. `FireWeapon` passes the player there
(0x78d68, both entity arguments the player), and the mounted MG's
`Bullet_Fire` (0x690dc) passes the gun: the turret's fire block calls it
with the gunner first and the gun last (0x621d4, the gun is the entity
that gets `EV_FIRE_WEAPON_MG42`), and `Bullet_Fire` forwards the gun as
both the trace's pass entity and the eighth argument. So a turret round's
world impact names the gun, not the gunner. VERIFIED (capture):
`snapshots-15-turret.bin` (1.5b) carries 201 `EV_BULLET_HIT_*` from the
followed gunner's rounds and every one names entity 298, the `eType` 11
gun (`play::replay::a_1_5_turret_rounds_impacts_name_the_gun`). The flesh
copy of a hit (below) names the script callback's attacker instead.

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

A flesh hit reaches everyone else as one of these with `surfType == 7`.
VERIFIED: the player-hit path in `finishPlayerDamage`
(`game.mp.i386.so 0x43ab5` to `0x43b0a`) hardcodes `te->s.surfType = 7`,
writes `DirToByte` of the normalized `vDir` to both `eventParm` and `scale`,
ORs `0x20` into the `svFlags` byte at `+0xf5` (that is, `svFlags |= 0x2000`)
and stores the victim's `ps->clientNum` in `r.singleClient` (`+0xf8`). The
delivery rule under the next event says what `0x2000` does with it.

### `EV_BULLET_HIT_CLIENT_SMALL` (175) / `EV_BULLET_HIT_CLIENT_LARGE` (176)

`game.mp.i386.so 0x43b1c` to `0x43b78`:

```
te->s.surfType       (136) = 7;                   // always flesh
te->s.otherEntityNum (116) = attacker entity number;
te->s.clientNum      (144) = victim's clientNum;
te->r.svFlags        (244) = 0x800;               // a store, not an OR
te->r.singleClient   (248) = victim's clientNum;
```

No direction bytes. VERIFIED: with no attacker entity passed,
`otherEntityNum` is 1022, since the attacker pointer starts as
`g_entities + 0xc49d8` (`0x43778`) and `0xc49d8` is `1022 * 0x314`, the
gentity size the `imul` at `0x4379b` indexes by. The pair and the conditions
it is raised on (a `weaponType` 0 weapon, melee included) are
`docs/research/cod11-combat.md` 4.5. Consumer, `CG_EntityPreEvent` case 175/176 @ `0x3001e8d9`:
`CG_BulletHitFlesh(pos=cent->lerpOrigin, surfType=es.surfType, attacker=es.otherEntityNum)`
@ `0x300396f0`. VERIFIED, the register loads at `0x3001e8d9`..`0x3001e8ea`:
`edx = es+0x74` (`otherEntityNum`, pushed), `edi = es+0x88` (`surfType`),
`ecx = cent+0x1f8`, `eax` the event id. What the function does:

- VERIFIED: `cmp eax,0xaf` at `0x300396f1` picks the alias table,
  `0x301d5eec` for 175 and `0x301d5f48` for 176, indexed by `surfType`. These
  are the tables `CG_BulletHitWall` reads its impact sound from for 173 and
  174 (`0x3003964d`, `0x30039664`).
- VERIFIED: it calls `0x30021bf0` with `0x3fe` (`ENTITYNUM_WORLD`) and the
  impact position, the same call the wall path plays its impact sound through.
- VERIFIED: it tail-calls `CG_Tracer` @ `0x30039590` at `0x30039721` with
  `eax = otherEntityNum`, `esi` still the impact position, and the pushed pair
  `surfType`, `ds:0x3007486c` (`"tag_flash"`), which is the argument shape
  `CG_BulletHitWall` passes at `0x300396dc`.
- VERIFIED: its body (`0x300396f0`..`0x3003972b`) holds no `push 0xe4`, the
  effect syscall the wall path spawns its impact `.efx` through, so a client
  hit draws no effect.

Delivery, `cod_lnxded` (the 1.1d engine binary). VERIFIED: the snapshot
builder loads `esi = ps+0xac` (`ps.clientNum`) from the frame's playerstate at
`0x808f25f`, after copying it in, and pushes it as the second argument of the
entity pass `0x808e298` at `0x808f301`. VERIFIED: that pass tests `svFlags`
(`gentity+0xf4`) for `0x800` at `0x808e322` and compares `r.singleClient`
(`+0xf8`) with that argument at `0x808e32a`, and tests `0x2000` at
`0x808e336` against the same compare at `0x808e345`. INFERRED from those
branches: an entity with `0x800` is skipped when `r.singleClient` differs from
the snapshot's `ps.clientNum` (`jne` at `0x808e330`), and one with `0x2000` is
skipped when it equals it (`je` at `0x808e34b`). So 175/176 go only to the
snapshots whose `ps.clientNum` is the victim, and the plain 173/174 flesh copy
to every other. Both carry no `SVF_BROADCAST`, so both are PVS-culled after
that. A follow-spectator's `ps.clientNum` is the followed player's (section
7), so INFERRED: whoever follows the victim receives the victim's 175/176 and
not the 173/174 copy.

VERIFIED live, the retail 1.1d server running
`client-probes/probe_bump` as the gametype with `+set probe_teleport 1`, a
`--probe-target` client and a `--save-hit --probe-sweep` client placed 200
units apart, both printing every 173 to 176 they drained:

- The victim (client 0) received 176 on every hit, `surfType` 7,
  `otherEntityNum` 1 (the shooter), `eventParm` 0, and no 174 with
  `surfType` 7 in the same frame. The shooter received the 174 with
  `surfType` 7 at the same origin on the same frame, `eventParm` 52 or 53,
  and no 176.
- During the victim's killcam the victim received the 174 flesh copies of the
  shots that had killed it, at their original origins, and no 176. Those
  frames carried health 100 and `eFlags` with `0x400`. VERIFIED: the stock
  `maps/mp/gametypes/dm.gsc` `killcam` (`pak5.pk3`) sets
  `self.spectatorclient` to the attacker's number and `self.archivetime` to
  the delay plus 7. INFERRED: a killcam frame is the killer's archived
  playerstate, so its `ps.clientNum` is the killer's and the `0x2000` test
  lets the plain copy through. The delivery rule keys on the frame's
  `ps.clientNum`, not on the slot the snapshot goes to.
- A second run with `--probe-melee` on both halves, target in slot 1: each
  swing that landed (`D;` lines with `MOD_MELEE`) put a 174 with `surfType` 7
  and `eventParm` 111 or 112 in the swinger's snapshot beside the `EV_MELEE_HIT`,
  and a 176 with `clientNum` 1 in the victim's.

VERIFIED live (a public TDM server, 100 s capture during combat by a
free-flying spectator): 175/176 never arrived while 173 with `surfType == 7`
appeared 38 times. A free spectator's `ps.clientNum` is its own, so this
matches the delivery rule; it does not show the events are unused. vcod resolves 175/176 in play mode and while following,
through the same path as 173/174 minus the impact effect
(`crates/client/src/fx/registry.rs`, `crates/client/src/audio/cues.rs`).
vcod's server raises the pair in `finishPlayerDamage`
(`crates/server/src/game/builtins/combat.rs`, `combat::flesh_impacts`) and
filters it per snapshot against the frame's `ps.clientNum`
(`crates/server/src/game/temp_entity.rs`). It models the follow and not the
killcam, so a follower's frame carries the followed client's number
(`cod11-spectator-follow.md`).

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
which suppresses the decal (section 9.2).

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

The server half is the `playFx` builtin, `functions[66]` at
`game.mp.i386.so` 0x5b148. VERIFIED: it refuses fewer than 2 or more than 3
arguments with the usage string at 0x77a00, reads `Scr_GetInt(0)` and
`Scr_GetVector(1)`, and has two `G_TempEntity` calls, one with event 0xbf
(`EV_PLAY_FX`, 0x5b25a) and one with 0xc0 (`EV_PLAY_FX_DIR`, 0x5b228), each
followed by a store of the id's low byte at `ent+0xa0` (`eventParm`). The
0xc0 arm also stores `DirToByte` of the `VectorNormalize`d third argument at
`ent+0xd8` (`scale`, 0x5b24f), and the function formats
`"playFx called with (0 0 0) forward direction (effect = %s)\n"` (0x77a80)
into a `Scr_Error`. INFERRED, off the `Scr_GetNumParam() == 3` branch at
0x5b19a: the third argument picks the 0xc0 arm. INFERRED, off the `fucompp`
against zero at 0x5b1c1: the error is the arm a zero-length vector takes.
VERIFIED: nothing in the function writes `svFlags`. INFERRED: the event is
sent by PVS like any bare temp entity.

vcod plays both in `fx::registry` off configstring `780 + parm`
(`CS_EFFECTS`), on syscall `0xe3`'s up axis or `0xe4`'s along the byte dir
(`efx-grammar.md` R11). `EV_PLAY_FX_ON_TAG` is "`playFxOnTag`" below.

VERIFIED: the id is `G_EffectIndex`'s (0x65fa4), which `loadFx` (0x5ef88)
returns unchanged: the scan runs `i` from 1 while `i < 0x40`, compares
`configstring(0x30c + i)` with `strcasecmp`, and a new effect is written at
`0x30c + i` and `i` returned. So `level._effect[...]` holds `1..63`, never a
configstring number.

#### `loadFx` after the first wait

- VERIFIED, game.mp: `G_InitGame` stores 1 at `level+0x1c` (0x4fc5d) and
  0 (0x4fe7e) right after its calls to `Scr_LoadGameType`, `Scr_LoadLevel`
  and `Scr_StartupGameType`. INFERRED: that word is `level.initializing`,
  set while the gametype's `main`, the map's `main` and
  `Callback_StartGameType` run to their first `wait`.
- VERIFIED, `G_EffectIndex`: when the scan finds no match and `level+0x1c`
  is 0 (0x65ff4) it raises `Scr_Error` on `va("%s \"%s\" not precached",
  "effect", name)` (strings 0x79ae0, 0x79b6a) and returns 0; nothing is
  written. `loadFx` itself raises the 0x77980 error (`loadFx must be called
  before any wait statements in the gametype or level script, or on an
  already loaded effect`) on a 0 id with the word clear (0x5efa7..0x5efbc).
- VERIFIED, `crates/gsc/tests/fixtures/semantics/probe_loadfx_wait.gsc` on
  retail 1.1d: after one `wait 0.05` an effect `main` loaded
  comes back 1, spelt in another case too, and a new one kills the server
  with `effect "fx/smoke/chimneysmoke.efx" not precached`. INFERRED: the
  first `Scr_Error` ends the call, so the 0x77980 message is never seen.

vcod: `GameHost::initializing`, set around `ScriptRuntime::start_bootstrap`,
and `Allocators::effect_index`'s `strcasecmp` scan; the probe's A/B is
`probe_loadfx_wait_matches_retail` in `crates/server/tests/semantics_ents.rs`.
The other precache builtins' "before any wait" errors (strings at
0x77580..0x77740) are in `cod11-gsc-language.md` 9, "Precache builtins".

VERIFIED, `pak5.pk3`: `fx/explosions/mp_bomb.efx` ends in a `Sound` block
naming `explo_rock`, and `maps/MP/gametypes/sd.gsc`'s `bomb_countdown` makes no
sound call of its own: it deletes the defuse trigger and `level.bombmodel`,
then `playfx(level._effect["bombexplosion"], origin)` and
`radiusDamage(origin, 500, 2000, 1000)`. INFERRED: the S&D explosion is heard
only through the effect, so a server that does not raise `EV_PLAY_FX` has a
silent bomb.

### Looped effects: `playLoopedFx` and `ET_FX` (10)

The server half is `playLoopedFx`, `functions[68]` at `game.mp.i386.so`
0x5b3c0 (`python3 tools/re/dump_builtins.py`), disassembled through
`tools/re/annotate_func.py`:

- VERIFIED: 3 to 5 arguments (`cmp eax,2; jbe` and `cmp eax,5; jbe`,
  0x5b3d1..0x5b3de), else the usage string at 0x77be0, which names them
  `<effect id from loadFx> <repeat delay> <vector position of effect>
  <optional cull distance (0 = never cull)> <optional forward vector>`.
- VERIFIED: with 5 the forward is `Scr_GetVector(4)` through
  `VectorNormalize` (0x5b442..0x5b44b), and a zero length formats
  `"playLoopedFx called with (0 0 0) forward direction (effect = %s)"`
  (0x77c80) into a `Scr_Error`. With 4 or 5 the cull is `Scr_GetFloat(3)`
  (0x5b4ca); with fewer both stay 0.
- VERIFIED: the origin is `Scr_GetVector(2)` and the delay
  `Scr_GetFloat(1) * 1000.0 + 0.5` (rodata 0x77cc4, 0x77cc8) stored by
  `fistp` under control word `| 0xc00`, so truncated to whole ms.
  Nothing checks it.
- VERIFIED, after `G_Spawn` (0x5b52f): `eType` (`+0x4`) = 10, `svFlags`
  (`+0xf4`) `|= 8` (`SVF_BROADCAST`, `cod11-sound-system.md` 9), `scale`
  (`+0xd8`) = the id's low byte, `G_SetOrigin` at the origin, `origin2`
  (`+0x5c..+0x64`) = the forward, `angles2[0]` (`+0x68`) = the cull,
  `angles2[1]` (`+0x6c`) = the delay in ms as a float, `trap_LinkEntity`,
  and `Scr_AddEntity` returns the entity.

The client half is `CG_AddCEntity` case 10 (the `eType` switch in cgame
0x3001d5f0, calling 0x3001cf20, named `CG_FX` here):

- VERIFIED: the period is `_ftol(es.angles2[1])` (0x3001cf37, `_ftol` at
  0x3005a890) and the last play time lives at `cent+0x21c`. When
  `cg.time` (0x30207148) is past it: fewer than a period behind returns
  with nothing drawn; otherwise the time advances by the period, at least
  once and until no more than a period is left (0x3001cf50..0x3001cf68).
  When `cg.time` is not past it, it is set to `cg.time`. INFERRED, off a
  grep of the module's disassembly for `+0x21c]`: nothing else in cgame writes that
  slot, so the timer outlives the entity and starts from 0 for a slot
  never used. INFERRED, off the loop's `jg` at 0x3001cf68: a zero period
  never leaves it, so retail's client hangs. vcod deliberately plays the
  effect every frame instead (`fx::looped`).
- VERIFIED: with `angles2[0]` non-zero the play goes ahead only while
  `angles2[0]^2` exceeds the squared distance from `lerpOrigin`
  (`cent+0x1f8`) to `cg.refdef.vieworg` (0x30207170); the timer has
  already moved either way.
- VERIFIED: the id is `es.scale` (`cent+0xd8`), 1..63 or the
  `"CG_PlayFx called with invalid effect id %i"` print. A non-zero
  `origin2` plays through syscall `0xe4` with it as the direction, a zero
  one through `0xe3` (`efx-grammar.md` R11 has the axes).

vcod: `crates/server/src/game/builtins/fx.rs` `play_looped_fx` and
`wire.rs`'s `Kind::Fx`; the client's `crates/client/src/fx/looped.rs`.

### `playFxOnTag`

`functions[67]` at `game.mp.i386.so` 0x5b280 is a function of three
arguments, not an entity method:

- VERIFIED: anything but 3 arguments is the usage error at 0x77ac0
  (`playFxOnTag <effect id from loadFx> <entity> <tag name>`); an id with
  `id - 1 > 0x3e` unsigned is `"effect id %i is invalid"` (0x77aff); an
  entity whose model byte (`+0x175`) is 0 is `"cannot play fx on entity
  with no model"` (0x77b20); a tag holding `"` is refused (0x77b60); and
  `trap_DObjGetBoneIndex` below 0 dumps the DObj and errors with
  `"tag '%s' does not exist on entity with model '%s'"` (0x77ba0).
- VERIFIED: `G_FindConfigstringIndex(va("%02d%s", id, tag), 0x34c, 0x100,
  1, ..)` (0x5b37d..0x5b39e), then `G_AddEvent(ent, 0xc1, index)`
  (0x5b3b0). `G_FindConfigstringIndex` (0x66e30) scans `i` from 1 while
  `i < max` with `strcasecmp`, so the pairs sit at 845..1099 and the
  event carries `i`.
- VERIFIED, cgame 0x3001db80 (named `CG_PlayFxOnTag` here): the
  configstring is `0x34c + parm`, bounds-checked against 0..0x7ff with
  `"CG_ConfigString: bad index: %i"`; the effect handle is read from
  `0x301d20bc + (c0 * 10 + c1) * 4` with `c0`, `c1` the two leading
  characters as signed bytes (0x3001dbb5..0x3001dbc2). `0x301d28fc`
  (`CG_PlayFx`'s array) less `0x301d20bc` is `0x840`, `('0' * 10 + '0') *
  4`. INFERRED: the two digits index the effect array with no range check.
- VERIFIED: syscall `0xdf` takes the entity's number (`cent+0`) and the
  string after the two digits; syscall `0xe5` gets the handle,
  `cent+0x1f8` (`lerpOrigin`), a null axis and a pointer to the pair
  (number, `0xdf`'s result). INFERRED, off the `jl` at 0x3001dbe8: `0xe5`
  is skipped for a negative result, so `0xdf` is the DObj's bone lookup, a
  tag the model lacks plays nothing, and the pair is the bolt `PlayEffect`
  takes.
- VERIFIED, CoDMP.exe: the cgame syscall switch's `0xe5` arm (0x4040e5)
  hands its four arguments straight to `PlayEffect` (0x493870), which
  calls 0x49e8d0 on the bolt into the origin and axis it spawns with and
  holds a return on that call's zero result. The origin it was passed is
  read by the per-element `cullrange` test (`efx-grammar.md` R9, R10).
  INFERRED, off the test of the bolt's entity against 0: a bolted play
  takes the tag's pose in place of the origin passed, and a tag that
  fails to pose plays nothing.
- VERIFIED, 0x49e8d0: its two calls into cgame are `vmMain` 12, which is
  0x30020260 (`lerpOrigin` and `AnglesToAxis(lerpAngles)`, `cent+0x204`,
  0x3003c770, for numbers under 0x400), and `vmMain` 11, which is
  0x3001d560 (the DObj's skeleton calc). The pose it writes is that frame
  times a 0x40-byte bone matrix out of the entity's DObj. INFERRED, off its
  test of the bone index against 0: a bone of -1 gets the entity's frame
  alone.
- VERIFIED, 0x3001d090 (named `CG_InterpolateEntityPosition` here, reached from
  0x3001d210 for a `pos.trType` 1 entity and for a `trType` 3 one under
  64): for `eType` 1 it copies the lerped angles into the client info
  (`ci+0x3e4`) and zeroes `lerpAngles` pitch and roll (`cent+0x204`,
  `+0x20c`). INFERRED, off that and `CG_Player`'s `AnglesToAxis(cent+0x204)`
  at 0x300282d7: a player's frame, drawn or bolted, turns by its yaw alone,
  as vcod's yaw-only player frame does. A corpse (`eType` 2) takes its
  `apos` whole through `BG_EvaluateTrajectory`; VERIFIED, every corpse in
  the committed snapshot captures (`dyingpose`, `turret-hit`, `uo-arty`,
  `uo-satchel`) carries pitch and roll 0, so vcod's yaw-only corpse reads
  the same there.
- VERIFIED, 0x493f40: a delayed element's queued record keeps the bolt,
  and the due branch calls 0x49e8d0 before the spawner. INFERRED, off its
  test of the record's entity, with the spawner's `relative` branch (R10): an
  effect played on a tag spawns at the tag's pose of its own frame, each
  delayed element at the pose of the frame it falls due, and only its
  `relative` elements follow the tag after that, dying when it can no
  longer be posed.

vcod plays it through `fx::registry::Resolved::OnTag`:
`entities::EntityScene::tag_pose` reads the tag off the entity as it was
drawn this frame, bone matched without regard to case as the server's tag
check does, and `FxSystem::spawn_on_tag` plays the effect on that axis
(`SpawnAt::Oriented`), its `relative` elements bolted to the tag and
re-posed every frame (`FxSystem::tag_bolts`, `set_tag_bolts`). A delayed
element, and a delayed `FxRunner`'s effect, waits in the tag's frame and
is posed on the tag as it stands the frame it falls due (`Particle::repose`,
`Chain::repose`); with the tag gone it spawns nothing. Where it differs:

- A plain model (`script_model`, item, missile) is posed in its bind
  pose, which is how vcod draws it.
- `cullrange` is measured from the tag, not from `lerpOrigin`.
- An effect on the body the view rides (the playerState ring, or the
  followed player) plays on its tag:
  - INFERRED, off the branch in cgame 0x3001bd00: under `pm_flags`
    0x50000 (the own view, or a followed client's copied frame) the
    predicted entity at 0x3020922c goes through the entity pass
    (0x3001d210, then 0x3001d5f0), and `CG_Player` (0x30028210) sets its
    `renderfx` to 2 when its number is `ps.clientNum` under the same bits.
    So that body's DObj is posed every frame and only the first-person
    view leaves it out.
  - INFERRED, off the snapshot transition 0x3002fc80: under the same bits
    it calls `BG_PlayerStateToEntityState` (0x30005ad0) with
    `ps.clientNum` in hand and then resets `cg_entities[ps.clientNum]`'s
    lerp (0x3002f840), so the entity `vmMain` 12 frames the bolt with is
    the playerstate's.
  - vcod builds that body from the playerstate (`play::seat::own_body`,
    under `play::seat::adds_view_body`), poses it in `build_instances`
    and keeps it out of the draw list, the muzzles, the shadows and the
    entity positions. A free spectator's frame carries neither bit, and a
    UO rider whose seat hides the body poses none, so an effect on either
    plays nothing.

### On the wire, measured

VERIFIED, `crates/server/tests/fixtures/gametypes/mp_carentan-playfx-fx.txt`
and `-log.txt` (retail 1.1d, `client-probes/probe_playfx.gsc`,
one allied `vcod --net-probe --save-scripted fx`), each effect seen in the
snapshot of the frame its script step ran:

- `playFx(bomb, (900 1930 -30))`: a temp entity (`eType` 12 + 191) at
  number 170, `eventParm` the id, at the origin.
- With forward `(0 3 4)`: 171, event 192, `scale` the byte dir of
  `(0, 0.6, 0.8)`, which reads back as `(0, 0.526, 0.851)`.
- At `(8900 9930 -38)`, 8000 units off past the map's edge: 172 is still
  sent, and ours sends it too. Which rule of the snapshot cull lets it
  through was not read.
- Three `playLoopedFx` calls: 173, 174 and 175, all `eType` 10, the
  far one included (the broadcast flag), with `scale`, `origin2` and
  `angles2` as above: `0.2` s is period 200, `1.2345` s is 1235.
- `playFxOnTag(bomb, player, "tag_helmet")`: `EV_PLAY_FX_ON_TAG` on the
  player's own playerstate ring, parm 1, configstring 845 `01tag_helmet`.
- `delete()` of a looped effect: gone from the snapshot of the frame of the
  call, not 100 ms later when the slot is freed. VERIFIED, the method at
  0x5da14 calls `trap_UnlinkEntity` (0x5da81) before arming the free
  (`cod11-gsc-object-model.md` 14); this capture shows that unlink on the
  wire.

`crates/server/tests/fx_ab.rs` replays the probe on ours and compares
every `!fx` line, each timed from the step before it.

### Which stock maps play map effects

From the paks (`maps/mp/*.gsc`, every `maps/mp/*.bsp` entity lump),
VERIFIED:

- No stock MP script calls `playLoopedFx` or `playFxOnTag`; only the
  single-player `maps/_fx.gsc` (`pak4.pk3`) does. MP's `maps/mp/_fx.gsc`
  `loopfx` repeats `playFx` from a thread instead, and its only caller,
  `mp_chateau_fx.gsc`'s chimney, is commented out of `mp_chateau.gsc`.
- `sd.gsc`'s `bomb_countdown` plays `fx/explosions/mp_bomb.efx` at the
  charge on every S&D map, and first `maps\mp\_utility::exploder` on the
  bombzone's `script_noteworthy`. `exploder` runs `cannon_effect` on every
  `script_model` of that group, which `OneShotfx`es its `script_fxid`
  0.05 s plus `script_delay` later, along the line to its `target` when it
  has one (`EV_PLAY_FX_DIR`), else plain (`EV_PLAY_FX`).
- Exploder effects, by map (bombzone A is exploder 1 on all but
  mp_powcamp, where A is 2): mp_dawnville (`flakpanzerexplosion`, both
  zones), mp_depot and mp_railyard (`tigertankexplosion`), mp_pavlov
  (`antitankgunexplosion`), mp_rocket (`fueltank` at 1, `v2rocketexplosion`
  at 2), all `fx/explosions/explosion1_nolight.efx` but the fuel tank's
  `fx/explosions/fueltank_ned.efx`; mp_powcamp eight per zone (`dirt`,
  `dust`, `wood_close`, each targeted, so directed). mp_brecourt,
  mp_carentan, mp_harbor and mp_hurtgen have exploder groups with no
  effect models; mp_chateau and mp_ship have none. mp_brecourt plays
  dawnville's effects file, which only loads.
- `maps/mp/_minefields.gsc` plays `fx/impacts/newimps/minefield.efx` on a
  minefield death:
  mp_brecourt (32 minefield triggers), mp_hurtgen (14), mp_pavlov (26),
  mp_rocket (10).

### Others worth knowing

| Event | Fields read |
|---|---|
| `EV_SOUND_ALIAS` (172) | `es.eventParm` -> configstring `524 + parm`; position `es.pos.trBase` (`cent+0x18`) |
| `EV_STEP_VIEW` (143) | `es.eventParm - 128` is the signed view-step in units; `es.clientNum` must equal `cg.clientNum` |
| `EV_ITEM_PICKUP` (146) / `EV_AMMO_PICKUP` (148) | `es.eventParm` is the weapon index (1..69) |
| reload / raise / putaway (151 to 158, 162) | `es.weapon` indexes the weapon table (stride 0x198) |
| `EV_MELEE_HIT` (166) | `es.otherEntityNum` (116) is the sound emitter |
| `EV_EARTHQUAKE` (195) | `es.angles2[0]` = scale, `es.time` = duration ms, `es.angles2[1]` = radius (`CG_EntityPreEvent` case `0xc3`); `EV_FIRE_WEAPON_MG42` (168) starts a fixed one too. The shake: `cod11-combat.md` 17.2 |
| `EV_RAILTRAIL` (185) | `es.pos.trBase` = start, `es.origin2` = end, `es.attackerEntityNum` (120) = shooter, `es.dmgFlags` (220) = 2. Debug only (`g_debugBullets`). |
| `EV_OBITUARY` (201) | handler `0x3001d6c0`; field usage not decoded, out of scope for effects |

Events 140 to 143 (`EV_STANCE_FORCE_*`, `EV_STEP_VIEW`) and 149 are single-client:
the cgame prints
`"Event %s just for client %i was sent to other clients"` when
`es.clientNum != cg.clientNum`. 140-142 set `cl_stance` to 0, 1 or 2
(`cod11-mantle.md`, "The forced-stance events").

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

#### One effect per explode

- VERIFIED, `weapons/mp/fraggrenade_mp` (`pak0.pk3`): `projExplosionEffect`
  is empty, so a frag's explode plays only its `grenade_explode` row.
- VERIFIED, the committed 1.1d captures `net/snapshots-skyfrag.bin`,
  `-corpseblast.bin`, `-panzer.bin` and `-panzer-wall.bin`: each carries
  one missile whose ring holds an explode, and no temp entity carries a
  second.
- vcod: `play::replay::each_retail_explode_plays_one_effect` runs those
  captures through the event drain and the resolver, one explode played and
  one effect resolved per missile; `play::loopback::each_thrown_frag_explodes_once`
  throws two frags on vcod-server through the client's input and drain and
  plays two explodes. A frag flash that reads too bright and white is not a
  doubled effect; see the flash's shape and fade in `efx-grammar.md` R4, "`nonUniformScale` and `size2`, in detail" and "A
  sprite's vertex colour".

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
INFERRED, from `efx-grammar.md` R10: the flash's `relative` elements ride
that tag while they live; vcod bolts them to the shooter's muzzle entry.

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
`crates/common/src/net/cod1/mod.rs` documents the same base. Models are
268..523, shellshock 1100..1115, in the same function.

### 5d. Ejecting brass: `CG_EjectBrass` (`0x30038a30`)

The name is vcod's; the cgame prints none for it. It takes the entity's
`cent` and the event id.

- VERIFIED, cgame 0x30038a30: the function reads `DAT_301d896c` first,
  then `cent[1]` (`es.eType`) against 11 and `cent[0x32]` (`es.weapon`)
  against 0 and against the weapon count at `0x300eef38`, with the string
  `CG_FireWeapon: ent->weapon > BG_GetNumWeapons()` on the last.
  INFERRED, off those branches: it does nothing with `DAT_301d896c` 0
  (that is `cg_brass`, `cod11-front-end.md`, the Performance options), for
  an `eType` above 11 or for weapon 0.
  vcod has no `eType` check of its own: an event entity (`eType` 12 and
  up) carries no weapon model, so `ctx.brass` holds no `tag_brass` for its
  number and `fx::registry::eject_brass` plays nothing, the same outcome.
- VERIFIED: it reads two handles off the weapon table, the `+0x144` slot
  (`0x301a6a84 + w*0x198`) and the `+0x140` slot (`0x301a6a80`), and
  compares the event against `0xa1` (`EV_FIRE_WEAPON_LASTSHOT`). INFERRED,
  off the branches: `+0x144` plays when it is non-zero and the event is
  161, `+0x140` otherwise, and a zero `+0x140` plays nothing. INFERRED,
  from the weapon-file key order and the stock files: `+0x140` is
  `shellEjectEffect` and `+0x144` `lastShotEjectEffect`. VERIFIED,
  `pak0.pk3`: every stock MP bullet weapon sets `shellEjectEffect` to
  `fx/shellejects/pistol.efx` (colt, luger, mp40, ppsh, sten, thompson) or
  `fx/shellejects/rifle.efx` (the rest), writes `lastShotEjectEffect`
  empty, and the three `mg42_bipod_*` turrets, `PTRS41`, the panzerfaust
  and the grenades set neither.
- VERIFIED: syscall `0xdf` looks up `"tag_brass"` on either entity
  `0x400 + weapon` (the viewmodel) or `es.number`, and syscall `0xe5`
  plays the handle on that tag, as `CG_MuzzleFlash` does (5b). INFERRED,
  off the `ps.pm_flags & 0x50000` and `es.number == ps.clientNum` test
  (section 7): the ridden body's case leaves the viewmodel, anyone else's
  the weapon on the body; a negative tag index plays nothing.
- VERIFIED, callers: `CG_FireWeapon` (`0x30038b70`) calls it at
  `0x30038ce6` after a read of the weapon def's `boltAction` (`+0x2c8`);
  `CG_EntityEvent` case `0xa3` (`EV_EJECT_BRASS`, 163) calls it behind a
  test of the predicted-entity argument the fire events test, and
  `CG_EntityPreEvent` case `0xa3` calls it with no test. INFERRED: every
  fire event of a weapon without `boltAction` ejects a case (159, 160,
  161, 168, 169, 170), and a bolt-action's case comes only on 163, which
  the server raises for its rechamber and mid-reload (`cod11-combat.md`,
  the reload states).
- VERIFIED, `pak5.pk3`: both stock effects are two `Emitter` blocks. The
  first flies `xmodel/shellcasing` (`flags useModel usePhysics`, `life
  5000`, `cullrange 100`, `bounce 0.5`, `velocity 100 0 0`, `acceleration`
  +-132 per axis, `gravity -800`, random `angle`, `angleDelta` +-133 for the
  pistol and +-233 for the rifle, `size` 0.09 and 0.15). The second has no
  model (`flags usePhysics impactKills impactFx`, no `cullrange`, pistol
  `bounce 0.6`) and lists `impactfx fx/shellejects/shelleject_pistolsound`
  or `..._riflesound`, each a single `Sound` block naming the alias
  `shell_eject_pistol` or `shell_eject_rifle` (`soundaliases/iw_sound.csv`
  in `pak1.pk3`: `weapons/impact/pistol_shell.wav` / `rifle_shell.wav`,
  volume 0.4-0.7, distance 150-700). INFERRED: the case's clink is that
  unseen second particle's first hit, so it is one alias per calibre and
  never per surface type, and it is heard from a shooter past the model's
  100-unit `cullrange`. The flight, bounce and rest are the effect
  system's (`efx-grammar.md` R9).
- VERIFIED, `xmodel/shellcasing` in `pak0.pk3`: its bounds are +-9.78 on
  every axis, so the effect's `size` scales it to about 0.9 units (pistol)
  and 1.5 units (rifle) half-length.

vcod: `fx::registry::eject_brass` resolves the effect onto the posed
`tag_brass` (the viewmodel's for the ridden body, the held weapon's on
every drawn player), `fx::sim` flies it, and `cg_brass 0` resolves
nothing. Not modelled: the `eType` test, which no event vcod sees fails.

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
arg0 = `surfType`, arg1 = the string `"tag_flash"`. Both pass the event's own
`es.surfType`, and for 175/176 the server writes the literal 7 (section 2), so
INFERRED: a client hit always takes the flesh branch below. It returns at once when
`cg_tracerchance` (value at `0x301df448`) is `<= 0` or when `CG_CalcMuzzlePoint`
@ `0x30039440` cannot resolve `tag_flash` on the shooter entity. A shooter
that is the body the view rides skips the roll and the tracer and goes
straight to the whizby: VERIFIED, `test [snap+0x18],0x50000` at `0x300395c8`
and `cmp ebx,[snap+0xb8]` at `0x300395d1`, whose `je` lands on the whizby
call at `0x30039628`. Any other shooter rolls
`rand() < cg_tracerchance * RAND_MAX`. On success a flesh hit
(`surfType == 7`) calls the segment setup at `0x30039340` directly, and every
other surface calls `CG_SpawnTracer` @ `0x30038f30` with the muzzle point and
the impact position. INFERRED, from the branches above: in every case it then
calls `CG_BulletWhizby` @ `0x30038dc0`, which plays the whiz-by sound near the
`cg.refdef` origin.

`0x30039340` is the flesh branch's one-frame segment, drawn once at a random
distance along the shot. `CG_SpawnTracer` allocates a local entity with type 2
whose linear trajectory starts at the muzzle and moves at `cg_tracerSpeed`,
back-dates its start by up to half a frame, and lives until that point is
`cg_tracerlength` short of the impact; `CG_AddLocalEntities` draws the streak
from the trajectory point forward. `0x30039340` is not `CG_BloodSpray` and
not a setup shared by every tracer. The formulas and addresses for both
branches are in `docs/research/efx-grammar.md`, R8.
The tracer is drawn with the shader `gfx/misc/tracer`
(registered in `CG_RegisterGraphics` @ `0x30020da0`, handle stored at
`0x301d5abc`). Cvars: `cg_tracerchance` (value at `0x301df448`),
`cg_tracerSpeed`, `cg_tracerlength`, `cg_tracerwidth`.

The "viewing this entity first person" suppression is
`(cg.snap->ps.pm_flags & 0x50000) && shooterEnt == cg.snap->ps.clientNum`, see
section 7; `snap+0x18` and `snap+0xb8` are `ps+0xc` (`pm_flags`) and `ps+0xac`
(`clientNum`), INFERRED from the snapshot header ahead of `ps`. VERIFIED, the
routine reads no other flag and no cvar besides `cg_tracerchance`: no third
person, no sight test. So a playing client never sees its own tracers, and a
follow-spectator never sees the followed player's. vcod applies the same test
(`fx::registry::view_body`). The test never suppresses a 175/176: those reach only the victim's
`ps.clientNum` (section 2), and INFERRED, the shooter of the round is never the
player it hit, since a bullet trace skips its own shooter.

vcod draws tracers off `EV_BULLET_HIT_*`: `es.otherEntityNum` is the
shooter, the streak runs from that entity's muzzle to `es.pos.trBase`, gated
on a tracer-chance roll (`fx::registry`; the start point is below).

### Where a round starts: `CG_CalcMuzzlePoint`

The cgame's own name for 0x30039440 is `CG_CalcMuzzlePoint` (the string
`"No %s in CG_CalcMuzzlePoint on entity %d.\n"` it prints, VERIFIED). It
never reads the server's muzzle: the wire carries no shot origin on 1.1
or 1.5, so the start of a tracer and of the whizby is the client's own.
VERIFIED, disassembly and decompile, 1.1 0x30039440:

- The body the view rides (`pm_flags & 0x50000` and the shooter is
  `ps.clientNum`): `cg.snap`'s `ps.origin` with `viewHeightCurrent`
  (`ps+0xd0`) added to z, then `AddLeanToPosition` (0x3003f4e0) with
  `ps.leanf` (`ps+0x40`), 16 and 20, at `cg.refdefViewAngles[YAW]`
  (0x302095d0). INFERRED, off 0x30032540, which writes an angle into that
  float and caps a copy of the one before it at 45, as Q3's third-person view
  treats yaw and pitch: 0x302095cc..0x302095d4 are the refdef's view
  angles. No bob and no truncation. Only the whizby reads
  this point, since `CG_Tracer` draws no tracer for that shooter.
- Any other entity whose DObj exists (syscall 0xa2): its `tag_flash`
  (0x3001c2c0). No DObj returns 0, and `CG_Tracer` then plays neither
  tracer nor whizby.
- A DObj without the tag (a gunner, whose weapon `CG_Player` does not
  draw, `cod11-turrets.md`): `cent->currentState.pos.trBase` (`cent+0x18`,
  not `lerpOrigin`), and for an entity under 64 the warning above and z
  raised by the integer of `bg_viewheight_prone` (11) when `eFlags & 0x40`,
  else `bg_viewheight_crouched` (40) when `eFlags & 0x20`, else
  `bg_viewheight_standing` (60) (`fild` of 0x3019d06c, 0x3019dc4c,
  0x3019d7cc, the vmCvar rows at 0x30075170, 0x30075160, 0x30075150).

VERIFIED, 1.5 `cgame_mp_x86.dll` 0x3003ab90 (called from `CG_Tracer`
0x3003ace0): the same function, its view-body test on 1.5's `pm_flags`
0x30000, its DObj syscall 0xa3, the same three cvars (rows 0x30077160,
0x30077150, 0x30077140). VERIFIED, UO `uo_cgame_mp_x86.dll` 0x30048b60
(called from 0x30048d60): the same again on UO's playerstate
(`viewHeightCurrent` `ps+0xf8`, `pm_flags` 0xc0000, rows 0x30085a00,
0x300859f0, 0x300859e0), with one more step for the ridden body: when
the held weapon's `weaponClass` is 3 (`lmg`) and `pm_flags & 0x20` (the
bipod), the point moves along a vector 0x3004a200 builds, scaled by the
float at 0x3007c190, before the lean. INFERRED: that is the server's
19-unit bipod offset (`cod11-combat.md` 2.1); the vector and the constant
were not read. None of the three adds the walk bob or the 8-unit floor
the 1.5 and UO servers' `CalcMuzzlePoint` add (`cod11-combat.md` 2.1), so
the server's muzzle has no client counterpart.

The rest of the shot's client side is tags in all three: the flash on
`tag_flash` of the viewmodel or the world weapon (5b), the case on
`tag_brass` (5d), and on 1.5 the fire sound at the shooter's `tag_flash`
when it resolves (`CG_FireWeapon` 0x3003a2a0, the `0x1e4` flag, then
syscall 0xa3 and 0x3001d540 before the sound call 0x30022ed0; VERIFIED
decompile). No cgame of the three traces a player's round itself; UO's
client-traced vehicle and turret rounds start at their gun's tag
(`coduo-diff.md` 19, "Client-traced rounds"), and UO's `EV_BULLET_TRACER`
starts at the `trBase` the server sent, which is the server's muzzle
point (case 0x30023172, with no view-body test).

vcod: `fx::registry::shot_origin` starts the tracer and the whizby at the
shooter's posed `tag_flash` (`entities::BuiltScene::muzzles`, the gun's
own for a turret), else at `fx::registry::muzzle_fallback` for a drawn
player body without one (`BuiltScene::shot_fallbacks`); a body without
the tag plays no flash. For the ridden body vcod keys the viewmodel's
`tag_flash` instead of the eye-and-lean point: the whizby needs the
listener 64 units past the start along the shot (`cues::WHIZBY_MARGIN`),
and the listener is the view itself, so neither point ever whizzes
(INFERRED, off the margin test). The fire sound plays on the entity,
not the tag (not modelled).

---

## 7. Detecting spectator-follow state in `playerState`

VERIFIED: `ps.clientNum` differing from the client's own number means it is
following someone. The server side of the follow (buttons, cycle, let-go, disconnect,
live measurements) is `cod11-spectator-follow.md`.

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
followed player too. In free-fly spectate there is no copy and `ps.clientNum`
stays the client's own.

Flag summary, all from setters/testers in the server:

| Bit | Meaning | Evidence |
|---|---|---|
| `0x010000` | following a client | set in `SpectatorClientEndFrame` at `0x408ea`, which is past the exit taken when no client to follow was found (`0x40890`), so it marks a follow that succeeded rather than any spectator; tested by `StopFollowing` `0x46a51` |
| `0x020000` | a forced follow (script `spectatorclient >= 0`) as against free cycling | set/cleared on `spectatorClient >= 0`, `0x40903`/`0x408fd`. The preservation across the ps copy at `0x408c1` is an `eFlags` `0x20000`, not this bit |
| `0x040000` | live in-game player | set in `ClientEndFrame` `0x40fb0`, cleared for spectators `0x407b7`; `GetFollowPlayerState` `0x415dc` refuses to copy a player who does not have it |

The client's recurring test, `(ps.pm_flags & 0x50000) != 0 && es.number ==
ps.clientNum`, reads as "the view rides a player body (its own, `0x40000`, or
a followed one, `0x10000`) and this event came from that body": the
first-person suppression case.

With the client's own number from the connect handshake:

- `ps.clientNum != own_client_num` -> following that client.
- `(ps.pm_flags & 0x010000) != 0` -> spectator view of any kind.
- `(ps.pm_flags & 0x020000) != 0` -> following a specific client rather than
  free-flying.

The netfield table in `crates/common/src/net/cod1/fields_v1.rs` gives `pm_flags`
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

Only the table's address moved. The surrounding machinery is unchanged: the
1.5 MP cgame carries the same nine CSV impact-type keys
(`bullet_small_normal` ... `molotov_explode_reflect`), the same
`viewFlashEffect`/`worldFlashEffect` weapon keys and the same `tag_flash` tag,
and the 1.5 install ships the same `fx/iw_impacts.csv` in `main/pak5.pk3`.

The single-player modules also agree between 1.1 and 1.5 (198 entries, identical
order, table at `0x30067f20` in both `cgamex86.dll` files). The SP enum is a
different enum (see the top).

1.1 MP event constants hold unchanged for 1.5 MP. What differs per version
and is not surveyed here: the netfield tables (version-parameterised in
`crates/common/src/net/protocol.rs`) and the syscall numbers, internal to the
retail DLLs.

---

## 9. Impact marks

Bullet holes and scorches are not drawn by the particle system. An `.efx`
`Decal` element calls back into the cgame, which has the renderer clip a
square onto the world's triangles and pools every clipped triangle as one
mark. Addresses are `cgame_mp_x86.dll` 1.1 unless they say `CoDMP.exe`.

### 9.1 The call chain

- VERIFIED, `CoDMP.exe` 0x496896..0x4969b9: the effect runner's case 7
  (block type 7 is `decal`, `efx-grammar.md` R1) calls the cgame through
  `VM_Call(cgvm, 14, ...)` (`push 0xe` at 0x4969af, `call 0x460480`).
  The arguments, last pushed first: the shader the list at template
  `+0x68` picks (0x4969a4, `call 0x492780`), the effect's origin, its
  direction, `rotation` (template `+0x124`/`+0x128`), three colour
  floats, `alpha` (`+0x1e4`/`+0x1e8`), the constant 1, `size`
  (`+0x1fc`/`+0x200`), the constant 0, and `ftol(life)` (`+0x5c`/`+0x60`,
  `call 0x538be0` at 0x49696f). Each ranged key draws through 0x431440
  when its pair differs. INFERRED: the three colour floats are the start
  `rgb` the spawner drew before the switch (0x495015 area).
- VERIFIED: `vmMain` case 0xe (0x3002051d..0x3002054c) forwards the
  arguments to 0x30023560, Q3's `CG_ImpactMark`, with the direction in
  `ecx`: `(shader, origin, dir, orientation, r, g, b, a, alphaFade,
  radius, temporary, lifetime)`. So an effect's mark has alpha fade on,
  is permanent, and has `size` for its radius.
- VERIFIED, `CoDMP.exe` 0x4939e5..0x493a1f: the effect runner compares the
  view origin's distance to the effect against `cullrange` (template
  `+0x64`) before it spawns an element. INFERRED from the branch at
  0x493a1f: an element beyond its `cullrange` is not spawned, so a decal
  farther than that from the viewer (1200 units for
  `fx/impacts/default_hit.efx`) places no mark at all.

### 9.2 `CG_ImpactMark` (0x30023560)

- VERIFIED, 0x3002356a..0x30023586: it returns at once unless
  `cg_marks` is non-zero (the vmCvar at 0x301a6280, its integer at
  0x301a628c; cvar row 0x30074c80, default `"1"`, flags 1) and the
  no-marks flag 0x3020c9c0 is 0. `CG_EntityEvent` sets that flag for
  `EV_ROCKET_EXPLODE_NOMARKS` (0x3001e59f) and clears it after the
  effect (0x3001e64d).
- VERIFIED, 0x3002358c..0x30023597: a negative lifetime becomes 20000 ms.
- VERIFIED, 0x300235a5..0x300235d5 and the stores after: the basis is
  `axis[0]` the normalized direction (0x30039ef0), a perpendicular of it
  (0x3003a970 picks the axis the normal leans on least and projects it,
  Q3's `PerpendicularVector`), `axis[2]` from 0x3003a060 with the
  orientation, and `axis[1] = axis[0] x axis[2]`. INFERRED from the call
  shape: 0x3003a060 is `RotatePointAroundVector`, turning the
  perpendicular `orientation` degrees about the normal.
- VERIFIED: the square is `origin -+ axis[1]*r -+ axis[2]*r` in the order
  (-,-), (+,-), (+,+), (-,+), handed to syscall 0x2b
  (0x30023739..0x3002377e) with the origin, the basis, the radius, 0x400
  points, a point buffer, 0x180 fragments and a fragment buffer, and the
  shader.
- VERIFIED, 0x30023784..0x300237df: the colour bytes are `ftol(c * 255)`
  (0x30069384 is 255.0).
- VERIFIED, 0x300237f8 onwards: per returned fragment, its point count is
  clamped to 10; a temporary mark goes straight to syscall 0x40
  (`trap_R_AddPolyToScene`) with the fragment's own shader, a permanent
  one is allocated (0x300234d0) with `time = cg.time` (0x30207148), that
  shader (`+0xc`), alpha fade (`+0x10`), the four colour floats
  (`+0x14`..`+0x20`), the point count (`+0x28`), the points (`+0x30`,
  32 bytes each, the colour bytes at byte 28) and the lifetime (`+0x170`).

### 9.3 The mark pool and its fade

- VERIFIED, `CG_InitMarkPolys` 0x30023440: it zeroes 0x17400 dwords and
  links 0x400 entries of 0x174 bytes, so the pool holds 1024 fragments,
  not 1024 impacts. `CG_Init` (0x3002334a) and `CG_MapRestart` 0x3002ca60
  (0x3002cabe) both call it, so a `map_restart` clears every mark.
- VERIFIED, 0x300234d0: with the free list empty, it frees the active
  list's head and every entry after it with the same `time`, so a full
  pool drops the oldest impact whole. New marks go on the tail.
- VERIFIED, `CG_AddMarks` 0x300238f0: it returns at once when `cg_marks`
  is 0, so marks neither draw nor expire then. It walks from the tail
  (newest) back, frees a mark when `time + lifetime < cg.time`
  (0x30023930..0x3002393a), and when the time left is under half the
  lifetime (0x30069348 is 0.5) writes `fade = ftol(left * 255 /
  (lifetime / 2))` (0x300695c8 is 255.0): into every point's alpha byte
  with alpha fade on (0x300239b4), else `ftol(fade * colour)` into the
  three colour bytes (0x300239cf). It then adds the fragment through
  syscall 0x40.
- VERIFIED: `CG_PlayerShadow` 0x30027630 is the cgame's one other
  caller (0x3002778d); section 9.7.
- vcod (`marks::Mark::rgba_at`, `mark_pass`): the faded alpha reaches the
  screen through the mark shader's `alphaGen vertex`, which its `rgbGen
  vertex` implies (cod11-shader-scripts.md section 3). A parse that misses
  that leaves a scripted mark opaque until it is freed.

### 9.4 `R_MarkFragments` (`CoDMP.exe` 0x4e8db0)

Reached as syscall 0x2b: the cgame trap table 0x4041f0 sends it to
0x402727, which calls `re` slot `+0x98` (`re` at 0x1432860), which
GetRefAPI fills with 0x4e8db0 (VERIFIED, the store to 0xca2760).

- VERIFIED, 0x4e8db0 onwards: the gather box spans each corner, the
  corner pushed `radius` out along the normal and `20 * radius` in
  (0x5691a4 is -20.0). The clip planes are, per edge, `normalize((p[i+1]
  - p[i]) x axis[0])` at `dot(n, p[i])`, then `axis[0]` at
  `dot(axis[0], p[0]) - radius` and `-axis[0]` at `-dot(axis[0], p[0]) -
  radius`: the square's sides and a slab `radius` deep either side of its
  plane.
- VERIFIED: the gather walks the world's nodes (0x4e8c50, from world
  `+0x98`, BoxOnPlaneSide 0x430c80) to each leaf's cell (world `+0x124`,
  0x40 bytes each) and takes the surfaces of its cull groups (0x4e8a80,
  `+0x24`/`+0x28`, de-duplicated) and its AABB tree (0x4e8900, `+0x18`)
  whose bounds meet the box, at most 0x1000. Both skip a surface whose
  shader has bit 0x20 in its byte `+0x5c` (0x4e8b57), the `nomarks`
  surface flag (infoparm table row 0x571a60). INFERRED: `+0x5c` is the
  shader's surface flags; the dlight pass tests `sky|nodlight` there
  (0x500d66). INFERRED from the walk's roots: brush models and props take
  no marks, as their surfaces are not in the world's cells.
- VERIFIED, 0x4e931a: a surface whose first word is under 0x18 is
  skipped, the gate the world's surface add uses too (0x4e1772).
- VERIFIED, 0x4e934a..0x4e9375: unless the shader is the default one,
  for a surface whose shader has a lightmap index (`+0x40`) of 0 or more
  it calls 0x4fc5c0 with the mark shader, that index, 1 and 9, and the
  fragment carries the result's handle (`+0x44`). INFERRED: the mark
  shader is remade per lightmap page, so a mark's `$lightmap` bundle
  samples the surface's own lightmap.
- VERIFIED: per triangle, the normal is `normalize((v0 - v1) x (v2 -
  v1))`, and only a triangle with `dot(normal, axis[0]) > 0.5`
  (0x568e70) is clipped. The clip input carries position and lightmap
  coordinates (5 floats a point); 0x4e8cf0 chops it plane by plane
  through 0x4e86a0 with epsilon 0.5, and copies the result into the point
  buffer at 32 bytes a point.
- VERIFIED, 0x4e86a0: Q3's `R_ChopPolyBehindPlane`. A point more than
  the epsilon in front is front, more than it behind is back, else on;
  no front point drops the polygon, no back point keeps it whole, more
  than 0x3d points drops it; on points are kept and a front/back edge is
  cut at `d0 / (d0 - d1)` (0 when the two match), every coordinate
  interpolated.
- VERIFIED: a kept fragment's texture coordinates are `dot(p - origin,
  axis[1]) * (0.5 / radius) + 0.5` and the same on `axis[2]`. Each
  clipped triangle is its own fragment. It returns once the fragment
  count reaches the cgame's maximum or the points used pass the maximum
  minus 3.

### 9.5 The mark shaders and where they sort

- VERIFIED, `pak5.pk3 : fxshaders/pj_impact.shader`: the stock decals
  (`gfx/impact/bullethole1`, `bullethit_wood1`, ...) are `polygonOffset2`
  blocks with a `blendFunc blend`, `rgbGen vertex` stage whose second
  bundle is `$lightmap`, plus a `perlight` stage.
- VERIFIED, `CoDMP.exe` 0x4fad60..0x4fadc9: `polygonOffset`,
  `polygonOffset2` and `polygonOffsetConst` set shader flags 0x10, 0x20
  and 0x40, each clearing the other two. VERIFIED, 0x4fba4d: any of them
  sorts an unsorted shader 6 (`decal`).
- VERIFIED, `CoDMP.exe` 0x4d5991..0x4d5a05: the draw state's offset
  branch calls `glPolygonOffset(r_offsetfactor, r_offsetunits)` for state
  bit 0x2000, the doubled pair for 0x4000 and `(0, r_offsetunits)` for
  0x8000; `r_offsetfactor` and `r_offsetunits` register `-1` and `-2`
  (0x4b3f0a..0x4b3f33).
- VERIFIED, `CoDMP.exe` 0x4fbbdd..0x4fbc60: shader flag 0x20 sets state
  bit 0x4000 in the stage's `+0x684`, 0x10 sets 0x2000 and 0x40 sets
  0x8000. So `polygonOffset` draws at (-1, -2), `polygonOffset2` at (-2,
  -4) and `polygonOffsetConst` at (0, -2).
- VERIFIED, `CoDMP.exe` 0x4faf29..0x4faf39: `nofog` sets shader flag
  0x400. INFERRED: the fog pass skips such a shader, as Q3's does.
- INFERRED: a mark reaches the draw list as a poly with its lightmapped
  shader, so it sorts by that shader's sort (6) among the world's
  surfaces.

### 9.6 What vcod implements

`crates/client/src/marks.rs` holds the clip, the basis, the pool and the
fade; `crates/client/src/renderer/mark_pass.rs` draws them; the `Decal`
element hands its request over in `fx::sim::FxSystem::take_marks`. The
world's mark triangles are model 0's drawn soups without `nomarks` in
their BSP surface flags. Marks draw through shader.wgsl's stage shader
with the surface's lightmap page, one material per (shader, lightmap), in
the frame's sorted list at the shader's sort, after the world's draws of
that sort and before the models and effects (vcod's guess). Each mark
stage's pipeline carries the stage's exact blend pair and the shader's
own polygon offset (9.5), so the stock decals draw at (-2, -4). The
world's stage pipelines still draw any of the three offsets at (-1, -2);
no stock world shader uses the other two. `cg_marks`,
`EV_ROCKET_EXPLODE_NOMARKS`, `cg_shadows 1` (9.7) and the `map_restart`
clear (9.8) are honoured.

The marks' `perlight` stages draw nothing, in retail as in vcod:

- VERIFIED, `pak5.pk3 : fxshaders/pj_impact.shader`: no mark shader there
  carries a `sort`, so each sorts 6 off its `polygonOffset2` (9.5).
- VERIFIED, `CoDMP.exe` 0x500d3a (`cod11-shader-scripts.md` 3): the
  dlight pass runs only for a shader sorted at most 5.0; past that the
  `perLight` stages draw once per light of the current entity's list
  (`[0x16d8e5c] + 0xcc`).
- VERIFIED, `CoDMP.exe` 0x4d6710 (the draw-surface loop): a surface whose
  sort key has 0x2 set, or entity 0x3fe, points 0x16d8e5c at the world
  entity 0x16c508c. INFERRED: a scene poly (a mark) draws as the world
  entity, as Q3's `R_AddPolygonSurfaces` does. INFERRED: no instruction
  in `CoDMP.exe` stores to 0x16c5158 (`0x16c508c + 0xcc`) by its address
  (a search of the disassembly), so that list is empty and a mark's
  `perlight` stage never draws.

Not implemented: the surface-type gate at 0x4e931a, since vcod has no
field it maps to.

### 9.7 `markShadow`: `CG_PlayerShadow` (0x30027630)

- VERIFIED, `CG_Player` 0x30028210: it returns without drawing for an
  entity whose `eFlags` has 0x100 (`EF_NODRAW`) or 0x1 (`EF_DEAD`) set,
  or whose client has no model (syscall 0xa2 returns 0), and otherwise
  calls 0x30027630 at 0x300283c8. 0x30027630 has no other caller, and
  the entity dispatch 0x3001d5f0 sends only `eType` 1 to `CG_Player`
  (corpses, `eType` 2, go to 0x30028400). So only a drawn live player
  casts a blob; a corpse never does, whatever its stance.
- VERIFIED: `cg_shadows` is the cgame's vmCvar 0x301d9380 (table row
  0x30074a20, name 0x30064950, default `"0"`, flags 0x201, archived and
  cheat-protected); the function reads its integer at 0x301d938c. At 0
  it returns at once. Any other value runs the trace below; only 1
  places a mark (0x300276d8). Retail's default draws no blob.
- VERIFIED, 0x3002764c..0x3002768f: a point trace through syscall 0x26
  from the entity's `lerpOrigin` (`cent+0x1f8`) to 64 units below it
  (0x300695a0 is 64.0), mins and maxs null, mask 0x2810011. INFERRED,
  `CoDMP.exe` 0x40266d: syscall 0x26 is `CM_BoxTrace` (0x424b50) with the
  cgame's model argument 0, so it hits the world's brushes only, no
  entity.
- VERIFIED, 0x30027695..0x300276bc: a fraction of exactly 1.0 or 0.0
  places nothing. The shadow plane written back to the caller is the
  hit's z plus 1 (0x30069304).
- VERIFIED, 0x300276e0: an entity with `eFlags & 1` at `cent+8` places
  nothing.
- VERIFIED, 0x300276e9..0x3002775d: the distance from the entity to
  `cg.snap->ps.origin` (`[0x301e2160] + 0x20`, through 0x30039b70) sets a
  grey level: none at 250 or nearer (0x30069720), `(d - 250) / 262` below
  512 (0x30069718, 0x30069710), `1 - (d - 512) / 512` from 512 to 1024
  (0x30069700, 0x30069708), none past 1024. That level times `1 -
  fraction` is the mark's colour. So a player close to the viewer, the
  viewer's own body and a followed player's included, casts no blob.
- VERIFIED, 0x30027761..0x3002778d: the call is `CG_ImpactMark(markShadow
  (0x301d5d4c), trace endpos, trace normal, orientation 0, grey, grey,
  grey, alpha 1.0, alphaFade 0, radius 16, temporary 1, lifetime -1)`.
  The temporary branch (9.2) adds each fragment through syscall 0x40 the
  same frame and pools nothing; `cg_marks 0` stops it with every other
  mark.
- VERIFIED, `pak4.pk3 : scripts/gfx.shader`: `markShadow` is `nofog`,
  `polygonOffset`, one stage `map textures/sfx/shadow_soft.tga`,
  `blendFunc GL_ZERO GL_ONE_MINUS_SRC_COLOR`, `rgbGen exactVertex`. The
  grey level darkens the ground by the image times that level.

vcod: `crates/client/src/blob_shadow.rs` runs the trace and the fade per
drawn player (`entities::BuiltScene::shadow_casters`), with
`CollisionWorld::point_trace` without props standing in for the world
trace. The marks go into the frame's mark list ahead of the pooled marks.
INFERRED from `CG_DrawActiveFrame`'s Q3 order: the packet entities, and so
the shadows, are added before `CG_AddMarks`.

### 9.8 `map_restart`: `CG_MapRestart` (0x3002ca60)

The `n` server command runs it (`cod11-hud-protocol.md`, server command
table).

- VERIFIED, in order: 0x3001fec0 frees every local entity (it zeroes
  0x1d80 dwords and relinks the free list; its neighbour 0x3001ff50 prints
  `CG_FreeLocalEntity: not active`); 0x30023440 frees every mark (9.3);
  syscall 0xe9; syscall 0xd3 with 0; 0x30021b30 restarts the ambient from
  configstring 3 (`cod11-sound-system.md`); it zeroes the view state
  words below, sets `cg_thirdPerson` 0, `cl_stance` 0, `cl_run` 1, closes
  the script menus (0x3002de60, twice), and clears `cg_objectiveText`.
- VERIFIED, `CoDMP.exe` trap table 0x4041f0: syscall 0xd3 goes to
  0x40278f, `S_StopAllSounds` 0x44fa40, which with 0 ends every 2D and 3D
  sample and every stream. Syscall 0xe9 goes to 0x404151, which calls
  0x49e810(0). INFERRED: 0x49e810 frees every live effect: it destroys
  the objects listed at 0xc9ce60 and 0xc9ce6c.., then runs 0x48da60 and
  0x492c80 on 0x14075a0, the effect system that syscalls 0xe4 and 0xe5
  spawn through (0x493870).

- VERIFIED, United Offensive's `CG_MapRestart` (uo cgame 0x30039500,
  string `CG_MapRestart\n`): the same order. 0x3002a9e0 frees the local
  entities, 0x3002e400 the marks, syscall 0xed the effects, syscall 0xd7
  with 0 the sounds, 0x3002c9c0 restarts the ambient from configstring 3,
  then `cg_thirdPerson` 0, `cl_stance` 0, `cl_run` 1 and, for an argument
  of 0 only, the script menus; `cg_objectiveText` empties last. `B` calls
  it with 0, `n` with 1 (0x3003ac90; `coduo-diff.md`, "`map_restart`'s
  letter"). INFERRED: none of those callees touches the flamethrower's
  chunk pool, so a burning flame outlives a UO restart.

The words `CG_MapRestart` writes, 1.1 cgame (VERIFIED, decompile, each
address's other references read for what it is):

| Address | Write | What it is |
|---|---|---|
| 0x3020c488 | 0 | the centre print's time; `CG_PriorityCenterPrint` (0x300159e0) stores `cg.time + 2000` there and the draw ends the print at 0 (`cod11-hud-protocol.md`) |
| 0x3020c918 | 0 | the pickup time `0x3001da80` stamps; nothing draws it |
| 0x3020c8ec | 0 | the cursor hint's fade length; 0x30016f10 stamps it from a cvar with `cg.time` at 0x3020c8e8 and the hint at 0x3020c8e4 while the playerstate's `+0x384` is not 0 |
| 0x301d64d0, 0x301d64d4 | -1, 0 | the teamkill complaint's client and end (`cod11-votes.md`) |
| 0x301d21c0 | 0 | `cgs.voteTime`, read from the vote configstring at 0x30022f10; the vote draw (0x30018120) shows nothing at 0 |
| 0x30207154 | 1 | `cg.mapRestart`: the snapshot transition (0x3002fc80, 0x3002ffda..0x300300b8) runs the respawn reset 0x30028a70 when it is set whatever the spawn count, and 0x30028a70 clears it |
| 0x3020d098 | 0 | its only reference in the module; not identified |
| 0x3020c9e0 | 0 | the red damage flash's end (`cod11-hud-protocol.md`, "Damage direction") |
| 0x3020c948.. | 0x18 dwords | the eight hit direction icons, three dwords each (0x300287f0) |
| 0x3020c92c.., 0x3020c944 | 6 dwords, 0 | the weapon strip's slot scales and its last draw time (`cod11-hud-protocol.md`) |
| 0x3020c030, 0x3020c038 | 0, `cg.time` | a held scoreboard lets go and fades (`cod11-hud-protocol.md`) |

Not identified: trap 200 with 1 and 0x3000ef10, taken only when
`sv_running` read non-zero at init (0x301d1fd8, 0x300205e0); trap 0x7c;
trap 0xf1 last.

**The respawn reset.** VERIFIED, 0x30028a70, run from the two first
snapshot setups (0x3002fa8f, 0x3003037f) and from the snapshot
transition on a new `stats[5]` (snapshot `+0x114`), a new `clientNum`
(`+0xb8`) or `cg.mapRestart` (combat doc 9.8, hud doc "Weapon name and
ammo"): it clears `cg.mapRestart`, copies the snapshot's playerstate to
the predicted one (0x3020715c, 0x834 dwords), takes the weapon file
pointer (0x30209484), stamps the weapon name (0x3020c920), and zeroes
the cursor hint (0x3020c8e4, 0x3020c8e8), 0x3020cbdc (one of the 2D
pass's three skip flags, hud doc "The weapon-slot strip"), the view
weapon's sway, tilt, idle factor and movement position
(0x302095d8..0x302095f8, 0x30209454..0x30209480, 0x3020c9f0; combat doc
17.4), the recoil (0x3020cb2c..0x3020cb40), the damage kick
(0x3020c9a8, 0x3020c9e4, 0x3020c9e8), the gun spring
(0x3020cb94..0x3020cba8), the hit icons (0x3020c948, 0x18 dwords), the
quakes (0x3020cff4, 0x24 dwords) and 0x3020948c..0x30209494; then it
sets `cg_weaponSelect` to the playerstate's weapon, `cl_stance` 0 and
`cl_run` 1. It leaves the damage flash's end (0x3020c9e0) alone.

vcod's client runs its part on `n` and `B` (`play::join::is_map_restart`):
`FxSystem::clear` (live particles, lights and pending decals),
`Renderer::clear_marks`, `AudioSystem::stop_all`, the ambient from CS 3 and
the script menu rearm. The cvar resets go to the
`v` cvars (`ClientCvars::on_map_restart`: the scoreboard's objective text
empties), `cl_run` and the input's stance, which stands. The HUD
(`Hud::on_map_restart`) drops the centre print, the complaint, the vote
lines (`Vote::on_map_restart`), a held scoreboard, the weapon strip's
scales and clock, the hit icons and the flash. The view, the input and
the HUD forget the spawn count (`OnlineView::map_restart`,
`PlayInput::map_restart`, `Hud::spawn`), so the next snapshot runs the
respawn reset, as `cg.mapRestart` does on retail. vcod has no local
entities and no third person view; `fx::flame` keeps its chunks, as UO's
does. The cursor hint draws straight off the playerstate in vcod, with
no fade to clear.

vcod's respawn reset runs on a new `(clientNum, stats[5])`, the first
snapshot of a gamestate and the first after a restart: the selection,
`cl_run` and the stance (`PlayInput::sync`), the recoil and the quakes
(`OnlineView::track_spawn`), the hit icons and the gun's sway, tilt,
movement and damage kick (`Hud::track_spawn`, `GunAim::respawn`), and
the weapon name's stamp (`WeaponNameFade`).

---

## Open items

None. Whether a follow-spectator receives 175/176 for the followed player is
settled in section 2: the single-client test compares the snapshot's
`ps.clientNum`, not the client slot. A free-flying spectator's capture never
shows them.
