# CoD 1.1 MP: mounted MGs

How a `misc_mg42` works on a stock 1.1 MP server: where the maps place them,
what the weapon file feeds, what the spawn builds, how the use key mounts a
player, what pmove does to a mounted player, the per-frame aim, fire and loop
sound, how the body is put on the gun, what lets go, what the unmanned barrel
does, which anims play and how a kill is credited.

Evidence rules as everywhere in this directory. This document carries no
document-level default: every claim carries its own label. VERIFIED is a byte
read out of the module, an asset or a committed capture (an immediate, a
relocation, a string, a store at an offset, a table entry). INFERRED is
anything read off control flow, which includes the order of two stores and
every branch condition, and anything that says what a field means. No retail
capture of a mount exists yet; section 12 is where one goes.

Module: `game.mp.i386.so`, the 1.1d Linux dedicated server's MP game module.
Addresses are the module's own VAs as `nm -D`, `objdump` and
`python3 tools/re/annotate_func.py <elf> <symbol|0xaddr>` print them. A Ghidra
export that loads the module at +0x10000 shows each function 0x10000 higher
(`turret_use` 0x52a9c reads `00062a9c` there). The module is
position-independent, so a call or data reference is only readable with its
relocation resolved; `annotate_func.py` does that. Six statics carry no
dynamic symbol: 0x51488 (the fire parameters), 0x515a8 (body placement),
0x5201c (aim), 0x521d4 (the mounted frame), 0x52880 (the arc test) and 0x524cc
(the unmanned slew); the names here describe what the bytes do.

Offsets used throughout. `ent` is a gentity (stride 0x314), `cl = ent+0x158`
is its client, the playerstate sits at `cl+0`, and `rec = ent+0x15c` is a
turret's record.

| offset | meaning | evidence |
|---|---|---|
| `ent+0x8` | `s.eFlags` | VERIFIED, entity netfield offset 8 |
| `ent+0x30` | `s.apos.trType` | VERIFIED, entity netfield offset 48 |
| `ent+0x68` / `+0x6c` / `+0x70` | `s.angles2[0..2]`: barrel pitch, barrel yaw, flash pitch | VERIFIED, entity netfield offsets 104/108/112; the meanings are INFERRED from `turret_controller` (section 9) |
| `ent+0x74` | `s.otherEntityNum` | VERIFIED, entity netfield offset 116 |
| `ent+0x84` | `s.loopSound` | VERIFIED, entity netfield offset 132 |
| `ent+0xc8` | `s.weapon`, the configstring 7 index | VERIFIED, entity netfield offset 200 |
| `ent+0x134` / `+0x140` | `r.currentOrigin` / `r.currentAngles` | VERIFIED, entity field table `origin` 308 and `angles` 320 (`cod11-gsc-object-model.md`) |
| `ent+0x14c` | `r.ownerNum`, 0x3ff for none | VERIFIED, `turret_use` 0x52ac4 and the release stores 0x3ff at 0x524b1; the name is INFERRED |
| `ent+0x15c` | the turret record pointer | VERIFIED, `G_SpawnTurret` 0x52cdc |
| `ent+0x171` | `takedamage` | VERIFIED, `G_SpawnTurret` 0x5301a stores 1; the name is INFERRED (`cod11-map-cycle.md`) |
| `ent+0x172` | the busy byte: 1 on a mounted player and on its turret, 2 a dismount request | VERIFIED, the stores in `turret_use` 0x52ab1/0x52ab8 and `Cmd_Activate_f` 0x484a6; the meaning is INFERRED |
| `ent+0x1fc` / `+0x200` | nextthink / think | VERIFIED, `G_SpawnTurret` 0x52ff9 / 0x52fe7 |
| `ent+0x210` | use callback | VERIFIED, `G_SpawnTurret` 0x53009 stores `turret_use`; `Cmd_Activate_f` calls through it at 0x485d6 |
| `ent+0x220` | DObj controller callback | VERIFIED, `G_SpawnTurret` 0x52fff stores `turret_controller` |
| `ent+0x230` / `+0x238` | `health` / `dmg` | VERIFIED, entity field table offsets 560 and 568 |
| `ent+0x28c..0x294` | a copy of `r.currentAngles` taken at mount | VERIFIED, `turret_use` 0x52ba8..0x52bc6 |
| `cl+0xc` | `ps.pm_flags`; 0x1 prone, 0x2 crouch | VERIFIED, player netfield offset 12; bit names from `docs/protocol-1.1.md` |
| `cl+0x34` | `ps.grenadeTimeLeft` | VERIFIED, player netfield table |
| `cl+0x54` | `ps.groundEntityNum` | VERIFIED, player netfield table |
| `cl+0x80` | `ps.eFlags`; byte `+0x81` holds 0x4000 and 0x8000 | VERIFIED, player netfield offset 128 |
| `cl+0xc0` | `ps.viewangles` | VERIFIED, player netfield table |
| `cl+0x374` / `+0x378` / `+0x380` | `ps.viewlocked` (8 bits) / `ps.viewlocked_entNum` (16 bits) / `ps.gunfx` (8 bits) | VERIFIED, player netfield offsets 884, 888 and 896 in `crates/common/src/net/fields_v1.rs` |
| `cl+0x20d0` | `sessionstate` | VERIFIED, `cod11-map-cycle.md`; the name is INFERRED |
| `cl+0x21e8` | the last cmd's buttons byte | VERIFIED, `cod11-gsc-object-model.md` 23.5 |

The turret record: 32 of them, 0x40 bytes each, array at 0xaa180.
VERIFIED: `G_SpawnTurret` walks `0xaa180` in 0x40 steps comparing each first
dword against zero, holds a bound of 0x20 and `Com_Error(1, "G_SpawnTurret:
max number of turrets (%d) exceeded")` (string 0x75960, call 0x52cc3), and
calls `__bzero(rec, 0x40)` at 0x52cd1. INFERRED: a 33rd turret is a fatal
error. VERIFIED: `G_InitTurrets` (0x53034) zeroes
the 32 first dwords, and `G_InitGame` calls it (0x4fe18).

| off | written by | meaning | evidence |
|---|---|---|---|
| `+0x00` | spawn 1, `G_FreeTurret` 0 | in use | VERIFIED, the stores |
| `+0x04` | spawn 3 (0x52f39); `turret_use` `\|= 0x800` (0x52aee); the aim step and the release clear 0x800 | flags | VERIFIED, the stores; 0x800 as "mounted this frame, not yet aimed" is INFERRED |
| `+0x08` | spawn 0 (0x52d78); the mounted frame | fire cooldown, ms | VERIFIED, the stores; the unit is INFERRED from the 50 decrement |
| `+0x0c` / `+0x14` | spawn | pitch min / max | VERIFIED, arithmetic 0x52e89..0x52edc |
| `+0x10` / `+0x18` | spawn | yaw min / max | VERIFIED, arithmetic 0x52e0e..0x52e61 |
| `+0x1c` | spawn -90, `turret_think_init` | rest pitch | VERIFIED, `docs/protocol-1.1.md`, "A turret's `angles2[0]` is where its barrel came to rest" |
| `+0x20` | spawn, from weapon def `+0x80` (0x52d85) | gun stance: 0 stand, 1 duck, 2 prone | VERIFIED, the load; the enum order is VERIFIED from the `.data` pointer table at 0x7c958..0x7c960 (`stand`, `duck`, `prone`, each through an `R_386_RELATIVE`) |
| `+0x24` | spawn -1 (0x52d88); `turret_use`; release -1 | the player's stance at mount: 2 prone, 1 crouch, 0 stand, -1 none | VERIFIED, the stores; the meaning is INFERRED |
| `+0x28` | spawn 0 (0x52d8f); fire `3 * fireTime` | loop-sound timer, ms | VERIFIED, the stores |
| `+0x2c..0x34` | `turret_use` | the player's origin at mount | VERIFIED, 0x52af8..0x52b0a |
| `+0x3c` / `+0x3d` | spawn | loop / stop sound alias index, one byte each | VERIFIED, `G_SoundAliasIndex` results stored at 0x52db1 / 0x52ddc |

## 1. Placements

VERIFIED, the entity lump of every `maps/MP/*.bsp` in pak4 and pak5: ten
mounted guns on six maps, every one `classname misc_mg42`, `model
xmodel/mg42_bipod`, `weaponinfo mg42_bipod_stand_mp`, plus `origin`, `angles`
and on all but one `export`. No map uses `misc_turret`. No map sets
`leftarc`, `rightarc`, `toparc`, `bottomarc`, `damage`, `health`,
`targetname`, `target`, `spawnflags` or any `script_*` key on a turret.

| map | lump index | origin | angles |
|---|---|---|---|
| mp_brecourt | 187 | 591 218 73 | 0 162 0 |
| mp_carentan | 853 | -500 1896 175 | 0 295 0 |
| mp_carentan | 855 | 1712 1830 8 | 0 229 0 |
| mp_dawnville | 330 | -1713 -18308 94 | 0 90 0 |
| mp_dawnville | 331 | 83 -16256 42 | 0 180 0 |
| mp_hurtgen | 19 | 3324 626 -127 | 0 270 0 |
| mp_hurtgen | 1563 | 1448 -3906 -193 | 0 169 0 |
| mp_hurtgen | 1608 | 1829 -2007 -196 | 0 180 0 |
| mp_railyard | 354 | -495 1736 34 | 0 315 0 |
| mp_rocket | 596 | 11800 6129 435 | 0 225 0 |

VERIFIED, same lumps: chateau, depot, harbor, pavlov, powcamp and ship have
none. The `script_model`s carrying `xmodel/turret_flak88_static_*` (brecourt,
carentan, harbor, hurtgen) are bombzone and exploder props, and the
`xmodel/ship_*_turret` models on mp_ship are `misc_model`s.

**The lump index is not the entity number.** VERIFIED, the committed retail
capture `crates/server/tests/fixtures/entities/mp_carentan-dm.txt`: carentan's
two `eType` 11 entities are **297** (`pos.trBase` -500 1896 175, yaw 295,
`angles2[0]` -63) and **298** (1712 1830 8, yaw 229, `angles2[0]` -72), with
`index` 58 and `weapon` 16 on both, in every sample of that file. So the gun a
probe mounts on carentan at 1712 1830 8 goes on the wire as entity 298, and
`viewlocked_entNum` and `otherEntityNum` read 298, not 855. An earlier survey
and the first version of the design spec called the two turrets #853 and
#855, which are their positions in the entity lump.

VERIFIED, a case-insensitive grep of every `maps/MP/*.gsc` and
`maps/MP/gametypes/*.gsc` in the stock paks for `turret`, `mg42`, `misc_mg`,
`usable`, `weaponinfo`, `mounted`, `useby`: no match. VERIFIED,
`tools/re/dump_builtins.py` over the five MP builtin tables: no turret method
(`setTurretTargetVec` and the rest are SP). VERIFIED: `_teams.gsc`
`restrictPlacedWeapons` (line 463) deletes only `mpweapon_*` classnames.
INFERRED: the whole feature is engine code, and no stock cvar or script turns
a mounted gun off.

## 2. The weapon file

VERIFIED, `weapons/mp/mg42_bipod_stand_mp` (pak0), the keys a turret reads:
`weaponClass turret`, `weaponType bullet`, `stance stand`, `damage 60`,
`leftArc 45`, `rightArc 45`, `topArc 40`, `bottomArc 40`, `fireTime 0.05`,
`loopFireSound weap_mg42_loop`, `stopFireSound weap_mg42_cooldown`,
`animHorRotateInc 15`, `useHintString CGAME_USEMG42`. VERIFIED, the keys
nothing in the turret code reads: `accuracy 0.5`, `horTurnSpeed 40`,
`vertTurnSpeed 40`, `convergenceTime 1.5`, `maxRange 0`,
`playerPositionDist 46`, `rifleBullet 1`, `script mg42/stand`, `idleAnim
standMG42gun_aim_foward`, `fireAnim standMG42gun_fire_foward`,
`viewFlashEffect`, `worldFlashEffect`, `reticleCenter`, `killIcon
gfx/hud/hud@death_mg42.tga`. VERIFIED: the file has no ammo, clip, overheat,
`worldModel`, `viewModel` or `reloadTime` key.

VERIFIED, the duck and prone files ship in pak0 and are in configstring 7
(`cod11-server-handshake.md`): `mg42_bipod_duck_mp` is the stand file with
`stance duck` and `topArc`/`bottomArc` 30; `mg42_bipod_prone_mp` has `stance
prone`, arcs 45/45/20/20, `animHorRotateInc 20`, `horTurnSpeed 20`,
`vertTurnSpeed 100`, `script mg42/prone` and the `proneMG42gun_*_medium`
anims. No stock map places either (section 1).

VERIFIED, the weapon field table's `{name, offset, type}` records
(0x7c994..0x7d534; type codes in `cod11-combat.md`), for the fields above:

| key | def offset | type |
|---|---|---|
| `weaponType` | `+0x70` | 8 (enum) |
| `weaponClass` | `+0x74` | 9 (enum) |
| `stance` | `+0x80` | 12 (enum) |
| `loopFireSound` / `stopFireSound` | `+0xa0` / `+0xa4` | 0 (string) |
| `damage` | `+0x1c0` | 4 (int) |
| `fireTime` | `+0x1d4` | 7 (time) |
| `rifleBullet` | `+0x2c0` | 5 (bool) |
| `leftArc` / `rightArc` / `topArc` / `bottomArc` | `+0x3dc` / `+0x3e0` / `+0x3e4` / `+0x3e8` | 6 (float) |
| `accuracy` | `+0x3ec` | 6 |
| `vertTurnSpeed` / `horTurnSpeed` | `+0x3f0` / `+0x3f4` | 6 |
| `convergenceTime` / `maxRange` | `+0x3f8` / `+0x3fc` | 6 |
| `animHorRotateInc` / `playerPositionDist` | `+0x400` / `+0x404` | 6 |
| `useHintString` | `+0x408` | 0 |
| `script` | `+0x410` | 0 |

INFERRED: `fireTime` is held in milliseconds, so the stock gun's reads 50
(`cod11-combat.md`, the type-7 reading).

VERIFIED: `BG_SetupWeaponInfo` (0x36674) walks every weapon, and for each
whose `useHintString` is non-empty calls `G_GetHintStringIndex(def+0x40c,
def->useHintString)` (0x369f5), with `Com_Error(1, "Too many different
hintstring values on weapons. Max allowed is %i different strings", 0x20)`
(string 0x72360) on a zero return. INFERRED: `def+0x40c` holds the slot in the
32-entry hint-string range at configstring 1212 (`cod11-gsc-object-model.md`,
the hint-string paragraph), so the stock turret's is slot 0. VERIFIED:
retail's carentan configstring 1212 reads `CGAME_USEMG42`
(`cod11-server-handshake.md`).

VERIFIED, `soundaliases/iw_sound.csv` (pak1): `weap_mg42_loop` names
`weapons/mg42/mg42_loop02.wav` and loops, and `weap_mg42_cooldown` names
`weapons/mg42/mg42_cooldown.wav`; each has two rows split on the map
(`! Pavlov` / `pavlov`). VERIFIED: those are carentan's configstrings 526 and
527 (`cod11-server-handshake.md`). VERIFIED,
`localizedstrings/english/cgame.str`: `CGAME_USEMG42` is `"Press [%s] to use
the MG42"`.

## 3. Spawn

VERIFIED, `spawns` (0x7eb30): `misc_mg42` and `misc_turret` both name
`SP_turret` (0x533b0). VERIFIED: `SP_turret` calls `G_SpawnString("weaponinfo",
"", &s)`, holds `Com_Error(1, "no weaponinfo specified for turret")` (string
0x75a40) and calls `G_SpawnTurret(ent, s)`. INFERRED: a turret without the key
is a fatal map-load error.

VERIFIED, `G_SpawnTurret` (0x52c84), the stores and calls:

- the record allocation above, `ent+0x15c = rec` (0x52cdc) and `rec+0 = 1`;
- `s.weapon = BG_GetWeaponIndex(weaponinfo)` (0x52cec), with
  `Com_Error(1, "bad weaponinfo '%s' specified for turret")` (0x759a0) beside
  it;
- `BG_GetInfoForWeapon` (0x52d21), a compare of `level+0x1c` against 0
  (0x52d2b), `IsItemRegistered` (0x52d3e) and `Scr_Error` on `"turret '%s'
  not precached"` (0x759ca, 0x52d5f), then `RegisterItem(weapon, 1)`
  (0x52d73);
- `rec+0x8 = 0`, `rec+0x20 = def->stance`, `rec+0x24 = -1`, `rec+0x28 = 0`
  (0x52d78..0x52d8f), and the two alias bytes from `G_SoundAliasIndex` on
  `def+0xa0` and `def+0xa4`, or 0 (0x52db1..0x52de4);
- `G_SpawnFloat` on `rightarc` (0x759e5), `leftarc` (0x759ee), `toparc`
  (0x759f6) and `bottomarc` (0x759fd), each with the empty default
  (0x759e4), and loads of the weapon file's `rightArc`, `leftArc`, `topArc`
  and `bottomArc` beside them;
- a compare of `health` (`ent+0x230`) against 0 (0x52ee7) and a store of 100
  (0x52ef0);
- `G_SpawnInt("damage", "0", &ent->dmg)` (key 0x75a09, 0x52f0e), a store of
  the weapon's `damage` into `ent+0x238` (0x52f20), and a compare against 0
  with a store of 0 (0x52f26, 0x52f2f);
- `rec+0x4 = 3`, `ent+0x190 = 1`, `r.contents = 0x200004`, `ent+0xf4 = 0x80`,
  `s.eType = 11`, `ent+0x17d |= 0x20` (0x52f39..0x52f65), `G_DObjUpdate`;
- `r.mins = (-32, -32, 0)`, `r.maxs = (32, 32, 56)` (rodata 0x75a14, 0x75a18,
  0x75a1c; `docs/protocol-1.1.md`, "The box an entity links with");
- `G_SetOrigin`, `G_SetAngle`, `angles2 = 0` (0x52fd2), `think =
  turret_think_init` with `nextthink = level.time + 100`, `controller =
  turret_controller`, `use = turret_use`, `s.apos.trType = 3` (0x53013),
  `takedamage = 1` (0x5301a), `trap_LinkEntity` (0x53025).

VERIFIED, the arc arithmetic (0x52e0e..0x52edc): yaw min `= min(-rightarc,
0)`, yaw max `= max(leftarc, 0)`, pitch min `= min(-toparc, 0)`, pitch max `=
max(bottomarc, 0)`. So the stock gun's record reads yaw -45..45 and pitch
-40..40. INFERRED: positive yaw is to the left and positive pitch is down,
the engine's usual convention, which is why `leftarc` and `bottomarc` are the
maxima.

INFERRED: each arc key falls back to the weapon file when absent, a zero
`health` becomes 100, a negative `dmg` becomes 0, the `damage` key overrides
the weapon file only when present, the
precache check applies only outside the map load (`level+0x1c` zero, a
script spawn), and the model comes from the map's `model` key through the
generic spawn parse, since neither function sets one; the capture's `index`
58 is `xmodel/mg42_bipod` (`docs/protocol-1.1.md`). INFERRED: nothing
installs a `die` or `pain` callback, so a turret has health 100 and
`takedamage` 1 and nothing happens when damage reaches it; `takedamage` acts
only as a usability gate (section 4).

VERIFIED: `turret_think_init` (0x5268c) and the rest-pitch sweep are in
`docs/protocol-1.1.md`, "A turret's `angles2[0]` is where its barrel came to
rest"; they are not repeated here.

## 4. Mount

### 4.1 The use key

VERIFIED: `ClientThink_real` calls `Cmd_Activate_f(ent)` at 0x4064e behind a
`test cl+0x21f0, 0x40`, the use bit's rising edge (`cod11-items.md` section
1). INFERRED: a mount happens inside the cmd that pressed use, after that
cmd's touch pass, and not at the end of the server frame.

VERIFIED, `Cmd_Activate_f` (0x48468):

- a call to `Scr_IsSystemActive(1)` at 0x4847e with a branch to the exit on 0;
- a compare of the player's busy byte against 0 at 0x4848e; past it, a test
  of `ps.eFlags` byte 1 against 0xc0 (0x4849d), a store of 2 into the busy
  byte (0x484a6) or of 0 (0x484b0), and a jump to the exit (0x484bc);
- `G_CheckForCursorHints(player)` (0x484c5) and a load of
  `cl+0x3b8` compared against 0x3ff (0x484d3);
- on the candidate `g_entities[cl+0x3b8]`: classname compares against the
  `func_door` and `func_door_rotating` constants (`G_TryDoor`), against
  `trigger_use` (a `"trigger"` notify), `eType` 3 (the item arm), and `eType`
  11 at 0x485b0 with `G_IsTurretUsable(turret, player)` at 0x485ba and the
  call through `turret->use(turret, player, player)` at 0x485d6.

INFERRED: a use press while mounted does nothing but request the dismount
(busy byte 2, acted on by the next mounted frame, section 8), and runs no
cursor scan, no pickup and no door; a busy byte left on a player who is not
mounted is simply cleared. INFERRED: an `eType` 11 candidate that fails
`G_IsTurretUsable` does nothing, and one that passes is mounted with no script
notify, since the turret arm jumps straight to the `use` call.

The candidate is the item scan's: `G_GetActivateEnt`'s box, 128-unit
distance to the entity's bounds centre, 0.76 cosine and first clear trace
(`cod11-items.md` 2.1). VERIFIED there. INFERRED: a turret enters that scan
through its `r.contents` 0x200000 bit, and its bounds centre sits 28 units
above its origin.

### 4.2 `G_IsTurretUsable` (0x5314c)

VERIFIED, the tests in the function: turret busy byte against 0 (0x53159),
`rec` against null (0x53162), `takedamage` against 0 (0x5316b), the call to
0x52880 (0x53179) and its return against 0, the player's
`ps.grenadeTimeLeft` against 0 (0x53188) and `ps.groundEntityNum` against
0x3ff (0x5318e). INFERRED: usable means nobody on the gun, a live record,
`takedamage` set, inside the arc test, no frag in hand and standing on
something.

VERIFIED, 0x52880: it reads the turret's yaw (`ent+0x144`), `rec+0x10` and
`rec+0x18`; takes `half = (|yawMax| + |yawMin|) * 0.5` (rodata 0x75934 = 0.5);
calls `AngleNormalize180(yaw + yawMin + half)`, `YawVectors` on the result and
`VectorNormalize` on the forward vector; normalises
`(turret.origin - player.origin)` with z zeroed (`r.currentOrigin`, 0x528e8
and 0x528fa); calls `Q_acos` on the dot product; multiplies by 180.0 (0x75938)
and divides by pi (double 0x75940); and compares the result against `half`
(0x5295a). INFERRED: it returns true when the angle is at most `half` (and on
a NaN, which the `and ah, 0x45` compare lets through). INFERRED: the player
must stand behind the gun, inside a cone about the centre of its yaw arc as
seen from the gun, horizontally; for the stock gun that is 45 degrees either
side of the gun's own yaw. There is no distance test here; the distance is
the scan's.

### 4.3 The cursor hint

VERIFIED, `G_CheckForCursorHints` (0x4f59c), the turret arm: an `eType` 11
compare at 0x4f6e2, `G_IsTurretUsable` at 0x4f6ef followed in address order
by a `je` to the loop increment at 0x4f870, `mov edi, 6` at 0x4f714, a test of
`*def->useHintString` (0x4f70e, 0x4f71c) and a load of `def+0x40c` into the
hint-string register (0x4f734). INFERRED: a usable turret hints 6,
`HINT_MG42` (slot 6 of `hintStrings`, `cod11-gsc-object-model.md`), with the
weapon's hint-string slot, and an unusable one is skipped for the next
candidate.

VERIFIED: the function stores 0 into `cl+0x384` and 0x3ff into `cl+0x3b8`
ahead of its busy-byte gate at 0x4f5e4, and the -1 into
`serverCursorHintString` at 0x4f603 sits past that gate (`cod11-items.md`
2.3). INFERRED: a mounted player reads `serverCursorHint` 0 every frame and
keeps whatever `serverCursorHintString` held when it mounted, which is the
turret's own slot 0.

INFERRED, from the loop at 0x4f870: when every candidate is an unusable
turret, the loop ends with `cl+0x3b8` naming the last one and the hint 0, and
the use key's own `G_IsTurretUsable` refuses it again.

### 4.4 `turret_use` (0x52a9c): what a mount writes

VERIFIED, the stores (arguments: turret, player, activator; the player is the
second):

- player and turret busy byte 1 (0x52ab1, 0x52ab8);
- turret `r.ownerNum` = the player's number (0x52ac4);
- `ps.viewlocked = 1` (0x52ad0), `ps.viewlocked_entNum` = the turret's number
  (0x52ae2);
- `rec+0x4 |= 0x800` (0x52aee);
- `rec+0x2c..0x34` = the player's `r.currentOrigin` (0x52af8..0x52b0a);
- player `s.otherEntityNum` = the turret's number (0x52b0f);
- stores of 2, 1 and 0 into `rec+0x24` (0x52b1f, 0x52b2c, 0x52b35) beside
  tests of `pm_flags & 1` (0x52b1b) and `pm_flags & 2` (0x52b28);
- `ps.eFlags` byte 1: `|= 0x40` with `&= 0x7f` (0x52b4d, 0x52b5a), `|= 0x80`
  with `&= 0xbf` (0x52b71, 0x52b7e) and `|= 0xc0` (0x52b90), beside compares
  of `rec+0x20` against 2 (0x52b3f) and 1 (0x52b63);
- turret `+0x28c..0x294` = its `r.currentAngles` (0x52bae..0x52bc6);
- `angles2[i]` for i = 0, 1 = `AngleSubtract(ps.viewangles[i],
  r.currentAngles[i])` (0x52c07), stored (0x52c0c), then compares against
  `rec+0x14 + 4i` and `rec+0xc + 4i` with a store of the bound over it
  (0x52c15..0x52c39);
- `SetClientViewAngle(player, (angles2[0] + pitch, angles2[1] + yaw, 0))`
  (0x52c75).

INFERRED: the saved stance is prone 2, crouch 1, stand 0; a prone gun sets
`eFlags` 0x4000, a duck gun 0x8000 and a stand gun 0xC000; the barrel is put
where the player is looking, clamped to the arcs, and the view is snapped to
the barrel even when nothing was clamped, since no compare sits in front of
the `SetClientViewAngle` call. The mount writes no 15-degree rate
limit (that is the per-frame step, section 6.2) and leaves `angles2[2]` alone.

VERIFIED, absence in the function's 0x1e8 bytes: no store to `ps.weapon`,
`ps.pm_type` or `ps.pm_flags`. INFERRED: the carried weapon stays in
`ps.weapon` for the whole mount and the turret's weapon lives only in its own
`s.weapon`; `pm_type` stays 0 (`PM_NORMAL`).

VERIFIED: `useby` (entity method 11, 0x5d774) notifies `"trigger"` and calls
`ent->use(ent, player, player)` with no `G_IsTurretUsable` call. INFERRED:
script can force a mount; no stock script does (section 1).

## 5. Mounted pmove

VERIFIED, `PmoveSingle`, the `pm_type` 0 arm (0x341b5..0x34276): an `eFlags &
0xC000` test, `groundEntityNum = 0x3ff`, two pml fields zeroed, and calls to
`PM_UpdateAimDownSightFlag`, `PM_UpdatePlayerWalkingFlag`, 0x316f4, 0x32a44
and 0x322c8, and a jump to the function's tail. INFERRED: a mounted player
does not move, has no friction, fires no weapon and reads as airborne to
pmove, while view angles are still updated earlier in `PmoveSingle`.

VERIFIED, 0x316f4 (the stance step), from the Ghidra body at `000416f4`: an
`eFlags & 0xC000` test, compares against 0x4000 and 0x8000, and the `pm_flags`
writes `|= 1` with `&= ~2`, `|= 2` with `&= ~1`, and `&= ~3`. INFERRED: 0x4000
takes the first pair, 0x8000 the second and 0xC000 the third: pmove puts the
player's stance to the gun's, and a stand gun reads `pm_flags` with neither
bit, whatever stance the player mounted from.

VERIFIED: `PM_Weapon` (0x390e0) holds an `eFlags & 0xC000` test beside a
return (`cod11-combat.md` 1.12). INFERRED: no weapon state, switch, reload or
fire event comes out of pmove while mounted.

VERIFIED, `PM_UpdateViewAngles`: one `eFlags & 0xC000` test (0x32f87) and no
load of any turret record or arc. INFERRED: the test skips the prone yaw cap
and nothing more, and the arcs are held only by the game module's
`SetClientViewAngle` in the aim step (section 6.2). VERIFIED: `PM_UpdateLean`
(0x32b10) tests the same mask. INFERRED: it forces the lean input to 0.

VERIFIED: `FireWeapon` and `FireWeaponMelee` test `eFlags & 0xC000` and the
busy byte (0x68d7d, 0x69515). INFERRED: they do nothing for a mounted player.

## 6. The mounted frame

### 6.1 Where it runs

VERIFIED, `ClientEndFrame` (0x40e98): `BG_PlayerAnimation` at 0x41486, a
test of `pm_flags` byte 2 against 0x4 (0x4148e, bit 0x40000), a test of
`ps.eFlags` byte 1 against 0xc0 (0x41494), and a call to
`turret_think_client(g_entities[ps.viewlocked_entNum])` (relocation 0x414b2).
VERIFIED: `G_CheckForCursorHints` is called from the same function at
0x4111a, a lower address. INFERRED: aim, body placement and fire run once per
server frame, after the frame's pmoves and after the animation step, for a
client whose own body is its view (`pm_flags` 0x40000, the live arm's bit,
`cod11-gsc-object-model.md` and `cod11-events-and-fx.md`).

VERIFIED, `turret_think_client` (0x52340): a compare of the owner's busy byte
against 1 (0x5235c) and of its `sessionstate` against 0 (0x5236b), the call to
0x521d4 (0x52379), and the loop-sound block (section 6.4); both compares
branch to the release (0x523e0, section 8). VERIFIED: 0x521d4 calls 0x5201c
(aim, 0x521ee) and 0x515a8 (body placement, 0x521f8), and holds the fire code
at higher addresses. INFERRED: the body is placed off this frame's new
`angles2`, and the shot leaves after both.

### 6.2 Aim (0x5201c)

Re-read with `annotate_func.py` for this document.

VERIFIED, the stores: `ps.viewlocked = 1` (0x5203f), `ps.viewlocked_entNum`
= the turret (0x52051), `ps.gunfx = 0` (0x52065).

VERIFIED, the loop over i = 0 (pitch) and 1 (yaw) (counter at `ebp-0x24`,
0x52155): `want = AngleSubtract(ps.viewangles[i], r.currentAngles[i])`
(0x520aa); a compare against `rec+0x14 + 4i` (0x520bb) and `rec+0xc + 4i`
(0x520d0) with the bound stored over `want` and a flag set (0x520df);
`d = AngleSubtract(want, angles2[i])` (0x52106), `|d|` compared against 15.0
(double 0x758f8) at 0x52112, and `want = angles2[i] + 15.0` or `- 15.0`
(float 0x75900, 0x52137 / 0x52143) with the same flag set (0x52121).
INFERRED: the clamp to the arcs comes first and the rate limit second, so the
barrel moves toward the clamped aim by at most 15 degrees a frame, 300
degrees a second at 20 frames, and a step that starts inside the arcs stays
inside them.

VERIFIED, after the loop: `angles2[0] = want0` (0x52164), `angles2[1] =
want1` (0x5216d), `angles2[2] = 0` (0x52170); a test of `rec+0x4` against
0x800 (0x5217d) with `rec+0x4 &= ~0x800` (0x52188) and `turret s.eFlags ^= 8`
(0x5218b); a test of the flag (0x5218f) and `SetClientViewAngle(player,
(want0 + pitch, want1 + yaw, 0))` (0x521c5). INFERRED: the first mounted
frame after a mount flips the turret's teleport bit, so a client snaps the
barrel instead of lerping it across the mount; the view is rewritten only on
a frame where a clamp or the rate limit changed `want`, so a view inside the
arcs and within 15 degrees of the barrel is left to the client's own
prediction.

### 6.3 Fire (0x521d4)

Re-read with `annotate_func.py` for this document, which found the two
refinements marked below.

VERIFIED, after the two calls: `BG_GetInfoForWeapon(s.weapon)` (0x5220a),
`ps.viewlocked = 1` (0x52217), `turret s.eFlags &= ~0x400` (0x52221), `rec+0x8
+= -50` (immediate 0xffffffce, 0x5222b) with a `jg` to the exit (0x52233),
`rec+0x8 = 0` (0x52239), a test of `cl+0x21e8 & 1` (0x52246) with a `je` to
the exit. INFERRED: the cooldown drops 50 per server frame whatever the
frame's length, and a frame fires when the cooldown has reached 0 and the
last cmd of the frame has attack set: held, not an edge, and a tap that
starts and ends between two frames is never seen.

VERIFIED, the fire block: `rec+0x8 = def->fireTime` (0x52259), `rec+0x28 = 3 *
fireTime` (0x52279..0x52281), a compare of the player's client pointer against
null (0x52287), a store of `&g_entities[1022]` (relocation addend 0xc49d8,
0x5229d), a compare of the player pointer against `&g_entities[1023]` (addend
0xc4cec, 0x522a4) and a store of the player pointer (0x522ac), and a call to
0x51488 (0x522bb) with a test of its return (0x522c3). Further on: `wp+0x3c =
def` (0x522d6), a compare of `def->weaponType` (`+0x70`) against 0 (0x522dc),
`Bullet_Fire(attacker, 0, ent->dmg, &wp, turret)` (0x522f1),
`Weapon_RocketLauncher_Fire(turret, 0, &wp)` (0x52307) and `G_AddEvent(turret,
0xa8, 0)` (0x5231a). `ps.viewlocked = 2` (0x52325) and `turret s.eFlags |=
0x400` (0x5232f).

INFERRED, from where those branches land: the attacker is the player unless
the player is entity 1023, when it is 1022; the bullet leaves only on a
non-zero return from 0x51488; and with the stock 50 ms `fireTime` the
cooldown is back at 0 on the next frame, so a held trigger fires once every
server frame, 20 rounds a second at `sv_fps` 20; a bullet weapon takes
`Bullet_Fire` and anything else the rocket arm. A shot's event is
`EV_FIRE_WEAPON_MG42` (168, `cod11-events-and-fx.md`) on the turret's own
ring, and `ps.viewlocked` reads 2 on the frame of a shot and 1 on every other
mounted frame. Refinement one: a failed tag lookup in 0x51488 skips the
bullet and the event but still reaches `viewlocked = 2` and `eFlags |= 0x400`
(the `je` at 0x522c5 lands on 0x5231f). Refinement two: a player with no
client skips straight to `eFlags |= 0x400` (the `je` at 0x5228e lands on
0x5232f).

VERIFIED, 0x51488 (the fire parameters): `G_DObjGetWorldTagMatrix` on the
turret for `tag_flash` (0x75780, call 0x514a6) and `tag_player` (0x757d9,
0x514dd), each with a `Com_Printf("Couldn't find %s on turret (entity %d,
classname '%s').")` (0x757a0) and a 0 return beside it;
`AngleVectors(ps.viewangles)` into `wp+0`, `+0xc`, `+0x18` (0x5152b);
`wp+0x30..0x38 = wp+0..8` (0x51532..0x5153e); `VectorNormalize(flash.origin
- player.origin)` (0x5156f); `wp+0x24..0x2c = tag_player.origin + forward *
that length` (0x51574..0x51598). INFERRED: the bullet leaves from a point on
the player's view ray through `tag_player`, as far out as `tag_flash` is,
aimed along the player's view rather than along `angles2`; the two agree
except on a frame the rate limit bit, where the view has just been pulled to
the barrel.

VERIFIED, `Bullet_Fire` (0x690dc): the spread argument goes through `tan`,
the range is 8192.0 (0x79cd8), and `Bullet_Fire_Extended` is handed the
turret as inflictor with trace mask 0x2802031. INFERRED: a turret bullet has
no spread at all and the turret does not block its own shot.

VERIFIED, absence in 0x51488..0x53400: no operand at `+0x3ec`, `+0x3f8` or
`+0x3fc`, no read of any ammo or clip array, no `overheat` string anywhere in
the module. INFERRED: `accuracy`, `convergenceTime` and `maxRange` are dead in
MP, the gun never runs dry and never overheats.

### 6.4 The loop sound

VERIFIED, `turret_think_client` after the 0x521d4 call: `s.loopSound = 0`
(0x52387); a compare of `rec+0x28` against 0 (0x52391) with a `jle` to the
exit; `s.loopSound = rec+0x3c` (0x5239f); `rec+0x28 += -50` (0x523a8) with a
`jg` to the exit; a compare of `rec+0x3d` against 0 (0x523b6) with a `je` to
the exit; `s.loopSound = 0` (0x523c0) and `G_PlaySoundAlias(turret, rec+0x3d)`
(0x523d3). VERIFIED: `G_PlaySoundAlias` on an entity with no client writes
`EV_SOUND_ALIAS` (172) with the alias as parm on the entity's own event ring
(`cod11-sound-system.md`).

INFERRED, frame by frame from the last shot (timer 150 after the fire block):
that frame and the next read `loopSound` = `weap_mg42_loop`'s index; the
third reads `loopSound` 0 and carries one `EV_SOUND_ALIAS` for
`weap_mg42_cooldown` on the turret. So the loop is on the wire for two
snapshots counting the last shot's frame, not 150 ms past it as a first
reading put it, and the cooldown event comes 100 ms after the last shot.
INFERRED: the release zeroes the timer and `loopSound` (section 8), so a
release mid-burst cuts the loop with no cooldown event, and the same countdown
in `turret_think` (section 9) has nothing to count in the stock flow.

## 7. Body placement

VERIFIED, 0x515a8, the call list in order of address: a test of the current
anim record's `+0x50 & 4` (0x515e2); `G_DObjGetLocalTagMatrix` on the turret
for `tag_weapon` (0x757e4) with the warning `"WARNING: aborting player
positioning on turret since 'tag_weapon' does not exist"` (0x75800);
`BG_GetInfoForWeapon`; `Scr_GetAnimsIndex`; `vectosignedyaw`; `AnglesToAxis`;
`trap_XAnimClearTree`; repeated `trap_XAnimGetNumChildren`,
`trap_XAnimGetAnimName` (with `Com_Error` on `"Player anim '%s' has no
children"`, 0x75860), `trap_XAnimGetChildAt`, `trap_XAnimGetWeight` and
`trap_XAnimSetGoalWeight`; constants 0.5 (0x758f0) and 1000.0 (0x758f4);
`G_DObjGetLocalTagMatrix` for `tag_aim` (0x75882) with its own warning
(0x758a0); `trap_XAnimCalcAbsDelta`; `VectorAngleMultiply`; `RotationToYaw`;
`YawToAxis`; `MatrixMultiply43`; `trap_Trace` with mask 0x2810011 (0x51f6c,
call 0x51f8d); stores to `ps.origin[2]` (0x51f1b, 0x51fb3);
`BG_PlayerStateToEntityState`; `AxisToAngles`; `trap_LinkEntity`.

INFERRED, all of the following, from the call list and nothing finer:

- the routine runs only when the player's current anim line carries
  `turretanim` (the `+0x50 & 4` flag), and needs `tag_weapon` and `tag_aim`
  on the turret model;
- it sets the blend weights of the mounted anim's children (the yaw columns
  and the three pitch rows, section 10) from `angles2` and the weapon's
  `animHorRotateInc` (`+0x400`), whose stock 15 matches the stand grid's
  column step and the prone file's 20 the prone grid's;
- it takes the blended anim's absolute root delta and builds the player's
  origin and yaw so that the anim's gun lands on the turret's `tag_weapon`;
- it traces down from there to the turret's height and drops `ps.origin[2]`
  onto what it hits, then copies the playerstate to the entity state and sets
  `r.currentOrigin = ps.origin` and `r.currentAngles` from the axis.

VERIFIED: the turret model's `tag_aim` is at (-2.98, 0.06, 12.87) and its
children `tag_player` at (-45.09, 0.06, 20.92), `tag_weapon` at (-33.59,
-0.03, 12.12) and `tag_butt` at (-49.63, 0, 10.95), bind positions in model
space out of `xmodel/MG42_bipod`'s parts; `tag_flash` (8.41, 0.15, 15.16) hangs
off the separate `tag_aim_animated` chain under `mg01`. VERIFIED, the gun's own
anims (`standMG42gun_{aim,fire,recover}_foward`, 9 tracks) key only
`tag_pivot`, `mg01` and `tag_flash` with more than one frame.

VERIFIED, `cgame_mp_x86.dll` (1.1): the same two warnings at 0x30062fd0
(`tag_aim`) and 0x30063048 (`tag_weapon`), and `"Turret has no bone:
tag_player"` at 0x3006227c. INFERRED: a retail client places a mounted body
itself with the same routine, so the server's `ps.origin` is what a client
predicts against, not what it draws from.

**Open question.** Whether sampling the `standMG42_aim` leaves at the
turret's `angles2` with vcod's xanim and skeleton code reproduces retail's
`ps.origin` to the unit is not known until a capture exists (section 12).
Nor is what `playerPositionDist` (`+0x404`) feeds. VERIFIED, absence: no
operand at `+0x404` sits in 0x51488..0x53400. INFERRED: the 42-unit
horizontal gap between `tag_aim` and `tag_player` against its 46 is a
coincidence as far as the bytes go.

## 8. Release and its triggers

VERIFIED, the release body, the same stores in `G_ClientStopUsingTurret`
(0x53054), `G_FreeTurret` (0x5297c) and `turret_think_client`'s arm at
0x523e0:

- `rec+0x28 = 0`, turret `s.loopSound = 0` (0x523f6, 0x523fd);
- a compare of `rec+0x24` against -1 (0x5240a), compares against 2 and 1,
  one `G_AddEvent(player, event, 0)` call (0x5243c) fed 0x8e, 0x8d or 0x8c
  (0x52419, 0x5242a, 0x52436), and `rec+0x24 = -1` (0x52444);
- `TeleportPlayer(player, rec+0x2c, player r.currentAngles)` (0x5245a);
- `ps.eFlags` byte 1 `&= 0x3f` (0x52465), `ps.viewlocked = 0` (0x52472),
  `ps.viewlocked_entNum = 0x3ff` (0x52482), player busy byte 0 (0x52492),
  `ps.gunfx = 0` (0x52499), player `s.otherEntityNum = 0` (0x524a3), turret
  busy byte 0 (0x524aa), turret `r.ownerNum = 0x3ff` (0x524b1), `rec+0x4 &=
  ~0x800` (0x524bb).

INFERRED: a saved 2 sends 0x8e, a 1 0x8d, anything else 0x8c, and a -1
sends none. INFERRED: the stance event is `EV_STANCE_FORCE_PRONE` (142),
`EV_STANCE_FORCE_CROUCH` (141) or `EV_STANCE_FORCE_STAND` (140)
(`cod11-events-and-fx.md`), and puts the player back in the stance it
mounted from.

VERIFIED, `TeleportPlayer` (0x51380): a compare of `sessionstate` against 0
(0x51395) with `G_TempEntity` for 200 at the old origin and 199 at the new
one (0x513aa, 0x513c6), `trap_UnlinkEntity`, `ps.origin` = the destination
with 1.0 added to z (0x51414..0x51419), `ps.eFlags ^= 8` (0x51428),
`SetClientViewAngle` (0x51434), `BG_PlayerStateToEntityState`,
`r.currentOrigin = ps.origin`, and a conditional `trap_LinkEntity`. INFERRED:
the player lands one unit above where it stood to mount, facing the angles
its entity had, with `EV_PLAYER_TELEPORT_OUT`/`_IN` temp entities only when
it is still playing, and the teleport bit flipped.

The triggers, INFERRED from the call sites:

- the use key: `Cmd_Activate_f` sets the busy byte to 2 (section 4.1), and
  the next `turret_think_client` finds it not 1 (0x5235c) and releases;
- death: a killed client's `sessionstate` leaves 0 in the stock
  `CodeCallback_PlayerKilled`, a dead client still takes `ClientEndFrame`'s
  live arm and keeps `pm_flags` 0x40000, and `turret_think_client` finds
  `sessionstate` non-zero (0x5236b) and releases;
- a respawn: `ClientSpawn` (0x426c4..0x426e2) tests `pm_flags & 0x40000` and
  `eFlags & 0xC000` and calls `G_ClientStopUsingTurret` on
  `g_entities[viewlocked_entNum]`, which is the path a switch to spectator
  takes if it goes through `ClientSpawn`;
- deleting the turret: `G_FreeEntity` calls `G_FreeTurret` (0x66b6f), which
  releases any gunner and clears the record.

VERIFIED: `StopFollowing` (0x46b8c) clears `eFlags` 0xC000, `viewlocked`,
`viewlocked_entNum` and `gunfx` on a spectator's playerstate. INFERRED: a
spectator following a gunner inherits those fields through the follow copy
and loses them here.

## 9. The unowned barrel

VERIFIED, `turret_think` (0x5328c): `nextthink += 50`, a test of `ent+0x2e4`
beside a `G_GeneralLink` call, a test of `g_entities[r.ownerNum].client`
against null, the loop-sound countdown of section 6.4, a clear of `s.eFlags`
0x400, and a call to 0x524cc with a vector of (`rec+0x1c`, 0) and a third
argument 0 (0x5333b). INFERRED: the countdown, the clear and the slew run only
when the owner has no client. VERIFIED: 0x524cc holds 200.0 (0x75904), loads
of the weapon's `vertTurnSpeed`/`horTurnSpeed` (`+0x3f0`/`+0x3f4`), a multiply
by 0.05 (0x7590c) and a test of its third argument, and `turret_think` is its
only caller. INFERRED: the 200 applies when that argument is 0, so an unmanned
barrel walks back to (rest pitch, 0) at 10 degrees a frame, 200 a second, and
the turn speeds in the weapon file are dead in MP. INFERRED: with the owner at
0x3ff the test reads the world entity's client, which is null, so every
unmanned frame takes this branch.

VERIFIED: `turret_controller` (0x53348) hands (`angles2[0]`, `angles2[1]`, 0)
to `G_DObjSetControlTagAngles` for `tag_aim` and `tag_aim_animated`, and
(`angles2[2]`, 0, 0) for `tag_flash` (`docs/protocol-1.1.md`). VERIFIED: 0x524cc
stores the step it could not take in `angles2[2]` (same section), and the
aim step writes `angles2[2] = 0` (section 6.2). INFERRED: the flash tilts
only while an unmanned barrel is still slewing.

## 10. Anims

VERIFIED, `mp/playeranim.script` (pak4): `idle`, `idlecr` and `idleprone`
under `STATE COMBAT` each open with `mounted mg42, firing` (`both
standMG42_fire turretanim`, `proneMG42_fire` for `idleprone`) and `mounted
mg42` (`both standMG42_aim turretanim`, `proneMG42_aim`); the `fireweapon`
event block's `mounted mg42` clause is empty with the comment `Ignore the
fireweapon event while on a turret`. VERIFIED: no walk, run or turn movetype
has a `mounted` clause, and the legend lists `mounted: mg42` and not
`firing`. INFERRED: a crouched mount plays the stand blend.

VERIFIED, `animtrees/multiplayer.atr` (pak5): `standMG42_fire` (loopsync) and
`standMG42_aim` (nonloopsync) are blend nodes of three pitch rows (`15down`,
`level`, `15up`) of seven yaw columns (`45left`..`45right`, 15-degree steps);
`proneMG42_*` has five (`40left`..`40right`, 20-degree steps). VERIFIED: all
72 `pb_*MG42gunner_*` leaves are in pak0; the `aim` leaves have one frame and
the `fire` leaves five at 30 fps, looping. VERIFIED: the game module and the
cgame both carry `"BG_ParseCommands: Turret animations can only be played on
the 'both' body part"` (`.so` 0x6e7e0, `turretanim` at 0x6e7ca; cgame
0x30068d70).

VERIFIED, `BG_AnimUpdatePlayerStateConditions`: an `eFlags & 0xC000` test and
stores of 1 and 0 into one condition slot (0x2a454), and a later slot written
from the pmove cmd's attack bit. INFERRED: the slot reads 1 exactly when a
mounted bit is set, and the two slots are `mounted` and `firing`, by their
order against the script's condition list.

VERIFIED, 0x322c8 (Ghidra body at `000422c8`): an `eFlags & 0xC000` test,
tests of `pm_flags` 0x1 and 0x2, and the values 3, 2 and 1 written to the one
variable the function then takes its movetype from. INFERRED: a mounted player
gets 3 when prone, 2 when crouched and 1 otherwise, and those are the
`idleprone`, `idlecr` and `idle` movetypes, so with 0x316f4 putting `pm_flags`
to the gun's stance a stand gun always plays `idle`.

VERIFIED, `BG_PlayerStateToEntityState` (0x2cd94): an `eFlags & 0xC000` test
and a store of `ps.viewlocked_entNum` into `es.otherEntityNum`. INFERRED: the
store runs for a mounted player only, and that is how a second client learns
which turret a player's body belongs on.

## 11. Kill credit

VERIFIED, `player_die` (0x49a48), step 3 in `cod11-combat.md` 5.1: the weapon
is replaced by `g_entities[attacker->s.otherEntityNum]->s.weapon` behind tests
of the attacker's client, its `eFlags & 0xC000`, and that entity's `eType`
against 11. INFERRED: a kill from the gun is credited to
`mg42_bipod_stand_mp`, not the gunner's carried weapon, which is what puts the
MG42 kill icon on the obituary.

Not determined from the bytes: the means of death and the hit location a
turret bullet carries through `Bullet_Fire_Extended`. The `D;`/`K;` lines a
retail `games_mp.log` writes for a turret kill are the evidence for both
(section 12).

## 12. What the capture measured

## 13. As implemented
