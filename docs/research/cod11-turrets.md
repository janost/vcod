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
every branch condition, and anything that says what a field means. Section
12 reads the retail capture of a mount on mp_carentan; the earlier sections
point there where the capture bears on a claim.

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
| `cl+0x374` / `+0x378` / `+0x380` | `ps.viewlocked` (8 bits) / `ps.viewlocked_entNum` (16 bits) / `ps.gunfx` (8 bits) | VERIFIED, player netfield offsets 884, 888 and 896 in `crates/common/src/net/cod1/fields_v1.rs` |
| `cl+0x20d0` | `sessionstate` | VERIFIED, `cod11-map-cycle.md`; the name is INFERRED |
| `cl+0x21e8` | the last cmd's buttons byte | VERIFIED, `cod11-gsc-object-model.md` 23.5 |

The turret record: 32 of them, 0x40 bytes each, array at 0xaa180. VERIFIED:
`G_SpawnTurret` holds the base `0xaa180`, a stride of 0x40, a compare of a
record's first dword against zero, a bound of 0x20, `Com_Error(1,
"G_SpawnTurret: max number of turrets (%d) exceeded")` (string 0x75960, call
0x52cc3) and `__bzero(rec, 0x40)` (0x52cd1). INFERRED: it scans for the first
free record, clears it, and a 33rd turret is a fatal error. VERIFIED:
`G_InitTurrets` (0x53034) zeroes the 32 first dwords, and `G_InitGame` calls
it (0x4fe18).

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
`index` 58 and `weapon` 16 on both, in samples 2, 3 and 4 of that file
(fixture lines 242-261, 373-392, 504-523). Samples 0 and 1 carry neither: the
probe stood where the PVS culls them. So the gun a
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

VERIFIED: `BG_SetupWeaponInfo` (0x36674) holds a load of `def->useHintString`
(0x369e0) and a test of its first byte (0x369e6), a call to
`G_GetHintStringIndex(def+0x40c, def->useHintString)` (0x369f5), a test of
its return (0x369fd) and `Com_Error(1, "Too many different hintstring values
on weapons. Max allowed is %i different strings", 0x20)` (string 0x72360,
call 0x36a0d). INFERRED: the call runs for every weapon whose
`useHintString` is non-empty, and a zero return is the fatal error.
INFERRED: `def+0x40c` holds the slot in the
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
  `Com_Error(1, "bad weaponinfo '%s' specified for turret")` (0x759a0);
- `BG_GetInfoForWeapon` (0x52d21), a compare of `level+0x1c` against 0
  (0x52d2b), `IsItemRegistered` (0x52d3e) and `Scr_Error` on `"turret '%s'
  not precached"` (0x759ca, 0x52d5f), and `RegisterItem(weapon, 1)`
  (0x52d73);
- `rec+0x8 = 0`, `rec+0x20 = def->stance`, `rec+0x24 = -1`, `rec+0x28 = 0`
  (0x52d78..0x52d8f), stores of `G_SoundAliasIndex` on `def+0xa0` and
  `def+0xa4` into the two alias bytes, and stores of 0 into the same bytes
  (0x52db1..0x52de4);
- `G_SpawnFloat` on `rightarc` (0x759e5), `leftarc` (0x759ee), `toparc`
  (0x759f6) and `bottomarc` (0x759fd), each with the empty default
  (0x759e4), and loads of the weapon file's `rightArc`, `leftArc`, `topArc`
  and `bottomArc`;
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

INFERRED: the order of the list is the order the function runs in; each
alias byte is 0 when its weapon-file string is empty; the precache
`Scr_Error` is reached only when `level+0x1c` is zero (outside the map load,
a script spawn) and the item is not registered; each arc key falls back to
the weapon file when absent; a zero `health` becomes 100; the `damage` key
overrides the weapon file only when present and a negative `dmg` becomes 0;
and the model comes from the map's `model` key through the
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

VERIFIED: `ClientThink_real` holds a `test cl+0x21f0, 0x40` at 0x4063e and a
call to `Cmd_Activate_f(ent)` at 0x4064e, and `cl+0x21f0` is written from
`cmd.buttons & ~oldButtons` (`cod11-items.md` section 1). INFERRED: the call
is taken only on the use bit's rising edge. INFERRED: a mount happens inside
the cmd that pressed use, after that cmd's touch pass, and not at the end of
the server frame.

VERIFIED, `Cmd_Activate_f` (0x48468):

- a call to `Scr_IsSystemActive(1)` at 0x4847e and a test of its return;
- a compare of the player's busy byte against 0 at 0x4848e, a test of
  `ps.eFlags` byte 1 against 0xc0 (0x4849d), stores of 2 (0x484a6) and of 0
  (0x484b0) into the busy byte, and a `jmp` at 0x484bc;
- `G_CheckForCursorHints(player)` (0x484c5) and a compare of `cl+0x3b8`
  against 0x3ff (0x484d3);
- a load of `g_entities[cl+0x3b8]` (0x484e2..0x484ef); classname compares
  against the `func_door` and `func_door_rotating` constants and a call to
  `G_TryDoor`, a compare against `trigger_use` and a `"trigger"` notify, an
  `eType` compare against 3 (the item arm), an `eType` compare against 11 at
  0x485b0, `G_IsTurretUsable(turret, player)` at 0x485ba, and the call through
  `turret->use(turret, player, player)` at 0x485d6.

INFERRED: `Scr_IsSystemActive` returning 0 ends the function; a non-zero busy
byte takes the 2-or-0 store and the `jmp` at 0x484bc leaves the function;
otherwise the cursor scan runs and a 0x3ff candidate ends it; the candidate
is then tested against each arm in the order listed. INFERRED: a use press
while mounted does nothing but request the dismount
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
compare at 0x4f6e2, `G_IsTurretUsable` at 0x4f6ef, a `je` at 0x4f6f9 whose
target operand is 0x4f870 (the loop increment), `mov edi, 6` at 0x4f714, a
test of `*def->useHintString` (0x4f70e, 0x4f71c) and a load of `def+0x40c`
into the hint-string register (0x4f734). INFERRED: a false return takes the
`je`, so an unusable turret is skipped for the next candidate; a usable one
hints 6, `HINT_MG42` (slot 6 of `hintStrings`, `cod11-gsc-object-model.md`),
with the weapon's hint-string slot.

VERIFIED: the function stores 0 into `cl+0x384` (0x4f5ba), 0 into `cl+0x388`
(0x4f5c4) and 0x3ff into `cl+0x3b8` (0x4f5ce), compares the player's `health`
against 0 (0x4f5d7) and its busy byte against 0 (0x4f5e4), and stores -1 into
`serverCursorHintString` at 0x4f603 (`cod11-items.md` 2.3). INFERRED: the
three stores run on every call, and a player with health at or below 0 or a
non-zero busy byte leaves before the -1 store. INFERRED: a mounted player
reads `serverCursorHint` 0 every frame and keeps whatever
`serverCursorHintString` held when it mounted, which is the turret's own slot
0. VERIFIED on the wire: 12.10.

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
- tests of `pm_flags & 1` (0x52b1b) and `pm_flags & 2` (0x52b28), and
  stores of 2, 1 and 0 into `rec+0x24` (0x52b1f, 0x52b2c, 0x52b35);
- `ps.eFlags` byte 1: `|= 0x40` with `&= 0x7f` (0x52b4d, 0x52b5a), `|= 0x80`
  with `&= 0xbf` (0x52b71, 0x52b7e) and `|= 0xc0` (0x52b90), and compares
  of `rec+0x20` against 2 (0x52b3f) and 1 (0x52b63);
- turret `+0x28c..0x294` = its `r.currentAngles` (0x52bae..0x52bc6);
- `angles2[i]` for i = 0, 1 = `AngleSubtract(ps.viewangles[i],
  r.currentAngles[i])` (0x52c07), stored (0x52c0c); compares against
  `rec+0x14 + 4i` and `rec+0xc + 4i` and a store of the bound into the same
  slot (0x52c15..0x52c39);
- `SetClientViewAngle(player, (angles2[0] + pitch, angles2[1] + yaw, 0))`
  (0x52c75).

INFERRED: the stores run in the order listed; each clamp store happens only
past its bound; the saved stance is prone 2, crouch 1, stand 0; a prone gun
sets `eFlags` 0x4000, a duck gun 0x8000 and a stand gun 0xC000; the barrel is
put where the player is looking, clamped to the arcs, and the view is snapped
to the barrel even when nothing was clamped, since no compare sits in front of
the `SetClientViewAngle` call. The mount writes no 15-degree rate limit (that
is the per-frame step, section 6.2) and leaves `angles2[2]` alone.

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
and 0x322c8, and a `jmp` whose target is the function's tail. INFERRED: the
arm is taken on a mounted bit and skips the rest of the move. INFERRED: a
mounted player does not move, has no friction, fires no weapon and reads as
airborne to pmove, while view angles are still updated earlier in
`PmoveSingle`.

VERIFIED, 0x316f4 (the stance step), from the Ghidra body at `000416f4`: an
`eFlags & 0xC000` test, compares against 0x4000 and 0x8000, and the `pm_flags`
writes `|= 1` with `&= ~2`, `|= 2` with `&= ~1`, and `&= ~3`. INFERRED: 0x4000
takes the first pair, 0x8000 the second and 0xC000 the third: pmove puts the
player's stance to the gun's, and a stand gun reads `pm_flags` with neither
bit, whatever stance the player mounted from.

VERIFIED: `PM_Weapon` (0x390e0) holds an `eFlags & 0xC000` test
(`cod11-combat.md` 1.12). INFERRED: the function returns on it, so no
weapon state, switch, reload or fire event comes out of pmove while mounted.

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
0x521d4 (0x52379), and the loop-sound block (section 6.4); the two `jne`
instructions at 0x52363 and 0x52372 carry the target 0x523e0, the release
(section 8). VERIFIED: 0x521d4 calls 0x5201c (aim, 0x521ee) and 0x515a8 (body
placement, 0x521f8), and holds the fire code at higher addresses. INFERRED: a
busy byte other than 1 or a `sessionstate` other than 0 takes the release
instead of the mounted frame; the aim runs before the placement and both
before the shot. INFERRED: the body is placed off this frame's new `angles2`,
and the shot leaves after both. VERIFIED, 12.2: each snapshot's origin
matches that snapshot's `angles2`.

### 6.2 Aim (0x5201c)

Re-read with `annotate_func.py` for this document.

VERIFIED, the stores: `ps.viewlocked = 1` (0x5203f), `ps.viewlocked_entNum`
= the turret (0x52051), `ps.gunfx = 0` (0x52065).

VERIFIED, a body indexed by i = 0 (pitch) and 1 (yaw), with a counter at
`ebp-0x24` set to 1 (0x52079) and decremented at 0x52155: `want =
AngleSubtract(ps.viewangles[i], r.currentAngles[i])` (0x520aa); a compare
against `rec+0x14 + 4i` (0x520bb) and `rec+0xc + 4i` (0x520d0), and stores of
the bound into `want` and of 1 into a flag (0x520df, 0x520e6); `d =
AngleSubtract(want, angles2[i])` (0x52106), `|d|` compared against 15.0
(double 0x758f8) at 0x52112, and `want = angles2[i] + 15.0` or `- 15.0` (float
0x75900, 0x52137 / 0x52143) and a store of 1 into the same flag (0x52121).
INFERRED: the body runs twice; each bound and each 15-degree step is stored
only when exceeded. INFERRED: the clamp to the arcs comes first and the rate
limit second, so the barrel moves toward the clamped aim by at most 15 degrees
a frame, 300 degrees a second at 20 frames, and a step that starts inside the
arcs stays inside them.

VERIFIED, at 0x5215e..0x521c5: `angles2[0] = want0` (0x52164), `angles2[1] =
want1` (0x5216d), `angles2[2] = 0` (0x52170); a test of `rec+0x4` against
0x800 (0x5217d), `rec+0x4 &= ~0x800` (0x52188) and `turret s.eFlags ^= 8`
(0x5218b); a test of the flag (0x5218f) and `SetClientViewAngle(player, (want0
+ pitch, want1 + yaw, 0))` (0x521c5). INFERRED: the clear and the xor run only
when 0x800 is set, and the view call only when the flag is set. INFERRED: the
first mounted frame after a mount flips the turret's teleport bit, so a client
snaps the barrel instead of lerping it across the mount; the view is rewritten
only on a frame where a clamp or the rate limit changed `want`, so a view
inside the arcs and within 15 degrees of the barrel is left to the client's
own prediction. VERIFIED on the wire: 12.3.

### 6.3 Fire (0x521d4)

Re-read with `annotate_func.py` for this document, which found the two
refinements marked below.

VERIFIED, at 0x521fd..0x5224d: `BG_GetInfoForWeapon(s.weapon)` (0x5220a),
`ps.viewlocked = 1` (0x52217), `turret s.eFlags &= ~0x400` (0x52221),
`rec+0x8 += -50` (immediate 0xffffffce, 0x5222b), a `jg` at 0x52233 and a
`je` at 0x5224d whose target operands are both 0x52333, `rec+0x8 = 0`
(0x52239), and a test of `cl+0x21e8 & 1` (0x52246). INFERRED: 0x52333 is the
epilogue, so a cooldown still above 0 or a clear attack bit ends the frame
with no shot. INFERRED: the cooldown drops 50 per server frame whatever the
frame's length, and a frame fires when the cooldown has reached 0 and the
last cmd of the frame has attack set: held, not an edge, and a tap that
starts and ends between two frames is never seen.

VERIFIED, the fire block: `rec+0x8 = def->fireTime` (0x52259), `rec+0x28 = 3 *
fireTime` (0x52279..0x52281), a compare of the player's client pointer against
null (0x52287), a store of `&g_entities[1022]` (relocation addend 0xc49d8,
0x5229d), a compare of the player pointer against `&g_entities[1023]` (addend
0xc4cec, 0x522a4) and a store of the player pointer (0x522ac), and a call to
0x51488 (0x522bb) with a test of its return (0x522c3); `wp+0x3c =
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
0x5232f). VERIFIED, 12.4: a held trigger fires on every server frame with one
event each, `viewlocked` 2 exactly on those frames, and 20 rounds from a still
barrel land on one point.

VERIFIED, 0x51488 (the fire parameters): `G_DObjGetWorldTagMatrix` on the
turret for `tag_flash` (0x75780, call 0x514a6) and `tag_player` (0x757d9,
0x514dd), a test of each return, one `Com_Printf("Couldn't find %s on turret
(entity %d, classname '%s').")` (0x757a0, call 0x51507) and a store of 0 into
the return register (0x5150c); `AngleVectors(ps.viewangles)` into `wp+0`,
`+0xc`, `+0x18` (0x5152b); `wp+0x30..0x38 = wp+0..8` (0x51532..0x5153e);
`VectorNormalize(flash.origin - player.origin)` (0x5156f); `wp+0x24..0x2c =
tag_player.origin + forward * that length` (0x51574..0x51598). INFERRED: a
missing tag prints the line and returns 0 before any of the vector work.
INFERRED: the bullet leaves from a point on the player's view ray through
`tag_player`, as far out as `tag_flash` is, aimed along the player's view
rather than along `angles2`; the two agree except on a frame the rate limit
bit, where the view has just been pulled to the barrel.

VERIFIED, `Bullet_Fire` (0x690dc): the spread argument goes through `tan`,
the range is 8192.0 (0x79cd8), and `Bullet_Fire_Extended` is handed the
turret as inflictor with trace mask 0x2802031. INFERRED: a turret bullet has
no spread at all and the turret does not block its own shot.

VERIFIED, `Bullet_Fire_Extended` 0x68b05: a world impact's
`otherEntityNum` is that inflictor, so it names the gun, and the cgame
starts the tracer at the gun's own `tag_flash` (`cod11-events-and-fx.md`
2 and 6). VERIFIED (capture): every bullet hit in `snapshots-15-turret.bin`
names the gun (1.5b). vcod-server writes the gun's number on a turret
round's impacts (`TurretShot::gun`; `turret_ab`'s
`a_turret_rounds_impacts_name_the_gun`); the flesh copy keeps the gunner,
`finishPlayerDamage`'s attacker.

VERIFIED, the round's pass entity: `Bullet_Fire` hands its fifth argument
(the gun) to `Bullet_Fire_Extended` as the first (1.1d 0x691b3, 1.5b
0x6dfeb, UO 0x7a90f), and `Bullet_Fire_Extended` passes that entity's
number to `trap_LocationalTrace` (1.1d 0x68903..0x68912, 1.5b 0x6e093,
UO 0x7a9c7). VERIFIED, the engine's pass rule (`cod_lnxded` 1.1d
`SV_ClipMoveToEntity` 0x8091099..0x80910bf): an entity is skipped when it
is the pass entity, when its `r.ownerNum` is the pass entity, or when its
`r.ownerNum` is the pass entity's own owner, read at 0x809184c and turned
into -1 for 0x3ff at 0x809185f. VERIFIED, `turret_use` stores the
gunner's number into the gun's `r.ownerNum` (1.1d 0x52ac4 at `+0x14c`);
INFERRED, 1.5b 0x53008 (`+0x150`) and UO 0x5c955 (`+0x154`) are the same
store, the field moved by the larger entity header. INFERRED, from those
three: a manned gun's round skips the gun and whatever the gunner owns,
as the gunner's own pass did, and does not skip the gunner. INFERRED: the
gun never clips a round either way, since its `r.contents` 0x200004
(section 3) shares no bit with the mask 0x2802031, so the gunner's own
body is the only thing the choice decides. vcod-server passes the gun
(`TurretShot::pass`). Measured in process only: on mp_carentan's 855 gun
swapped for the stand, duck and prone weapon files, a round at each arc
corner and at the centre never meets the gunner (`turret_ab`'s
`a_gunner_never_meets_his_own_round`); the muzzle sits
`|tag_flash - tag_player|` along the view, ahead of his bones. VERIFIED
(capture): the fire and target phases of the 1.1d, 1.5b and UO turret
fixtures carry no pain event on the gunner. Not measured on retail: a
placement that puts the gunner's body in front of that point.

VERIFIED, absence in 0x51488..0x53400: no operand at `+0x3ec`, `+0x3f8` or
`+0x3fc`, no read of any ammo or clip array, no `overheat` string anywhere in
the module. INFERRED: `accuracy`, `convergenceTime` and `maxRange` are dead in
MP, the gun never runs dry and never overheats.

### 6.4 The loop sound

VERIFIED, `turret_think_client` at 0x5237e..0x523d8: `s.loopSound = 0`
(0x52387); a compare of `rec+0x28` against 0 (0x52391); `s.loopSound =
rec+0x3c` (0x5239f); `rec+0x28 += -50` (0x523a8); a compare of `rec+0x3d`
against 0 (0x523b6); `s.loopSound = 0` (0x523c0) and
`G_PlaySoundAlias(turret, rec+0x3d)` (0x523d3); and a `jle` (0x52395), a
`jg` (0x523b0) and a `je` (0x523ba) whose target operands are all 0x524bf.
VERIFIED, `cod11-sound-system.md`: `G_PlaySoundAlias` holds a
path that writes `EV_SOUND_ALIAS` (172) into the playerstate ring and one
that writes it into the entity's own ring, with the alias as parm.

INFERRED: 0x524bf is the epilogue; the loop alias is set only while the timer
is positive, the timer drops only then, and the cooldown alias plays only on
the frame the timer reaches 0 or below with a stop alias present; a turret,
having no client, takes the entity-ring path.

INFERRED, frame by frame from the last shot (timer 150 after the fire block):
that frame and the next read `loopSound` = `weap_mg42_loop`'s index; the third
reads `loopSound` 0 and carries one `EV_SOUND_ALIAS` for `weap_mg42_cooldown`
on the turret. So the loop is on the wire for two snapshots counting the last
shot's frame, not 150 ms past it as a first reading put it, and the cooldown
event comes 100 ms after the last shot. VERIFIED, 12.4: the capture shows
those three frames exactly. INFERRED: the release zeroes the timer
and `loopSound` (section 8), so a release mid-burst cuts the loop with no
cooldown event, and the same countdown in `turret_think` (section 9) has
nothing to count in the stock flow.

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

### 7.1 The reads and stores

Re-read with `annotate_func.py` over 0x515a8..0x5201b for this section. `ent`
is the player, `tur` the turret, `ci` the player's `bgs` client info
(`bgs + clientNum * 0x448`). Each item carries its own labels.

- VERIFIED: a compare of the word at `ci+0x9ba78` against 0 (0x515c7), a
  compare of the pointer at `ci+0x9ba7c` against null (0x515d4) and a test of
  its `+0x50` byte against 4 (0x515e2). VERIFIED: `BG_ParseCommands` keeps its
  third argument plus 0x50 at `ebp-0xc` (0x2866a) and ORs 4 into the byte at
  that base plus the anim index times 0x5c (0x2896d..0x28973), next to the
  push of the `turretanim` string (0x28946). INFERRED: `+0x50 & 4` is a flag on
  the 92-byte `animation_t` record, set by any script line that names the anim
  with `turretanim`, and the placement runs only while the current legs anim
  carries it.
- VERIFIED: the word at `ci+0x9ba78` with bit 0x200 cleared (`and ah,0xfd`,
  0x51671) and `Scr_GetAnimsIndex`'s result shifted into the high half
  (0x51652), and the dword at `ci+0x9bb2c` loaded as the first argument of
  every `trap_XAnim*` call (0x51638). INFERRED: the handle is the legs anim
  in the player's own anim tree.
- VERIFIED: `vectosignedyaw` called on the pointer `G_DObjGetLocalTagMatrix`
  returned for `tag_weapon` (0x51682), and loads of that matrix's floats at
  `+0x30`, `+0x34` and `+0x38` (0x51e9e, 0x51eaa, 0x51710). INFERRED: the
  first three floats are the tag's forward axis and `+0x30..+0x38` its
  translation in the gun's model space.
- VERIFIED: `AnglesToAxis(tur.r.currentAngles)` into `ebp-0x3c` (0x5169d) and
  `tur.r.currentOrigin` stored right after it at `ebp-0x18..-0x10`
  (0x516ae..0x516c0); `ent.r.currentOrigin - tur.r.currentOrigin` multiplied
  by the floats at `ebp-0x24..-0x1c` and summed (0x516c5..0x51700), stored at
  `ebp-0x154` (0x5170a), and that sum minus the tag's `+0x38` stored at
  `ebp-0x140` (0x51719). INFERRED: the first is the player's height above the
  gun along the gun's up axis, the second that height below `tag_weapon`.
- VERIFIED, the immediates and calls between 0x5173e and 0x51a16:
  `trap_XAnimGetNumChildren` on the anim (0x5173e), `trap_XAnimGetChildAt`
  (0x517e9), `trap_XAnimSetGoalWeight` with 1.0 (0x51812),
  `trap_XAnimGetNumChildren` on the child (0x5181e), a multiply of that count
  by 0.5 (0x51854, 0x758f0), a divide of the tag's yaw by the weapon def's
  `+0x400` (0x5186c), compares against 0 and against the count minus one
  (0x5187e..0x518ae), a `fistp` under control word bits 0xc00 (0x518df), a
  subtract of the integer back out (0x518fd), `trap_XAnimSetGoalWeight` with
  `1 - f` (0x5195b), a compare of `f` against 0 (0x5196b),
  `trap_XAnimGetChildAt` with the index plus one (0x51986) and
  `trap_XAnimSetGoalWeight` with `f` (0x519b6), `trap_XAnimCalcAbsDelta` on
  the child with its translation out at `ebp-0xc` (0x519d4), a compare of the
  float at `ebp-0x4` against `ebp-0x140` (0x519df), stores of that float, `f`
  and the index (0x519ec..0x51a04), an increment of `ebp-0x12c` and a compare
  against the child count (0x51a0a, 0x51a10). INFERRED: this is a loop over
  the anim's children (the rows); per row the column position is `x = 0.5 * n
  - yaw / animHorRotateInc` clamped to `[0, n - 1]`, rounded toward zero, and
  split between the two columns either side; the `je` at 0x519ea leaves the
  loop at the first row whose delta z reaches the target, and every other row
  leaves its z, `f` and index behind for the next.
- VERIFIED, the calls between 0x51a20 and 0x51e57: `trap_XAnimClearTree` on
  the anim (0x51a33); `trap_XAnimSetGoalWeight` with `1 - f` and with `f`
  (0x51aa8, 0x51b35); compares of `ebp-0x12c` against 0 and against the child
  count (0x51b3d, 0x51b4c); `G_DObjGetLocalTagMatrix` for `tag_aim` (0x51b64)
  and a `jmp` to a `trap_XAnimSetGoalWeight` with 1.0 (0x51be7, 0x51e57);
  `(ebp-0x140 - z_prev) / (z - z_prev)` (0x51bf0..0x51c14) passed to
  `trap_XAnimSetGoalWeight` (0x51c7a); `trap_XAnimGetChildAt` with the index
  minus one (0x51c9b) and its goal weight one minus that (0x51d1a); the
  stored index and `f` passed on to two more `trap_XAnimSetGoalWeight` calls
  (0x51da8, 0x51e57). VERIFIED: every one of these weights is passed with a
  time built from `trap_XAnimGetWeight`, 1000.0 (0x758f4) and the int at
  `level+0x1f0`. INFERRED: a loop that broke at the first row or never broke
  gives that end row alone weight 1 (the `tag_aim` lookup only guards it);
  otherwise the breaking row gets `t`, the row before it `1 - t`, and each
  row its own column pair.
- VERIFIED: `trap_XAnimCalcAbsDelta` on the whole anim (0x51e7b);
  `VectorAngleMultiply` of its translation by the tag's yaw (0x51e90); the
  translation's x and y plus the tag's `+0x30` and `+0x34` stored at
  `ebp-0x48`, `ebp-0x44`, and `ebp-0x154` stored at `ebp-0x40`
  (0x51e9b..0x51ebf); `RotationToYaw` of the delta's rotation plus the tag's
  yaw passed to `YawToAxis` into `ebp-0x6c` (0x51ec3..0x51edb);
  `MatrixMultiply43` of `ebp-0x6c` with `ebp-0x3c` into `ebp-0x9c` (0x51ef2);
  stores of `ebp-0x78..-0x70` into `ps.origin` (0x51efa..0x51f1b).
- VERIFIED: `trap_Trace` with a zero box, start `ps.origin[0]`,
  `ps.origin[1]` and `tur.r.currentOrigin[2]`, end `ps.origin`, the player's
  number as the skip entity and mask 0x2810011 (0x51f1e..0x51f8d); a compare
  of 1.0 against the fraction (0x51f97) and a store of the trace's `+0xc` into
  `ps.origin[2]` (0x51fb3). INFERRED: a trace that hits something moves the
  origin's z onto what it hit, and a clear one leaves it.
- VERIFIED: `BG_PlayerStateToEntityState` (0x51fc6), stores of `ps.origin`
  into `ent.r.currentOrigin` (0x51fd7..0x51fec), `AxisToAngles` from
  `ebp-0x9c` into `ent.r.currentAngles` (0x51ffb) and `trap_LinkEntity`
  (0x5200a).

VERIFIED, the leaves (pak0, flag 0x2 root tracks, one key each): the 21
`pb_standMG42gunner_aim_*` root translations read z -39.57 on every `15down`
leaf, -47.46 on every `level` leaf and -55.41 on every `15up` leaf, x between
-11.92 and -17.21, y between 2.85 and 6.01; the root rotations are the
column's own yaw, `45left` +45 through `45right` -45, none on `forward`. The
`fire` leaves carry the same values, except the three `fire_30left` leaves,
which key a second translation at frame 5, (-15.99, 5.62) against (-15.33,
5.35) at frame 0.

### 7.2 What it does

- INFERRED, 7.1's control flow: the placement runs only when the legs anim
  carries `turretanim`. That anim is taken as a two-level blend, its children
  the rows and theirs the columns, in the engine's child order, the reverse
  of the file's (`player-model-anim-system.md`, "Animation indices: the
  animtree"). For `standMG42_aim` that is rows `15up`, `level`, `15down`,
  root z increasing, and columns `45right` through `45left`.
- INFERRED, 7.1's control flow: the column position is
  `n / 2 - yaw / animHorRotateInc`, clamped to the row and split between the
  two columns either side. With 7 columns and 15 degrees the gun's yaw 0
  lands halfway between `forward` and `15left`, not on `forward`: the 0.5
  multiplies `n`, not `n - 1`.
- INFERRED, 7.1's control flow: the barrel's pitch is not read. The row is
  found by height: the target is the player's height above the gun minus
  `tag_weapon`'s height in the gun's frame, which the barrel's pitch moves
  about `tag_aim`. The first row whose root z reaches the target is blended
  linearly with the row before it so the blend's z equals the target; a
  target below the first row or above the last takes that row alone.
- INFERRED, 7.1's control flow: the origin is the blended root translation
  turned by `tag_weapon`'s yaw and added to its x and y, at the player's own
  height, carried into the world by the gun's axis and origin. The body's
  yaw is the blend's root yaw plus the tag's, in the gun's frame.
- INFERRED, 7.1's control flow: the trace lifts the origin onto whatever
  lies between the gun's height and that spot; a clear trace leaves the
  player's own height. Nothing here puts the player on the floor: the z is
  the height the player mounted at, which pmove left on the ground, and the
  mounted pmove arm moves nothing (section 5). vcod's port does the same.

VERIFIED, the replay: `every_captured_gunner_origin_replays`
(`crates/server/src/game/turret.rs`) runs vcod's port of 7.2 over the gun's
`angles2` and the gunner's origin of all 260 mounted snapshots on a turret
anim in the 12.2 capture, both tables and every snapshot between, through
mp_carentan's collision: horizontal residual max 0.099 units, mean 0.049.
The fixture prints one decimal, so that is its rounding. Walking the
children in file order instead misses by up to 3.0 (mean 1.8). The test
feeds the captured origin in, so its z says nothing about the trace; the
same test feeds each snapshot again from 16 units under the floor, and the
trace lifts every one onto mp_carentan's floor at -23.875, the capture's
-23.9.

Not determined:

- Which time `XAnimCalcAbsDelta` samples a root with more than one key at.
  vcod reads frame 0. Only the three `fire_30left` leaves differ, by 0.7 at
  their last frame, and no captured firing snapshot blends them.
- The body's yaw. VERIFIED, 7.1: `BG_PlayerStateToEntityState` is called at a
  lower address than the `AxisToAngles` store. INFERRED: the entity state is
  built before the store, so the yaw never reaches the wire and no capture
  can check it. vcod blends the leaves' yaw rotations by weight. The client
  works the same yaw out for itself and draws it (14.7).
- Whether the goal weights pose anything besides this routine's own deltas
  (the gunner's server-side body for a locational hit, for one). vcod does
  not keep them.

### 7.3 The tags and the client's copy

VERIFIED: the turret model's `tag_aim` is at (-2.98, 0.06, 12.87) and its
children `tag_player` at (-45.09, 0.06, 20.92), `tag_weapon` at (-33.59,
-0.03, 12.12) and `tag_butt` at (-49.63, 0, 10.95), bind positions in model
space out of `xmodel/MG42_bipod`'s parts; `tag_flash` (8.41, 0.15, 15.16) hangs
off the separate `tag_aim_animated` chain under `mg01`. VERIFIED, the gun's own
anims (`standMG42gun_{aim,fire,recover}_foward`, 9 tracks) key only
`tag_pivot`, `mg01` and `tag_flash` with more than one frame.

VERIFIED, `cgame_mp_x86.dll` (1.1): the same two warnings at 0x30062fd0
(`tag_aim`) and 0x30063048 (`tag_weapon`), and `"Turret has no bone:
tag_player"` at 0x3006227c; both warnings are referenced from the function at
0x300279b0 in the Ghidra export. INFERRED: a retail client places a mounted body
itself with the same routine, so the server's `ps.origin` is what a client
predicts against, not what it draws from.

**Open question.** What `playerPositionDist` (`+0x404`) feeds. VERIFIED,
absence: no operand at `+0x404` sits in 0x51488..0x53400. INFERRED: the 42-unit
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
(`cod11-events-and-fx.md`), and asks the client to go back to the stance it
mounted from. VERIFIED, 12.7: a stand mount's release raises 140, a crouch
mount's 141, and after the crouch one `pm_flags` carries no crouch bit, so
the server itself does not restore the stance.

VERIFIED, `TeleportPlayer` (0x51380): a compare of `sessionstate` against 0
(0x51395), `G_TempEntity` for 200 at the old origin and 199 at the new one
(0x513aa, 0x513c6), `trap_UnlinkEntity`, `ps.origin` = the destination with
1.0 added to z (0x51414..0x51419), `ps.eFlags ^= 8` (0x51428),
`SetClientViewAngle` (0x51434), `BG_PlayerStateToEntityState`,
`r.currentOrigin = ps.origin` (0x51457..0x51469), a `test ebx, ebx` (0x5146f)
and `trap_LinkEntity` (0x51477). INFERRED: the temp entities are made only for
`sessionstate` 0 and the relink only when `ebx` is non-zero. INFERRED: the
player lands one unit above where it stood to mount, facing the angles its
entity had, with `EV_PLAYER_TELEPORT_OUT`/`_IN` temp entities only when it is
still playing, and the teleport bit flipped. VERIFIED, 12.7: both releases
in the capture land one unit above the mount origin with 200 and 199 on temp
entities and the teleport bit flipped; the facing is not separable from the
probe's own view there.

VERIFIED, `TeleportPlayer`: each temp entity takes the player's
`s.clientNum` (gentity +0x90, the entity netfield at offset 144; stores
0x513b7 and 0x513d3). VERIFIED: the 200's `G_TempEntity` call (0x513aa) is
handed `ps.origin` (`client+0x14`, 0x513a6). VERIFIED: the 199's call
(0x513c6) is handed the destination argument (0x513c5). INFERRED: that
argument is read before the one-unit lift at 0x51414, so the 199 sits at the
unlifted destination.

VERIFIED, `G_TempEntity` (0x67938): no `svFlags` store. VERIFIED: its origin
goes through three truncating `fistp`/`fild` pairs (0x67995..0x679fe).
VERIFIED: it calls `G_SetOrigin` (0x67a0c). INFERRED: the snapped origin is
the one `G_SetOrigin` stores, so the pair carries an origin snapped toward
zero. INFERRED: the pair is culled by PVS like any entity, not broadcast.

VERIFIED, `SetClientViewAngle` (0x41e30): a test of `ps.pm_flags & 1`
(0x41e56). VERIFIED: a test of `ps.eFlags` byte 1 against 0xc0 (0x41e60).
VERIFIED: a block reads `ps.proneDirection` (+0x368) through `AngleDelta`
and `AngleNormalize180` (0x41e76..0x41e90). VERIFIED: a per-axis loop
stores `ANGLE2SHORT(angle)`, truncated, masked with 0xffff, minus the dword
at `client+0x20f8 + 4i`, to `ps.delta_angles[i]` (0x42060..0x42098).
VERIFIED: the angles are stored to the entity's +0x140..0x148 and to
`ps.viewangles` (0x4209f..0x420e1). VERIFIED: the ps offsets are the
CoDMP.exe playerstate netfield table's (`pm_flags` 12, `eFlags` 128,
`delta_angles` 72, `viewangles` 192, `proneDirection` 872). INFERRED: the
two tests guard the `proneDirection` block, which runs only for a prone
player off a gun and clamps the asked angles to the prone cone before the
loop. INFERRED: the loop runs after that block and the angle stores after
the loop. INFERRED: `client+0x20f8` is `pers.cmd.angles`, the last cmd's.
INFERRED: +0x140 is `r.currentAngles`.

VERIFIED: `setPlayerAngles` (player method 11, 0x44df0) calls
`Scr_GetVector(0)` (0x44e56). VERIFIED: it calls `SetClientViewAngle`
(0x44e60). VERIFIED: its only other calls are `va` and `Scr_Error`
(0x44e24..0x44e43). INFERRED: those two are the not-a-player error path, and
the builtin is otherwise the vector read followed by `SetClientViewAngle`.
INFERRED, from the store order above: a prone player's `angles` read
back after `setPlayerAngles` are the cone's held angles, not the
script's. vcod's builtin holds them through the host's mirror of the
cone (`pmove::ProneCone`, set at each cmd and by `spawn`) since
2026-10-19. INFERRED: the cone `SetClientViewAngle` reads is the live
playerstate's (`ps.pm_flags & 1`, the `eFlags` byte at +0x81 & 0xc0,
`ps+0x368`, `ps+0x370`, 0x41e30), and of the player methods in the 1.1d,
1.5b and UO tables (`tools/re/dump_builtins.py`) only `spawn` moves any of
them inside a script frame; it rewrites the mirror from its own think, so
a `spawn` then a `setPlayerAngles` in one thread reads unheld angles on a
body the spawn stood up
(`setplayerangles_after_a_spawn_in_the_same_thread_reads_the_new_stance`).

The triggers:

- INFERRED, from the call sites: the use key. `Cmd_Activate_f` sets the busy
  byte to 2 (section 4.1), and the next `turret_think_client` finds it not 1
  (0x5235c) and releases. VERIFIED, 12.7: that next `turret_think_client` is
  the same server frame's; the fixture's use cmd `st` 37282 (fixture line
  1272) is released on snapshot 37300 (fixture line 1274), one snapshot
  later and no cmd between.
- INFERRED, from the call sites: death. A killed client's `sessionstate`
  leaves 0 in the stock `CodeCallback_PlayerKilled`, a dead client still
  takes `ClientEndFrame`'s live arm and keeps `pm_flags` 0x40000, and
  `turret_think_client` finds `sessionstate` non-zero (0x5236b) and releases.
- INFERRED, from the call sites: a respawn. `ClientSpawn` (0x426c4..0x426e2)
  tests `pm_flags & 0x40000` and `eFlags & 0xC000` and calls
  `G_ClientStopUsingTurret` on `g_entities[viewlocked_entNum]`, which is the
  path a switch to spectator takes if it goes through `ClientSpawn`.
  VERIFIED, retail captures
  (`crates/server/tests/fixtures/seatspawn/mp_carentan-turret.txt` on
  1.1d, `-turret-uo.txt` on UO 1.51b; `client-probes/probe_turret` with
  `probe_respawn 1`): a gunner the script spawns as a player off the gun
  reads `self.origin` `(1764.48, 1890.38, 3.99)` straight after the
  `spawn` on both, the spot (z 8) after the spawn's 100 ms fall, and his
  spawn-frame snapshot carries the release's 200 and 199 temp entities
  with an emptied ring and the lock cleared. INFERRED: the release's
  teleport is overwritten by the spawn's own origin before the think, so
  the read does not depend on where the gun was. vcod's `spawn` builtin
  thinks on a fresh sim and matches (`turret_ab`,
  `a_gunner_spawned_off_the_gun_reads_retail_s_origin`). The angles the
  script reads there (1.1d `(0, 0, 0)`, UO `(0, -41, 0)` for a spawn at
  yaw 319) are the think's view (cod11-gametypes-re-bel.md 8.6), which the
  same gate holds too. VERIFIED, the 2026-10-20 retakes of both fixtures
  (with the gunner's cmds and `ct=`): a frame later the script reads
  `(1764.48, 1890.38, 1.02)` on both servers, the frame having run the
  probe's two cmds 16 and 32 ms past the spawn (1.1d `ct` 11682 after the
  spawn's 11650, UO 12382 after 12350); the cmd stamped on the spawn's own
  time ran on neither. The yaw is the last of them (1.1d -147, UO -141).
  The same gate sends ours those two cmds as far past its own spawn and
  reads the same text. The rig's own 25 and 50 ms cmds fell 50 ms and
  read -1.01, which was the rig, not the server (VERIFIED, replay).
- INFERRED, from the call sites: deleting the turret. `G_FreeEntity` calls
  `G_FreeTurret` (0x66b6f), which releases any gunner and clears the record.

VERIFIED: `ClientDisconnect` (0x42aac) calls `G_FreeEntity` (0x42bfc).
VERIFIED: `G_FreeEntity` holds a compare of `r.ownerNum` against the freed
entity's number (0x66aef), a store of 0x3ff there (0x66af7), a compare of
`s.eType` against 11 (0x66b01) and a store of 0 to the busy byte at +0x172
(0x66b07), stepping 0x314 bytes an iteration (0x66b1a). INFERRED: the free is
of the player's own entity, and the four run for every entity in a loop, the
busy store only when the owner test matched and the entity is a turret.
INFERRED: a gunner who disconnects leaves its gun
unowned and free without any of the release body: no `rec+0x28` clear, so
the loop sound plays out under `turret_think`'s unowned branch (section 9),
and no stance event or teleport, since the player is gone.

VERIFIED, 1.5b and UO `game.mp*.so`: the sweep is its own exported
`G_FreeEntityRefs` (1.5b 0x6d1cc, UO 0x79a06), which `G_FreeEntity` calls
after the DObj free (1.5b 0x6d396, UO 0x79bdb). Its first loop runs to
`level.num_entities` (`[level]+0xc`; GOT slots 1.5b 0x89a1c and UO 0xb4640
are `R_386_GLOB_DAT level`, 0x89a40 and 0xb4670 `g_entities`) over the
entities in use (`+0x164` 1.5b, `+0x16c` UO; stride 0x31c, 0x34c) and
stores 0 into a pointer at `+0x1a0` (UO `+0x1a8`) equal to the freed
entity, 0x3ff into `r.ownerNum` (`+0x150`, UO `+0x154`) equal to its
number, then on `s.eType` 11 0 into the busy byte (`+0x176`, UO `+0x17e`),
and 0x3ff into `s.groundEntityNum` (`+0x7c`) equal to its number (1.5b
0x6d224..0x6d27b, UO 0x79a53..0x79ab5). The second clears a client's
`+0x2268` (UO `+0x46b8`) pointer to the freed entity over the 64 client
entities in use. The third, which 1.1d has no part of, zeroes any of the
32 words at `level+0x2180` (UO `+0x2184`, 1.5b 0x6d2f4..0x6d31a) equal to
the freed entity: the dropped-item ring `LaunchItem` writes (1.5b 0x4bd87,
UO 0x54f77; cod11-items.md 8). 1.1d's `G_FreeEntity` inlines the first two
loops (0x66ad1..0x66b5c, the `+0x198` pointer, `r.ownerNum`, the busy
byte, `groundEntityNum`, then `client+0x2260`), and its own exported
`G_FreeEntityRefs` (0x68670) holds the same two. INFERRED, off 1.1's
offsets one word down: the `+0x1a0` word is `parent` (cod11-combat.md
11.1's `fire_grenade`) and the client word the lookat trigger
`G_CheckForPreventFriendlyFire` stores (cod11-gsc-object-model.md).

vcod: a gunner's disconnect drops his turret's owner and busy byte
(`ScriptRuntime::client_disconnect`), and `GameHost::free_ground_refs` runs the
`groundEntityNum` part on every protocol, and `Missiles::free_owner_refs`
the missile `r.ownerNum` and `parent` part (cod11-combat.md 13.7).

VERIFIED: `StopFollowing` (0x46b8c) clears `eFlags` 0xC000, `viewlocked`,
`viewlocked_entNum` and `gunfx` on a spectator's playerstate. INFERRED: a
spectator following a gunner inherits those fields through the follow copy
and loses them here.

## 9. The unowned barrel

VERIFIED, `turret_think` (0x5328c): `nextthink += 50`, a test of `ent+0x2e4`,
a `G_GeneralLink` call, a test of `g_entities[r.ownerNum].client`
against null, the loop-sound countdown of section 6.4, a clear of `s.eFlags`
0x400, and a call to 0x524cc with a vector of (`rec+0x1c`, 0) and a third
argument 0 (0x5333b). INFERRED: the countdown, the clear and the slew run only
when the owner has no client. VERIFIED: 0x524cc holds 200.0 (0x75904), loads
of the weapon's `vertTurnSpeed`/`horTurnSpeed` (`+0x3f0`/`+0x3f4`), a multiply
by 0.05 (0x7590c) and a test of its third argument, and `turret_think` is its
only caller. INFERRED: the 200 applies when that argument is 0, so an unmanned
barrel walks back to (rest pitch, 0) at 10 degrees a frame, 200 a second, and
the turn speeds in the weapon file are dead in MP. VERIFIED, 12.8: no axis
moves more than 10 in a frame. INFERRED, 12.8: each axis is capped on its
own. INFERRED: with the owner at
0x3ff the test reads the world entity's client, which is null, so every
unmanned frame takes this branch.

VERIFIED: `turret_controller` (0x53348) hands (`angles2[0]`, `angles2[1]`, 0)
to `G_DObjSetControlTagAngles` for `tag_aim` and `tag_aim_animated`, and
(`angles2[2]`, 0, 0) for `tag_flash` (`docs/protocol-1.1.md`). VERIFIED,
0x524cc re-read with `annotate_func.py` for section 12: an add of
`angles2[2]` into `angles2[0]` with its store (0x524eb..0x524f4); a
two-pass loop holding a multiply by 0.05, an `AngleSubtract` call, compares
against the limit and its negation, and an add stored back into each
`angles2` slot (0x52588..0x525ec); a store of `angles2[0]` into `angles2[2]`
(0x525f4); a second `AngleSubtract` call with the same compares
(0x52642..0x52671); and stores to `angles2[0]` and `angles2[2]`
(0x52676..0x5267f). INFERRED, the order and the branches: the entry adds the
carried `angles2[2]` back into the pitch; the loop steps each axis toward the
target, clamped to the limit; `angles2[2]` takes the stepped pitch; the
second subtract measures that against the entry pitch and clamps it to the
same limit; `angles2[0]` becomes the entry pitch plus the clamped step and
`angles2[2]` what the clamp refused. VERIFIED: the aim step writes
`angles2[2] = 0` (section 6.2). INFERRED: `angles2[2]` carries into the next frame the
part of the pitch step the second clamp refused; starting from 0 with both
clamps at 10 that part is always 0, so the stock slew never tilts the flash.
VERIFIED, 12.8: `angles2[2]` reads 0.0 on every slewing snapshot.

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
stores of 1 and 0 into one condition slot (0x2a454), and another slot written
from the pmove cmd's attack bit. INFERRED: the slot reads 1 exactly when a
mounted bit is set, and the two slots are `mounted` and `firing`, by their
order against the script's condition list.

VERIFIED, 0x322c8 (Ghidra body at `000422c8`): an `eFlags & 0xC000` test,
tests of `pm_flags` 0x1 and 0x2, and stores of 3, 2 and 1 into one local.
INFERRED: the function takes its movetype from that local, and a mounted
player gets 3 when prone, 2 when crouched and 1 otherwise, and those are the
`idleprone`, `idlecr` and `idle` movetypes, so with 0x316f4 putting `pm_flags`
to the gun's stance a stand gun always plays `idle`.

VERIFIED, `BG_PlayerStateToEntityState` (0x2cd94): an `eFlags & 0xC000` test
and a store of `ps.viewlocked_entNum` into `es.otherEntityNum`. INFERRED: the
store runs for a mounted player only, and that is how a second client learns
which turret a player's body belongs on.

## 11. Kill credit

VERIFIED, `player_die` (0x49a48), step 3 in `cod11-combat.md` 5.1: a load of
`g_entities[attacker->s.otherEntityNum]->s.weapon` into the weapon argument,
and tests of the weapon against 0, of the attacker's client, of its `eFlags &
0xC000` and of that entity's `eType` against 11. INFERRED: the replacement
happens only when all four pass (`cod11-combat.md` labels that condition
INFERRED too). INFERRED: a kill from the gun is credited to
`mg42_bipod_stand_mp`, not the gunner's carried weapon, which is what puts the
MG42 kill icon on the obituary.

United Offensive credits the wound to the gun as well. VERIFIED,
disassembly: 1.1d's `Bullet_Fire_Extended` hands `G_Damage` its attacker
argument as both inflictor and attacker (0x68be5..0x68bea push
`[ebp+0xc]` twice), and so does 1.5b's (0x6e50c..0x6e516); UO's hands it
the inflictor argument when there is one (0x7b1e9, `[ebp-0xa0]`, the
vehicle swap of `coduo-vehicles.md` 13 aside), and `turret_fire`
(0x5aaa9) passes the gun there through `Bullet_Fire` (0x7a90f).
`G_DamageClient` (UO 0x4fd4b, 1.1d 0x4aa38) gives
the callback `inflictor->s.weapon` on all three. VERIFIED,
`mp_carentan-dm-turret-spawn-uo-script.txt` and
`-hitfine-uo-script.txt`: the `D;` line names `mg42_bipod_stand_mp`,
where 1.1d's and 1.5b's name `m1carbine_mp`. vcod: on UO the turret
round's hits carry the gun as inflictor and its weapon (`server.rs`, the
end-frame turret pass); `turret_ab`'s spawn gates compare the `D;` and
`K;` records on all three servers.

Not determined from the bytes: the means of death and the hit location a
turret bullet carries through `Bullet_Fire_Extended`. VERIFIED, 12.6: the
capture's `D;` and `K;` lines read `MOD_RIFLE_BULLET` and `torso_upper`, and
the `D;` line names the gunner's carried weapon where the `K;` line names
`mg42_bipod_stand_mp`.

## 12. What the capture measured

The retail capture is two committed files, both from one run on 2026-09-25:
`crates/server/tests/fixtures/turret/mp_carentan-dm-turret.txt` (the
`--save-turret` gunner's side, "fixture line N" below) and
`mp_carentan-dm-turret-script.txt` beside it (the server's `PROBE`, `D;` and
`K;` lines, "script line N"). The recipe is in both headers and in
`client-probes/README.md`, `probe_turret`. Client 0 is the `--probe-team
axis` target, placed 300 units in front of the gun (script line 31); client
1 is the gunner, placed 40 units behind it (script line 32). The gun is wire
entity 298 (fixture line 3, script line 30). The 40-unit spot took the mount
on the first tap, so no spot was moved. One run was taken; section 12.11 says
what it leaves open.

Evidence below is a fixture line. A "snapshot" is one `!trace` line, one per
50 ms server frame; `st` is its `serverTime`.

### 12.1 The mount frame (D1)

VERIFIED, the stand mount: the use cmd is fixture line 88 (`st` 24736,
`buttons` 64); the snapshot at 24750 (line 90) still reads `viewlocked` 0;
the first mounted snapshot is 24800 (line 93). It reads `viewlocked` 1,
`viewlocked_entNum` 298, `eFlags` 49176 (0xC018, the 0x18 it had with 0xC000
over it), `pm_flags` 262144 (0x40000, unchanged), `groundEntityNum` 1023,
`pm_type` 0, `weapon` 12 (unchanged), `legsAnim` 32, `torsoAnim` 0, `gunfx`
0, hint `0:0:0` and `eventSequence` 1 (unchanged: the mount raises no event
on the gunner). The origin already moved, 1738.2,1860.5 to 1745.7,1861.4.
The gun's line 94 reads `angles2` (-16.4, 0.3) against a view of (-16.4,
-130.7), which is the view minus the gun's angles (0, 229), and `eFlags` 8
where the wait phase read 0 (line 43).

VERIFIED, the crouch mount: use cmds at `st` 38282 and 38300 (lines
1366-1367), crouch held in every cmd. The first mounted snapshot, 38300 (line
1368), reads `viewlocked` 1/298 and `eFlags` 49200 (0xC030) but keeps the
crouch: `pm_flags` 262146, `groundEntityNum` 1022, `legsAnim` 111
(`pb_crouch_alert`) and the pre-mount origin. The gun reads `angles2` (-38.4,
0.3) and `eFlags` 8 to 0 (line 1369). The next snapshot, 38350 (line 1373),
reads `pm_flags` 262144, `eFlags` 49168 (0xC010), `groundEntityNum` 1023,
`legsAnim` 544 and a placed origin, with crouch still held.

VERIFIED: `legsAnim` 32 is `standMG42_aim`, 33 `standMG42_fire`, 111
`pb_crouch_alert` and 122 `pb_stand_alert`, by `AnimIndex` in
`crates/common/src/animtree.rs` over the stock paks; 544 and 545 are 32 and
33 with the 512 toggle bit.

VERIFIED, over every snapshot past `wait` where the asked view moves from one
cmd to the next: the snapshot at `T` reads the view of the last cmd stamped
below `T`, except the one at 30150, whose `delta_angles[0]` 10740 is
`ANGLE2SHORT(40)` less the wire pitch of cmd 30132, not of cmd 30148.
INFERRED: a cmd stamped in `[T - 50, T)` runs before frame `T` as a rule,
and 30148 and the use cmd 24736 are the two that reached the server after
their frame; cmd 38282 ran before frame 38300 by the rule. For 24736 no view
moves to say so either way; the reading rests on the mount alone, and the
crouch mount is the counter-evidence to a server that mounts a frame late:
VERIFIED (lines 1366-1368), its use cmd 38282 mounted on 38300, the frame its
stamp puts it in. On the stand mount more
cmds ran in the same frame after the mount, so pmove's stance step (0x316f4,
section 5) and the mounted movetype had run before `ClientEndFrame`; on the
crouch mount the use cmd was the frame's last, the animation step still chose
the crouch idle, and the placement's `turretanim` test (section 7) skipped the
body. INFERRED: the mount lands inside the use cmd, and the body placement and
mounted anim need one mounted pmove step in the same frame.

### 12.2 Where the body goes (D2)

VERIFIED, every mounted snapshot: `ps.origin[2]` reads -23.9, the floor, 31.9
below the gun's origin (all 261 mounted `!trace` lines). VERIFIED: the xy
origin moves with the gun's `angles2` on the same snapshot. The yaw sweep
steps `angles2[1]` -45 to -44 between lines 145 and 174 and the origin moves
from 1761.3,1831.0 to 1761.3,1831.8 on that same snapshot; revisits of one
`angles2` read one origin (0, 45) at lines 250 and 841, (10, 30) at lines 835
and 876, to 0.1.

Below, "behind" is the distance behind the gun's origin along the barrel's
world yaw (229 + `angles2[1]`) and "left" the distance to its left, both
computed from the fixture's origins. VERIFIED, the yaw sweep at `angles2[0]`
0:

| `angles2[1]` | line | origin | behind | left |
|---|---|---|---|---|
| -45 | 145 | 1761.3,1831.0 | 49.2 | 2.4 |
| -38 | 179 | 1761.1,1836.8 | 49.5 | 2.7 |
| -32 | 184 | 1760.2,1841.3 | 49.4 | 3.3 |
| -24 | 190 | 1758.3,1847.2 | 49.2 | 4.0 |
| -14 | 199 | 1753.6,1854.4 | 48.1 | 3.9 |
| -6 | 205 | 1749.1,1859.2 | 47.0 | 3.9 |
| -2 | 209 | 1747.3,1861.0 | 46.7 | 4.7 |
| 6 | 215 | 1743.5,1864.3 | 46.2 | 6.1 |
| 12 | 220 | 1739.7,1868.3 | 46.9 | 5.7 |
| 18 | 225 | 1735.1,1872.4 | 48.1 | 4.7 |
| 24 | 230 | 1730.3,1875.2 | 48.6 | 4.3 |
| 30 | 235 | 1726.4,1875.5 | 47.4 | 5.5 |
| 36 | 240 | 1722.6,1875.5 | 46.3 | 6.6 |
| 42 | 245 | 1718.0,1876.6 | 46.5 | 6.8 |
| 45 | 250 | 1715.4,1877.3 | 46.9 | 6.7 |

VERIFIED, the pitch sweep at `angles2[1]` 0 (negative pitch is up):

| `angles2[0]` | line | origin | behind | left |
|---|---|---|---|---|
| -40 | 498 | 1741.8,1856.6 | 39.6 | 5.0 |
| -32 | 508 | 1743.5,1858.6 | 42.3 | 5.0 |
| -26 | 513 | 1744.6,1859.8 | 43.9 | 5.1 |
| -20 | 518 | 1745.4,1860.8 | 45.2 | 5.0 |
| -14 | 523 | 1746.1,1861.6 | 46.2 | 5.0 |
| -8 | 528 | 1746.5,1862.1 | 46.9 | 5.0 |
| -2 | 533 | 1746.5,1862.0 | 46.8 | 5.0 |
| 4 | 538 | 1746.1,1861.6 | 46.2 | 5.0 |
| 10 | 543 | 1745.9,1861.3 | 45.9 | 5.1 |
| 16 | 548 | 1745.7,1861.1 | 45.6 | 5.0 |
| 22 | 553 | 1745.3,1860.6 | 44.9 | 5.1 |
| 28 | 558 | 1744.5,1859.7 | 43.7 | 5.0 |
| 36 | 564 | 1743.0,1858.0 | 41.5 | 5.0 |
| 40 | 568 | 1742.2,1857.1 | 40.3 | 5.0 |

INFERRED: the body turns with the barrel's yaw but is no rigid offset: it
sits 46 to 49.5 behind and 2.4 to 6.8 left across the yaw arc, and pitch
pulls it in toward the gun in both directions, to 39.6 at full up and 40.3 at
full down. That is the shape of the blended `standMG42_aim` leaves: section
7.2 replays both tables and every snapshot between them to 0.1. The other mounted snapshots (the mount-phase drift,
the flick, fire and the crouch remount) fall between the table's rows and
are all in the fixture.

### 12.3 The arcs, the view and the 15-degree step (D3)

VERIFIED, yaw: while the cmds ask for more than 45 either side, `angles2[1]`
reads -45.0 and the view yaw 184.0 (229 - 45) on every snapshot (lines
145-170), and 45.0 with 274.0 (229 + 45) (lines 250-481). VERIFIED, pitch:
`angles2[0]` -40.0 with view pitch -40.0 (line 498) and 40.0 with 40.0 (lines
568-820). VERIFIED: on those snapshots `delta_angles` moves every frame (for
example 0 to 6371 to 11650 to 15837, lines 145-158), and on the yaw sweep's
in-range snapshots, where the view follows the cmd at 6 degrees a frame,
`delta_angles` holds at 0,21844,0 (lines 174-245).

VERIFIED, the 15-degree step: the flick asks for (0, 60) off the gun from a
barrel at (40, 0), and `angles2` reads (25, 15), (10, 30), then (0, 45), the
last clamped, with the view equal to the barrel plus the gun's angles on each
(lines 830-841: 25.0,244.0; 10.0,259.0; 0.0,274.0) and `delta_angles`
rewritten on each. The pitch sweep's first step from (0, 45) toward (-60, 0)
reads (-15, 30), (-30, 15), (-40, 0) (lines 488-498). The fire phase's first
snapshot steps (0, 45) to (10, 30) (line 877). Each axis moves at most 15 a
frame.

INFERRED: this confirms section 6.2's reading on the wire. The clamp holds the
barrel at the arc and pulls the view back to it every frame the cmd asks
past it. The step limits each axis to 15 degrees a frame and pulls the view
to the barrel on every frame it limits. A view inside the arcs and within 15
degrees of the barrel is left alone.

The pitch-edge hold (lines 574-820) shows `delta_angles[0]` stepping 3642 a
frame. INFERRED: that is the probe, not the server: it writes each cmd's
wire angles as its asked view minus the last snapshot's `delta_angles`, and
the server's rewrite feeds back into the next cmd.

### 12.4 Fire and the loop sound (D4)

VERIFIED: attack is held from cmd `st` 33766 (line 873) to cmd 35000 (line
1076), and the gun fires on 25 consecutive snapshots, 33800 to 35000 (lines
876-1081). Each carries one `EV_FIRE_WEAPON_MG42` (168, parm 0) on the gun's
own ring, the gun's `eventSequence` stepping 1 to 25, one per 50 ms frame.
Every one of those snapshots reads `viewlocked` 2, gun `eFlags` 1032 (0x408)
and gun `loopSound` 2; the gunner's `ps.eFlags` reads 50200 (0xC418) and its
`legsAnim` 545.

VERIFIED: the snapshot after the last shot, 35050 (line 1085), reads
`viewlocked` 1, gunner `eFlags` 49176, `legsAnim` 32, gun `eFlags` 8 and
`loopSound` still 2 (line 1086). The one after, 35100, reads `loopSound` 0
and an `EV_SOUND_ALIAS` (172) with parm 3 on the gun's own ring, entity 298,
`eventSequence` 26 (lines 1090-1091). VERIFIED: `loopSound` 2 and parm 3 are
configstrings 526 and 527, `weap_mg42_loop` and `weap_mg42_cooldown`
(section 2). INFERRED: this is section 6.4's reading exactly: the loop is on
for the last shot's frame and the next, and the cooldown plays on the third,
on the turret, not on a temp entity.

VERIFIED: the 20 shots of the fire phase, from a still gunner at a still
barrel (10, 30), all put their `EV_BULLET_HIT_LARGE` (174) impact at
1648,1492,-31 (lines 879-1031). INFERRED: a turret bullet has no spread
(section 6.3).

VERIFIED: the one instruction between 0x521d4 and 0x524cc that sets 0x400 in
an `eFlags` byte is the byte-wide OR of 0x4 into byte 1 of the gun's
`s.eFlags` at 0x5232f, and the retail carbine capture
`playerstate/mp_carentan-tdm-hit-target.txt` reads `eFlags` 1040 (0x410) on
its line 80, a frame carrying `EV_FIRE_WEAPON`. INFERRED: the gunner's 0x400
is the generic firing flag the attack bit sets, not a turret store.

### 12.5 When the hit lands (D5)

VERIFIED: the target phase fires five rounds, `st` 34800 to 35000. Rounds at
34800 and 34850 hit the world far from the target (lines 1041, 1049). The
round at 34900 (fire `eventSequence` 23) puts an impact at 1518,1612,22, at
the target, and a second at 1248,1308,14 on the world behind it (lines
1054-1060). The round at 34950 (`eventSequence` 24) does the same, and the
same snapshot carries `EV_PAIN` (187) with parm 47 on entity 0 and
`EV_OBITUARY` (201) on temp entity 180 (lines 1064-1072). The round at 35000
reaches only the world behind (line 1080). VERIFIED: the server logged one
`D;` and one `K;` for client 0, both 53 damage (script lines 33-34).
INFERRED: the target spawned at the stock 100 health, which no committed line
of this run carries, so one 53 wounds and the second kills.

INFERRED: the 34900 round is the `D;` hit and the 34950 round the `K;` hit.
The kill's obituary is in the same snapshot as the fire event that caused it,
so a turret round's trace and damage run in the gunner's mounted frame, the
same server frame as the shot, not the next one. INFERRED: the first hit's
`EV_PAIN` (parm 47, the 47 health the 53 left) reaches entity 0 one snapshot
after its round. The victim's entity state is copied from its playerstate in
its own `ClientEndFrame`, and client 0's runs before the gunner's, client 1,
fires. A victim numbered above its gunner would show the pain on the round's
own snapshot. VERIFIED: both hitting rounds carry a second impact on the
world behind the target in the same snapshot (lines 1058-1059 and
1070-1071).
INFERRED: the round goes on past the player it hits, and that is the
rifle-bullet pass-through of `cod11-combat.md` 2.4, step 5, at half damage,
since the stock turret file sets `rifleBullet 1`.

### 12.6 Kill credit (D6)

VERIFIED, script lines 33-34: `D;0;axis;vcod;1;allies;vcod;m1carbine_mp;53;MOD_RIFLE_BULLET;torso_upper`
and `K;0;;vcod;1;;vcod;mg42_bipod_stand_mp;53;MOD_RIFLE_BULLET;torso_upper`.
The wound names the gunner's carried weapon, the kill names the turret's.
INFERRED: the weapon the damage callback is handed comes from the attacker's
own state, not from the turret's weapon def in the round's parameters, and
only `player_die`'s replacement (section 11) swaps in the turret's
`s.weapon`, so `CodeCallback_PlayerDamage` sees `m1carbine_mp` and
`CodeCallback_PlayerKilled` sees `mg42_bipod_stand_mp`. INFERRED: 53 is
`(int)(60 * 0.9f)`, the file's `damage` 60 times `torso_upper`'s 0.9 in
`info/mp_lochit_dmgtable` (`cod11-combat.md` 3.5), truncated below 54 by the
float product. The means of death is `MOD_RIFLE_BULLET` and the hit location
an ordinary body location, which section 11 left open.

### 12.7 The release (D7)

VERIFIED, the use dismount: use cmd `st` 37282 (line 1272), released on
snapshot 37300 (line 1274). That snapshot reads `viewlocked` 0,
`viewlocked_entNum` 1023, `eFlags` 16 (0xC018 lost 0xC000 and the teleport
bit 8), origin 1738.2,1860.5,-22.9, which is the pre-mount origin one unit
up, `groundEntityNum` 1023, `legsAnim` still 32 and hint `0:0:0`. It carries
`EV_PLAYER_TELEPORT_OUT` (200) on temp entity 252, `EV_PLAYER_TELEPORT_IN`
(199) on temp entity 258 and `EV_STANCE_FORCE_STAND` (140) on the gunner's own
ring (lines 1275-1277). The next snapshot (line 1281) reads origin z -23.9,
`groundEntityNum` 1022, `legsAnim` 634 and hint `6:0:0`. The gun keeps its
`angles2` and `eFlags` on the release snapshot; its `loopSound` was already 0.

VERIFIED, the crouch remount's release: use cmd 38800 (line 1418), released
on 38850 (line 1424): `eFlags` 49168 to 24 (the teleport bit set again),
the same pre-mount origin one unit up, 200 and 199 on temp entities 170 and
171, and `EV_STANCE_FORCE_CROUCH` (141) on the ring (lines 1425-1427).
`pm_flags` reads 262144, no crouch bit, on that snapshot and every one after
it.

INFERRED: the release runs in the frame of the use cmd that asked for it: the
busy byte 2 set in the cmd is read by the same frame's `turret_think_client`.
INFERRED: the stance event is all the release does to the stance. The server
does not put the player back into a crouch; a retail client acting on 141
would, and the probe does not act on it.

VERIFIED: the view after both releases reads (0, 229), the gun's own yaw,
with `delta_angles` unchanged across the release (lines 1274, 1424). The
probe's cmds asked for the same view (0, 229) in both phases, so this capture
cannot tell `TeleportPlayer`'s angle apart from the cmd's.

VERIFIED: before the first mount `viewlocked_entNum` reads 0 (line 42), after
a release 1023 (line 1274 onward).

### 12.8 The unowned barrel (D8)

VERIFIED: from the snapshot after the use release, `angles2` reads (-10, 0),
(-20, 0) and so on to (-70, 0), then (-72, 0), one step per frame (lines
1282-1317). After the crouch remount's release it reads (-13.6, -41.6) on the
release snapshot, then (-23.6, -31.6), (-33.6, -21.6), (-43.6, -11.6),
(-53.6, -1.6), (-63.6, 0.0), (-72.0, 0.0) (lines 1420-1457). -72 is the
barrel's rest pitch of the wait phase (line 43). The slew starts on the
snapshot after the release; the release snapshot keeps the barrel where the
gunner left it.

INFERRED: each axis steps toward (rest pitch, 0) by at most 10 degrees a
frame, independently: yaw reaches 0 on a 1.6-degree step while pitch still
takes 10.

VERIFIED: `angles2[2]` reads 0.0 on all 99 `!turret` lines, the 14 slewing
ones included. INFERRED: that is section 9's re-reading of 0x524cc, where the
carried part stays 0 when both clamps are 10, and not the flash tilt an
earlier reading of this document expected while the barrel slewed.

### 12.9 The gunner's anims (D9)

VERIFIED: on the 261 mounted snapshots, `torsoAnim` reads 0 on all of them
and `legsAnim` one of 32, 544 (`standMG42_aim`), 545 (`standMG42_fire`) or,
on the crouch mount's first snapshot only, 111. 545 is on exactly the 25
snapshots that fire (lines 876-1077); the snapshot after the last shot reads
32 (line 1085). The crouch remount reads 544 throughout, with crouch held
(lines 1373-1419). INFERRED: the mounted clause picks `standMG42_fire` on
frames whose last cmd held attack and `standMG42_aim` otherwise, whatever
stance the player mounted from, and its `both` body part reaches the wire as
`legsAnim` with `torsoAnim` 0.

### 12.10 The cursor hint (D10)

VERIFIED: standing 40 units behind the gun facing it the hint reads `6:0:0`
(lines 49-90): `serverCursorHint` 6, `serverCursorHintVal` 0,
`serverCursorHintString` 0, the slot of configstring 1212, `CGAME_USEMG42`
(section 2). Facing away (the wait snapshot, line 42) it reads `0:0:255`.
Mounted it reads `0:0:0` on every snapshot, and on the release snapshot too
(line 1274); the snapshot after reads `6:0:0` again (line 1281). INFERRED:
section 4.3's reading holds: the mounted player's scan leaves early with the
hint 0 and the string it had at the mount.

### 12.11 What this capture leaves open

The refused mount. The fixture's notes (lines 27-30) read `# BROKEN strafe
passed 100 units at bearing 50.5`, `# refused bearing=56.2 dist=126.9`,
`# BROKEN refused tap at dist=126.9` and `# REFUSED ok`. VERIFIED: the tap at
56.2 degrees mounted nothing. It was also 126.9 units out horizontally, at
the edge of the use scan's 128, so the capture does not say which test
refused it. VERIFIED, the strafe (lines 1471-1531): the gunner hit a wall at
about 1707,1888 and slid along it at a bearing of 45 to 46 degrees until it
was past 100 units. VERIFIED, a 30 by 30 by 70 box swept with
`vcod_common::collision::CollisionWorld::box_trace` over mp_carentan's BSP
(vcod's clip, a box rather than retail's capsule): no point at a bearing of
50 or more and inside 100 units of the gun is reachable in a straight line
from the 40-unit spot, and a strafe either way from it stops at a bearing of
34 to 44. INFERRED: the nest's side walls stand at about 44 to 46 degrees off
the gun's back, so no placement behind this gun gives the strafe what it
needs, and an arc refusal here needs either a stop just past 45 along the
right-hand wall (about 60 units out) or a tap from in front of the gun.

VERIFIED, lines 1491 and 1495: the hint goes from `6:0:0` to `0:0:255` while
the bearing, computed from the two origins, goes from 39.2 to 45.3 and the
angle between the view and the gun from 39 to 45. INFERRED: the gun leaves
the scan's 0.76-cosine cone (about 40.5 degrees) in the same step, so the
hint does not settle the arc either.

The mount-phase drift. VERIFIED: between the mount and the yaw sweep the
barrel's yaw runs 0.3 to -41.6 with no clamp (lines 93-140). INFERRED: that
is the probe re-aiming at the gun from an eye the placement kept moving, not
retail behaviour; the lines still serve as D2 samples.

The release view: 12.7 cannot separate the angle the release sets from the
one the probe's cmds asked for.

## 13. As implemented

The mounted frame is `ScriptRuntime::turret_think_client` over
`game::turret`'s `aim`, `fire_tick`, `loop_tick` and `muzzle`, run once per
server frame for every gunner after the cursor hint, last in the server's
`ClientEndFrame` pass. A use press splits the client's cmds for the tick:
the ones after it run once the touch pass has mounted it (13.1). Body
placement (section 7) runs between the aim and the fire: `vcod_common::turretpose::place_gunner` from the gunner's legs
anim, then the trace down onto the map's collision, then the origin and the
body's yaw mirrored to script.

The release (section 8) is `game::turret::release` on the record and
`release_sim` on the gunner, reached from four places:

- `turret_think_client` releases instead of running the frame when the busy
  byte is not 1 or the gunner is dead or not playing. The use key's busy 2 is
  set in the cmd's own item pass, ahead of the touch pass's `pm_type` gate.
  INFERRED, section 4.1: `Cmd_Activate_f` has no `pm_type` gate of its own.
- a gunner a lower slot's round killed reaches that release on its own turn
  of the same pass, since the round's callback runs inside the shooter's turn
  (`cod11-combat.md` 16.2). One a higher slot's round killed keeps the gun
  until its next frame's turn. INFERRED, 6.1 and 8: retail's
  `turret_think_client` runs inside each gunner's own `ClientEndFrame` and
  `player_die` (0x49a48) calls no release.
- a spawn the script queued releases before the sim is reset, and keeps the
  teleport's temp entities only for a spawn into play, the `sessionstate`
  the script set ahead of it.
- `GameHost::free_entity` releases the record of a manned gun and queues the
  gunner's half, which lands in the same frame's `ClientEndFrame` pass.
- a disconnect frees the record's owner and busy byte only.

A level boundary builds the script runtime and every sim afresh, a
`map_restart` included, so no mount survives one. The unowned think
(section 9) runs for every gun nobody mans right after the entity thinks, so
a released barrel starts home on the frame after its release, as 12.8
measured. A corpse cloned off a gunner killed on the gun is re-read from the
sim at the snapshot build, after the death frame's release, so it reaches the
wire without the mounted bits; the re-read keeps the place and facing the
clone took, since the release teleports the dead player back to its mount
spot. INFERRED: retail's clone copies `ps.eFlags`
(`cod11-combat.md`, the `cloneplayer` table) before that frame's
`ClientEndFrame` releases, so a retail corpse of a gunner killed by `kill`
may carry 0xC000; no capture covers it.

- The placement's trace sees the world's brushes and terrain only.
  INFERRED, from the skip-entity argument 7.1 reads: retail's `trap_Trace`
  also clips entities. The yaw goes to script as the whole of `angles`, pitch
  and roll 0. INFERRED, from the `AxisToAngles` call 7.1 reads on the gun's
  axis times the body's: retail carries a tilted gun's pitch and roll there
  too.
- The muzzle takes `tag_flash`'s distance from `tag_player` with both tags
  turned by the barrel about their own pivots, `tag_aim` and
  `tag_aim_animated` (13.8).
- The round is traced and its hits handed to the damage callback in the frame
  it was fired (12.5), after that frame's script frame. What the callback
  leaves (sim ops, weapon ops, health) is applied a second time there. A
  victim numbered above its gunner takes its damage feedback that frame and
  one numbered below on the next, which is 12.5's INFERRED reading of the
  `ClientEndFrame` order. The victim's `EV_DEATH` and entity state reach the
  wire on the round's own snapshot either way, since entity states here are
  built at snapshot time rather than copied at each client's end frame.
- The damage callback is handed the gunner's carried weapon and the killing
  arm of `finishPlayerDamage` swaps in the gun's (section 11, 12.6).
- INFERRED: the callback's `eInflictor` is the gunner, not the gun. 12.6's
  `D;` line names `m1carbine_mp`, `cod11-combat.md` 4.2 step 5 passes
  `inflictor->s.weapon` on, and the gun's `s.weapon` is
  `mg42_bipod_stand_mp` (section 11), so the inflictor `G_Damage` saw was
  not the gun, whatever `Bullet_Fire` was handed at 0x522f1.
- A round goes on through each player it hits at half damage, the same as
  a carried rifle's (`cod11-combat.md` 2.4, step 5), so the second impact
  12.5 shows on the world behind the target is raised here too; 13.1's
  replay reads it at retail's (1248, 1308) and (1248, 1312).
- The hit-location product is taken wider than a float before the
  truncation, as the x87 takes it: `60 * 0.9` reads 53 as in 12.6, where a float
  product rounds to 54 first.
- A gun with no `stopFireSound` keeps its loop on through the frame the
  timer runs out, as section 6.4's `je` past the clear reads.
- INFERRED: every gunner's aim and fire run in `turret_think_client` before
  any gunner's round is traced, so two gunners in each other's arc can both
  fire in the same frame. Retail traces and delivers each gunner's hit
  inside that gunner's own `ClientEndFrame` pass instead, so its second
  gunner traced is already dead before its own shot runs; a frame like that
  can end in a mutual kill here that retail's per-client order forecloses.
- The hit trace poses a gunner in the view-yaw frame `place_gunner` wrote
  that frame. INFERRED, from the `AxisToAngles` call 7.1 reads: retail's
  placement carries the body yaw off the gun's own axis instead, which
  diverges from the view yaw by up to roughly 45 degrees at the arc's
  edges; unmeasured, no capture pins which pose a live round is traced
  against.
- The gunner's `ps.eFlags` bit 0x400 (`EF_FIRING`) comes from two places on
  ours: `fire_tick`'s shot and `PmoveSingle`'s trigger bit (combat doc
  16.5), which holds for every cmd with attack held, the gunner's own weapon
  ready and its clip loaded. INFERRED and unmeasured: whether retail's ever
  separates from the gun's own shot for a gunner whose own weapon is empty.
- The turret pass's damage callback (`apply_effects`, one `deliver_hits` per hit, in round order) runs after the tick's
  one `take_link_ops` drain, so a `linkTo`/`unlink` or a client spawn its
  script notify queues waits for the next tick's drain rather than reaching
  this frame's wire.
- `activate_ent` drops an unusable turret before the trace pass that would
  otherwise clip it against the world, where retail's use/hint loop traces
  every candidate first and only then steps past one `G_IsTurretUsable`
  refuses (`crates/server/src/game/item.rs`).

### 13.1 The replay against the capture

`crates/server/tests/turret_ab.rs` replays the fixture's cmds on ours and
compares every `!trace` and `!turret` field, the drained events and the
impacts, snapshot by snapshot; `TURRET_REPORT=1` prints every row. How it
pairs the two:

- Our frame that ran the cmds stamped in `[T - 50, T)` pairs with retail's
  snapshot `T` (12.1). The two cmds 12.1 names as late, 24736 and 30148, go
  to the frame after.
- Each cmd goes out as the view retail's server built from it: its wire
  angles plus `delta_angles` of retail's snapshot `T - 50`. `delta_angles`
  and the gunner's own `eventSequence` are compared as their change from the
  first pair, since the join leaves each side its own offset.
- The rig numbers the clients as retail did, target 0 and gunner 1, so the
  victim is numbered below its gunner on both (12.5).
- Ours starts at retail's settled spot, to the fixture's 0.1 print.
  VERIFIED: the `PROBE place` line reads y 1860.19 (script line 32) and
  every pre-mount trace 1860.5 (fixture lines 42-90). INFERRED: retail's
  landing from the placement ended 0.31 units off the spot, and no line of
  the capture says why; our landing stays on the spot. Every
  release teleports back to the mount spot, so the offset would ride through
  every later phase.
- Origins are compared to 0.25 units, `viewangles` to the print plus one
  `ANGLE2SHORT` step, `angles2` to the print plus 0.01 degrees, the rest
  exactly, impacts included: both sides truncate them to whole units.

What the replay found, fixed: the stand mount's first snapshot, 24800, kept
the spot, `groundEntityNum` 1022 and `pb_stand_alert` on ours, where
VERIFIED (fixture line 93) retail reads 1023, the placed origin and
`legsAnim` 32, `standMG42_aim`. All of a tick's pmove steps ran before the
per-cmd use pass, so the cmds after the use cmd ran unmounted. A client's
cmds after a use press now wait for the touch pass and run in a second
round, so the mount lands inside the use cmd as 12.1 reads, and the rig's
`the_first_locked_snapshot_is_already_placed` pins it. The crouch mount,
where the use cmd was the frame's last, still keeps the stance for a frame
on both, and the gate compares those rows with no `GAPS` line.

Also fixed: `G_TempEntity`'s truncation (section 8) now
applies to every temp entity at the wire (`temp_entity::build`), so the
fire phase's twenty impacts read retail's (1648, 1492, -31); the landing
ladder reads the fall height and the land anim is gated on the landing speed
and the legs timer (`cod11-sound-system.md`, "Landing"), so a release's
one-unit drop lands in silence and keeps `pb_stand_alert`, as both releases
read.

What it lets through, each a `GAPS` line in the gate:

- The kill snapshot, 34950: VERIFIED (12.5), retail's carries the victim's
  entity with the `EV_PAIN` (parm 47) the round before raised; INFERRED
  (12.5), copied in the victim's own `ClientEndFrame` ahead of the gunner's.
  Closed 2026-10-17, VERIFIED, vcod measurement: `turret_ab` matches the
  kill snapshot's events on all three captures (1.1, 1.5, UO) once a
  client's entity goes out as its own end frame copied it (13.3).
- The wounding round's impact at 34900 enters a truncation step off
  retail's (ours 1517, 1611; VERIFIED retail 1518, 1612 at line 1057). It
  moves with the spawns: ours read (1517, 1611), then (1518, 1611) once the
  spawn's own think ran each client up to the frame's clock, and (1517,
  1611) again once that think's move picked the idle on the spawn frame
  (`cod11-spectator-follow.md` 13). INFERRED: the pose the round meets is
  the idle's phase, which runs from each side's own join. VERIFIED,
  `mp_carentan-dm-turret-victim.txt` line 1080 as committed in 102ecab0
  (the 2026-10-17 run): a second retail run of the same recipe put that
  round at (1517, 1611), ours; VERIFIED, the 2026-10-19 run now under that
  name, line 1083: a third, the same. The gate's legs toggle
  is compared as its change from the first sample for the same reason.
- The killing round's impact at 34950: ours (1515, 1610), VERIFIED retail
  (1514, 1609) at fixture line 1068. The target the round before knocked
  back has moved for however much of its own cmd clock retail's frame ran,
  and the target is the other client, whose clock this capture does not
  carry. 13.3 measures it, and the replay that follows it matches the
  round. The row stays in this capture's gate for want of that clock;
  13.4 closes it on a capture that logs it.
- The crouch release's one-unit drop: at 38900 ours reads grounded 0.2 above
  the floor (z -23.66), where VERIFIED (fixture lines 1431 and 1436) retail reads
  `groundEntityNum` 1023 at the same printed height and 1022 at -23.9 on the
  next snapshot; the hint and the legs follow
  the ground. The stand release lands on the same snapshot on both, and the
  capture holds one of each. Ours then stays 0.21 above the floor, inside the
  origin tolerance. Closed 2026-10-08: VERIFIED, vcod measurement, with the
  leaf walk's partition bounds test ported (`cod11-player-clip.md` 8.12)
  `turret_ab` matches the ground, hint, hint string and legs rows at 38900.
- The strafe's footsteps were a phase off retail's: ours raised the first
  at 39500 and a second at 39950, where VERIFIED (fixture lines 1495-1496
  and 1530, event 6 and `eventSequence` 6) retail raises one at 39600 and
  none after it before the strafe ends. Closed 2026-10-13: VERIFIED, vcod
  measurement, `turret_ab` matches every event and `eventSequence` row of
  the strafe and the refusal once `PM_Footsteps` scales the cycle by the
  speed over the full speed and truncates it (`cod15-movement.md` 5,
  "Footsteps"; the cause was the cadence, not the join's phase).

The strafe also slid along the nest wall into the sandbags at 39800, where
VERIFIED (fixture lines 1512-1513) retail steps up with `EV_STEP_VIEW` parm
139, 11 units, and ours raised 142, 14 units, with every origin after it off
by up to 5 units. That was unmounted pmove, not the turret, and two ports
closed it; what they are is in `cod11-mantle.md`, "The step-up stops a unit
short" and "A steep plane still steers the fall". The step, the slide down
the 52-degree face, the landing and every origin of the strafe and refused
phases now match.

The kill credit matches as well: after the replay, ours logs the same `D;`
and `K;` records as script lines 33-34 once the client numbers and names are
dropped, `m1carbine_mp` on the wound and `mg42_bipod_stand_mp` on the kill,
53 and `torso_upper` on both. Since 2026-10-18 the gate on this capture is
gone (13.4) and the `-spawn` gates compare the credit instead.

### 13.2 The live run against ours

A second run of the same recipe, live rather than replayed, taken again on
2026-09-25 after the round-split fix landed: `vcod-server` on mp_carentan
(port 28990, `--gametype-script client-probes/probe_turret.gsc
--set probe_teleport=1`), the axis client second
(`--net-probe 127.0.0.1:28990 --probe-team axis --probe-secs 180`), the
gunner client third (`--net-probe 127.0.0.1:28990 --save-turret
--probe-secs 170`). It overwrote the committed `mp_carentan-dm-turret.txt`;
that capture moved to a scratch copy (the pre-fix run stays in another
for comparison) and `git checkout` restored the
fixture directory, `git status` clean afterward.

VERIFIED, D1: the use cmd (`buttons=64`) reaches the server and the very
next snapshot already reads `viewlocked=1`, `viewlocked_entNum=298` and
`eFlags=49176` (0xC018), matching retail's own first-mounted values --
unchanged from the pre-fix run. VERIFIED: body placement now lands one
frame later instead of two -- the scratch copy's line 88 is the use
cmd (`st=36216`), line 93 (`serverTime=36250`, the very next snapshot)
already reads `viewlocked=1` but keeps `groundEntityNum` 1022, the
pre-mount origin and `legsAnim` 634, and line 97 (`serverTime=36300`, one
50 ms frame later) reads 1023, the placed origin and `legsAnim` 32.

INFERRED: the round split closed the first of the two gap ticks the pre-fix
run showed (12.1's `pb_stand_alert` mismatch, fixed per 13.1); the second
cause 13.1 already named for that run -- "a tick that ran no cmd at all...
with no mounted pmove step the anim stays unmounted, and retail's placement
needs one too" -- still applies here and is now the whole of the gap: no cmd
lands in the round that mounts the gun (the use tap is the only cmd that
tick; the next cmd the client had already queued arrives too late over the
wire to fall into that tick's second round), so `update_anims` cannot pick
the mounted pose until the following tick's own cmd runs, and
`turret_think_client`'s placement reads that same lagging `legs_anim()`.

INFERRED: this is not a regression from 13.1's replay result -- the replay
feeds every post-use cmd at once, so its second round always has one to run
in the same tick and places same-frame; a live client sending at the
probe's default ~16 ms only sometimes has a second cmd queued by the time
the mount's tick runs, and this capture is the case where it did not.

INFERRED, correcting the claim below against 12.1: retail's own capture
does not show a same-frame placement across the board. 12.1's crouch mount
(VERIFIED there, lines 1368 vs 1373) already carries this same
one-tick-old-pose shape, for the reason 12.1 itself gives: that use cmd was
the frame's last, so no further pmove step ran behind it that frame. Only
retail's *stand* mount avoids it, because extra cmds happened to run in the
same frame after that particular use tap. So a 125 fps client's tighter cmd
spacing makes a same-frame placement more likely, not certain, and 12.1's
own crouch capture is the counter-example already in this document.

VERIFIED: the crouch mount on ours reproduces that native one-tick shape
rather than adding to it, on this run or the last -- the first mounted
frame already reads `eFlags=49200` (0xC030) with the crouch held and
`groundEntityNum` 1022, and the next reads `pm_flags` 262144, `eFlags=49168`
(0xC010), `groundEntityNum` 1023 and the placed origin, field for field what
12.1 reads for retail's crouch mount.

VERIFIED, D3: `angles2[1]` clamps at exactly -45.0/45.0 and `angles2[0]` at
exactly -40.0/40.0 through the sweep, the same arc as section 2's stock
file and 12.3's capture.

VERIFIED, D4: the fire phase writes one `EV_FIRE_WEAPON_MG42` (168) on the
gun's ring every 50 ms frame, `viewlocked=2`, gun `eFlags=1032` (0x408),
`loopSound=2`, gunner `ps.eFlags=50200` (0xC418) and `legsAnim` 33/545
throughout, and all twenty impacts of the fire phase land at (1648, 1492,
-31), retail's own value from 12.4 -- the `G_TempEntity` truncation fix
13.1 already logged holds on a fresh capture. The loop clears two frames
after the last shot and the cooldown (`EV_SOUND_ALIAS` parm 3, cs 527)
plays on the frame after that, the same two-frame shape 12.4 reads.

VERIFIED, D5/D6: the target phase's two hitting rounds land at (1517,
1611) and (1516, 1612), each about a unit off retail's (1518, 1612) --
the truncation-step difference 13.1 already attributes to where the round
enters the posed bone box, reproduced here rather than newly found. The kill's `EV_OBITUARY`
(201) rides the same snapshot as the killing round's fire event, as 12.5
reads; the wounding round's `EV_PAIN` on the victim's own entity does not
reach the gunner's snapshot, the entity-state-at-snapshot gap 13.1 already
lists. The server's own log carries
`D;0;axis;vcod;1;allies;vcod;m1carbine_mp;53;MOD_RIFLE_BULLET;torso_upper`
and
`K;0;;vcod;1;;vcod;mg42_bipod_stand_mp;53;MOD_RIFLE_BULLET;torso_upper`,
identical to script lines 33-34 once the two runs' shared client numbers
and name are set aside.

VERIFIED, D7: the release snapshot reads `viewlocked=0`,
`viewlocked_entNum=1023`, `eFlags=16` (0xC018 losing 0xC000 and the
teleport bit) and the origin one unit above the pre-mount spot, carrying
`EV_PLAYER_TELEPORT_OUT` (200), `EV_PLAYER_TELEPORT_IN` (199) and
`EV_STANCE_FORCE_STAND` (140) -- the same fields and events 12.7 reads on
retail's stand release. The crouch release reads the same shape with
`EV_STANCE_FORCE_CROUCH` (141) in place of the stand event and `eFlags` 48
losing to 16, matching 12.7's crouch-release reading too.

VERIFIED, D8: the unowned barrel steps (-10, 0), (-20, 0) ... (-70, 0),
(-72, 0), one 10-degree step a frame, starting the frame after the
release -- the same table 12.8 reads off retail.

VERIFIED, D9: `torsoAnim` stays 0 throughout; `legsAnim` carries 33/545
through the fire snapshots and 32/544 (the same anim, the 512 toggle bit
differing run to run) everywhere else mounted, matching 12.9's reading
that the mounted clause distinguishes only fire from aim.

VERIFIED, D10: the hint reads `6:0:0` standing off the gun, `0:0:0` on
every mounted snapshot including the release snapshot, and `6:0:0` again
the frame after -- 12.10's reading reproduced.

VERIFIED: the run's own notes carry the same shape as retail's --
`# BROKEN strafe passed 100 units at bearing 49.1`,
`# refused bearing=53.1 dist=118.8`, `# BROKEN refused tap at dist=118.8`,
`# REFUSED ok` against retail's 50.5/56.2/126.9/ok -- the refused mount
still refuses and the strafe still exhausts its 100-unit cap before
clearing the arc, with the exact bearings differing the way 13.1's
capsule-vs-box clip difference already predicts for that phase.

No mismatch outside the `GAPS` list 13.1 already carries turned up in this
run: every divergence found here is one of the six already named there,
measured a second time on a fresh capture rather than a new finding. D1's
one-tick lock-to-placement gap is not a seventh: 13.1 already named its
cause (a tick with no cmd to run) as the half of the old two-tick gap the
round split does not touch, and this run is that half on its own, not a
regression from it.

### 13.3 The target's clock

`--save-turret` writes a `!victim` line per fire and target snapshot: the
other player's `pos.trTime`, `pos.trBase`, `pos.trDelta`, legs and torso
anims, `eFlags`, `solid` and `eventSequence`. Three runs of the 13.1
recipe on 2026-10-17 carry it: `mp_carentan-dm-turret-victim.txt` (1.1d,
replaced on 2026-10-19 by a run that also prints the start at full
precision; the old one is in git at 102ecab0), `-victim-15.txt` (1.5b) and
`-victim-uo.txt` (UO 1.51b), each a `--capture-tag` run beside the
`--probe-team axis` client.

- VERIFIED, the 2026-10-17 `-victim.txt` (102ecab0) lines 1078 and 1089:
  the target's `pos.trTime` reads 31932 on the wounding snapshot (31950) and 31998 on the kill
  snapshot (32000), so retail's kill frame ran 66 ms of the target's cmds,
  not 50; every earlier target snapshot reads its time less 18. Line 1089:
  `trBase` (1512, 1600, -31), three units back along the ray from the
  (1515, 1603) of every earlier line, `trDelta` (-30, -33, 0), `legsAnim`
  94 (`pb_combatrun_forward_loop`) and `solid` 0. The kill's impact
  (line 1092) is (1514, 1609), the first capture's to the unit.
- VERIFIED, `-victim-15.txt` lines 1065 and 1076 and `-victim-uo.txt` lines
  1021 and 1031: there the kill frame ran 50 ms of the target's cmds,
  `trBase` (1513, 1601), `trDelta` (-36, -40, 0), the run anim, `solid` 0.
  1.5b's kill impact (line 1079) is (1516, 1611), which is where ours put
  the first 1.5 capture's.
- VERIFIED, all three wound snapshots: `trDelta` reads (0, 0, 0) on the
  snapshot of the round that knocked the target back, and the knockback
  shows only on the next. INFERRED: the entity state is the copy the
  target's own `ClientEndFrame` made (`BG_PlayerStateToEntityState`, combat
  doc 5.4), ahead of the gunner's turn, and `finishPlayerDamage`'s velocity
  goes into the playerstate only. The kill snapshot's `solid` 0 is
  `player_die`'s relink as a corpse (combat doc 5.1, step 13).

vcod: `ClientSim::end_frame_entity` keeps the entity state each client's
own end-frame turn built, and the frame's snapshots send that copy; a client
a higher slot killed after its turn still goes out on it, `solid` 0. The
rig's target sends three cmds a frame, as the net probe does, or on a
`-victim` capture the cmds its `pos.trTime` names, one every 16 ms back to
the last snapshot's (INFERRED, the probe's `--probe-cmd-ms`).
`turret_ab`'s three `the_victim_capture_matches_*` gates diff every phase
of those captures, the target's entity included: the killing round matches
on 1.1, and 1.5 and UO each keep one impact a truncation step off along
the same ray with the target's entity matching to the unit (the wound on
1.5, the kill on UO; the idle phase, as the wounding round in 13.1).

The other phases, measured 2026-10-17 (`turret_ab.rs` `TURRET_REPORT=1`):

- VERIFIED, the sweep: a snapshot may already show a cmd stamped at or
  past its own time. `-victim-15.txt` 23550 and 23750 read pitch -32 and
  -8, the view of the first of two cmds stamped 23550 (sent at probe ms
  5284) and of the first 23750 (5483); `-victim-uo.txt` 20650 reads yaw
  145, cmd 20651's (1077), and 25150 pitch 4, the first 25150's (5576).
  `-victim-15.txt` 20000 reads -141, cmd 19969's, so 19985 (1733) had not
  run yet. INFERRED: the probe sends one cmd a message
  (`Connection::send_frame`), and a snapshot shows every cmd that reached
  the server before its frame, whatever the stamp says; the stamp rule of
  12.1 is the usual case of that. The gates list them (`EARLY_VICTIM_15`,
  `EARLY_VICTIM_UO` by send ms, since the probe repeats a stamp, and
  `LATE_VICTIM_15`); ours then matches every sweep snapshot.
- VERIFIED, UO's strafe at 34800: retail's body moved 10.5 units since
  34750 and 2.9 by 34850, ours 7.4 and 6.0. The first of two cmds stamped
  34800 (15229) ran before frame 34800; listed, ours matches.
- 1.5b's strafe step (the 142 against retail's 139, VERIFIED at 33900):
  not the step. Ours ran 0.2 units further along +y than retail from
  33750, the frame the body meets clip brush 7 (`clip_nosight_dirt`, face
  normal (0.9965, 0.0838, 0)), and so came onto the sandbags' 52-degree
  face further up. INFERRED: cmd 33700 (15446) reached retail's server
  behind 33716, which then ran from 33682, and `SV_UserMove` (1.5b
  `cod_lnxded` 0x808cade, the `cmdCount` strings) skips a cmd stamped at
  or below the last one run. Dropped, ours matches 33750 to 0.02 and the
  step raises 139 at retail's origin. The landing at 34050 is the same
  shape with 34016 behind 34032; of the cmds near it only that drop leaves
  no row (34000's leaves 0.17 units and a legs anim). These two rest on
  the fit alone: no capture logs what order the server received cmds in.
- 1.1d, the refused phase: closed, the gate's start was off. After the
  step at 36900 (parm 142, ours the same) the fall down the face drifted
  ours along -x, at least 0.05 off by 37000 and 0.16 by the landing at
  37050, while y stayed within print; the 0.256 at 37350 was that drift
  where the print rounds it past the gate's 0.25. No cmd early, late or
  dropped reproduces it (each of 36866..37084 shifted, each of
  36383..36900 dropped), and the 1.5b and UO falls down the same face
  match. The cause is the rebase (see the last item).
  - VERIFIED, 1.1d disassembly: the air move is not the difference.
    `PM_AirMove` (0x2f03c) runs `PM_Friction`, then `PM_CmdScale`
    (0x2e5bc: `ps.speed * max / (127 * total)`, times
    `walkSpeedScale` (ps+0x34c) under `pm_flags` 0x80 and
    `runSpeedScale` (ps+0x350) otherwise). Then comes the accelerate
    inline: `max(wishspeed, 100)` (0x708ec) times the frame time,
    capped at the lack, divided by `ps.friction` (ps+0x37c, always 1)
    under a ground entity. Last are the overbounce clip on
    `pml.groundPlane` (0x2f1d3) and `PM_StepSlideMove(1)`. vcod's
    `air_move` is that.
  - VERIFIED, ours: x moves by the accelerate alone, since the face's
    normal is (0, 0.789, 0.614) (brush 6's bevel, which the step's down
    pass lands on at its +x end). The drift is one velocity snap. The
    step cmd (36884) lands 13.92 above its start, and the step's speed
    scale (0x35761: `0.2 + 0.8 (1 - dz / 18)`, the 0.2 being 0x3e4ccccc
    at 0x70f14) takes vx from -35.44 to -13.516. That snaps to -14.
  - INFERRED, from the prints: the snapshots after the step read about
    1 unit/s less in x on retail (0.06 to 0.16 over 66 to 166 ms), so
    retail snapped -13. That needs a landing at least 0.0102 higher on the
    bevel, or a walk velocity 0.12% slower into the step. The 0.1 prints
    cannot tell the two apart.
  - VERIFIED, live (retail 1.1d, 2026-10-18, fixture
    `mp_carentan-dm-turret-full.txt`, the probe's `!trace` now carrying
    `exact=` and `velocity=`): the same strafe, a different run. It
    steps 7 onto the face (parm 135) and falls down it, and ours lands
    on retail's origin to the bit on every snapshot from the strafe on
    (`the_fall_off_the_sandbags_matches_retail_to_the_bit`). So the step
    scale, the air move and the slide down the face are retail's.
  - VERIFIED, ours, 2026-10-19: the 142 step's rounding edge is the
    gunner's start. Retail's placement on `-victim.txt` landed at
    (1737.9, 1857.8) printed, 2.4 short of the spot ours lands on, so
    the gate moves ours to the print, and the dismount puts the gunner
    back on that spot before the strafe. The print hides up to 0.05 per
    axis. Over that cell, from y 1857.8 ours steps to z -11.4273 and the
    step scale leaves vx at -13.52, which snaps to -14: the drift. From y
    1857.79 down (1857.795 at x 1737.94; x matters little) the step
    lands at -11.3887, 0.04 higher, vx snaps to -13 and every row of the
    capture matches, the 37350 landing included. Retail's gunner stood
    in that band. The gate starts ours in its middle, (1737.9, 1857.77)
    (`START_VICTIM`), and lists no gap. The constraint comes from this
    one row: no other snapshot of the capture tells the cell's points
    apart.
  - VERIFIED, live (retail 1.1d, 2026-10-19, the recipe rerun with
    `exact=` on every `!trace`; it replaced `-victim.txt`, server log
    `-victim-script.txt`): retail's placement landed at (1737.9427,
    1857.7877, -23.870617), line 46, inside the band the fit named. The
    gate now starts ours at the first `!trace`'s `exact=`
    (`Capture::start`, read by `parse`; `-full.txt` takes it too and still
    matches to the bit) and `START_VICTIM` is gone.
  - VERIFIED, ours: on this run the 0.1 print start passes too, though
    its strafe steps 142 again (parm 142 on its `strafe` event).
  - VERIFIED, the same run: the kill (25600, line 1095) hits the knocked
    back target at (1515, 1610). Ours from a clock of its own put it at
    y 1611.043, a truncation step on along the ray; on retail's level
    clock (`rig_aligned` off the log's `PROBE place 0 ... 8950`) it lands
    at 1610.89 and the row matches. INFERRED: the target's run clip
    (`legsAnim` 94 on line 1092) starts at a phase off the level time
    (`animclock`), so the pose the round meets needs retail's clock, as
    the `-spawn` gates (13.4) already replay. The sweep's 16450 (line
    194) reads yaw -157, the first of two cmds stamped 16450 (sent at
    probe ms 1571); listed as `EARLY_VICTIM`, every row matched. Without
    `ct=` nothing else tells that cmd's frame (see the next item), so
    since 2026-10-20 the gate on this run diffs only the phases from the
    dismount on (`AFTER_DISMOUNT`), which hold the 142 step and the fall.
  - VERIFIED, live (retail 1.1d, 2026-10-20, port 34201, the recipe rerun
    with `ct=` on every `!trace`: `-victim-ct.txt` and its server log
    `-victim-ct-script.txt`): the placement landed on the spot,
    `exact=` (1738.2423, 1860.1884, -23.874968) on line 47 (the spot
    the gsc prints, (1738.24, 1860.19)), the target at 9500 and the
    gunner at 12750 (`PROBE place`, script lines 26 and 28). Its strafe
    steps 139 (parm 139 on line 1532) and falls down the face. The two cmds
    stamped 24950 (sent at probe ms 9237 and 9253, lines 847-848) both
    ran in frame 24950 (`ct=` 24950, line 849), as 16450 did on the
    2026-10-19 run. Of the cmds stamped at a snapshot's own time that
    `ct=` shows, a repeated stamp ran there 7 times out of 8 in this run
    and none of the 328 single stamps did; 1.1d's `-spawn.txt` ran both
    of its 2 pairs there, 1.5b's `-spawn-15.txt` neither of its 2 and
    UO's `-spawn-uo.txt` 6 of 10 (and one single), so the stamp alone
    cannot place one (a count over the `ct=` captures' `!cmd` and
    `!trace` lines).
  - vcod: `the_victim_capture_matches_retail_on_mp_carentan` replays
    `-victim-ct.txt` with every cmd on the frame `ct=` names
    (`frame_of`) and each one sent at its own stamp (`Rig::aligned`; the
    gate first sent the gunner's cmds on our clock, which stamped those
    two 24949), and lists no gap: every phase matches, the `ct` row
    included.

### 13.4 The target's spawn and cmds

Three more runs of the recipe, each a `--capture-tag` run whose target is
`--probe-team axis --probe-fall` (its stdout logs every cmd stamp it sent,
`CMDS`, and its own playerstate, `FALL`) and whose gsc appends `getTime()`
to each `PROBE place` line: `mp_carentan-dm-turret-spawn.txt` (1.1d),
`-spawn-15.txt` (1.5b) and `-spawn-uo.txt` (UO 1.51b), each with a
`-script.txt` (the server log) and a `-target.txt` (the target's `CMDS`
and `FALL` lines) beside it. The first takes were on 2026-10-17; all three
were retaken on 2026-10-20 with the gunner's `!trace` lines carrying `ct=`
(below), the UO one with `probe_spawnlog 1`. The measurements in 13.4 to
13.7 that name a `-spawn` capture's times were made on the 2026-10-17
takes (in the tree up to a78fced2) unless they say otherwise.

- VERIFIED, `-spawn-target.txt` lines 9-10: the target reads `pm_type` 4
  and health 0 at 11050 and stands at the placed spot with health 100 at
  11150, the time `-spawn-script.txt` line 25 gives the placement; `FALL`
  prints on every health change, so the spawn and the placement share the
  frame. The same holds at 11300 on 1.5b (line 25) and 11950 on UO (line
  30).
- VERIFIED, `-spawn-target.txt` lines 11-18: the target falls from the
  placement on its own 16-17 ms cmds, lands at `commandTime` 11482 with
  `legsAnim` 99 (the `land` event's anim, `player-model-anim-system.md`)
  at 11500, and idles (634) by 11650.
  vcod measurement on the first take: our join's 50 ms cmds landed it a
  frame off with no land anim, so its idle ran from another time than
  retail's.
- VERIFIED, `-spawn-target.txt` lines 38-39: the wound shows on 30350
  (`pm_time` 50, `pm_flags` 0x40200, health 47) and the kill on 30400.
  1.5b: 29950 and 30000 (`-spawn-15-target.txt` lines 36-37); UO: 31100
  and 31150 (`-spawn-uo-target.txt` lines 37-38).
- VERIFIED, `-spawn-uo-script.txt` line 28 (`PROBE spawnat`): the UO
  retake's target spawned at (192, 1280, 0.13), yaw -135, and stands at
  the placed spot with velocity 0 (`-spawn-uo-target.txt` line 10).
  `spawn_gate` pins that spawn, as `hit_gate` does. vcod measurement: an
  earlier UO retake without `probe_spawnlog` (not kept) read velocity
  (5, 5) at the placement and its target stood 1.7 units off the spot
  when the rounds met him; ours, spawned elsewhere, stood on it. A spawn
  point that is not flat leaves the target a drift.

vcod: `turret_ab`'s `rig_aligned` runs our server alone until the target's
placement lands on retail's `PROBE place` time, sends the target's cmds at
the stamps its `CMDS` lines list from its join on (`common::join_pair_stamped`,
`Rig::target_cmds`), holds until our clock is retail's, and from there
replays every frame at the level time of the retail snapshot it pairs
with, the gunner's cmds on their own stamps. `spawn_gate` diffs every
phase and the target's playerstate against its `FALL` lines on the
snapshots that carry its entity.

- vcod measurement: on 1.1d and 1.5b every row matches, the wounding and
  the killing rounds' impacts included, which the earlier captures kept as
  `ENTRY_POINT` and `KILLING_ROUND` rows for want of the spawn time and
  the stamps. A retake of an earlier capture with both logged is a
  `-spawn` capture, so since 2026-10-18 `turret_ab` no longer replays
  `-turret.txt`, `-turret-15.txt`, `-turret-uo.txt`, `-victim-15.txt` or
  `-victim-uo.txt`; they stay committed as the evidence 12, 13.1 and 13.3
  cite. `-victim.txt` keeps a gate for the 1.1d fall of 13.3; since
  2026-10-19 it is a third run, with its server log, replayed on retail's
  level clock from its exact start, and `-victim-ct.txt` (2026-10-20)
  holds the whole recipe with `ct=` (13.3). Before the target's
  stamps were replayed the 1.1d kill was (1516, 1612) against retail's (1516, 1611); our entry point was 1612.054
  in y.
- vcod measurement: on UO the wound matches and the kill enters at x
  1513.0006 against retail's 1512 after truncation, with the target's
  entity and playerstate matching retail's at the kill, until the pose
  fix of 13.7 put it on 1512. It was the pose, not the trace's arithmetic
  (13.5): UO's target idles a frame ahead (13.6, 13.7).
- VERIFIED, the gunner's own cmds on the first takes, read off each
  snapshot's view: a cmd stamped a ms past the snapshot time and followed
  by one stamped a ms below it ran before that snapshot (1.1d 21551, UO
  22101 and 22551), and the one below never showed; 1.5b's 21184 and 21234
  and UO's strafe 36284 reached the server a frame late. The gates listed
  them by fit (`EARLY_SPAWN*`, `LATE_SPAWN*`).
- VERIFIED, the retakes' `ct=`: each `!trace` logs the snapshot's
  `commandTime`, the stamp of the last cmd its frame ran, so a cmd shows
  first in the first snapshot whose `ct=` reaches its stamp. Against the
  rule "the first snapshot past its stamp", 1.1d's 24798, 24899, 29033,
  34184 and 36384 ran a frame late, and the first of each pair stamped
  26400 and 32000 a frame early (`ct=` 26400 at 26400). 1.5b's 19933,
  20049, 26332, 27332, 30386, 30434, 31848, 35399 and 35949 ran late. UO's
  20198, 22682, 22783, 24949, 30149 and 33348 ran late and the first of
  each pair stamped 25200, 30300 and 31200 early. A cmd stamped at or
  below one already run lands in that frame and runs on neither server.
  vcod: `turret_ab`'s `frame_of` places every cmd off `ct=` and the gate
  diffs ours' `commandTime` against it on every snapshot, so no list is
  left; on all three servers every frame ran the same last cmd as
  retail's. `ct=` cannot tell a cmd overtaken by a later one (13.3's 1.5b
  strafe) from one that ran: both sit below the later stamp.
- VERIFIED, `-spawn-uo.txt`: UO's turret round rolls `g_tracerChanceLMG`
  on each player hit (`uo_turret_round`, `coduo-diff.md` 21). The first
  take drew a tracer on both hits; the retake on the wound at 31100 and
  not on the kill at 31150. Ours rolls off its own seed, so `spawn_gate`
  hands a UO replay retail's outcomes, one per hit in order
  (`Server::pin_tracer_rolls`); the draw still runs, so later draws stay
  where they were.

### 13.5 The hit point to the last bit

The `-spawn-uo` row is a whole unit apart only after truncation, so a
fourth run of the recipe on 2026-10-17 logs each hit's point and
direction with `probe_hitlog 1`. It is `mp_carentan-dm-turret-hitfine-uo.txt`
with its `-script.txt` and `-target.txt`. `probe_turret.gsc` wraps
`level.callbackPlayerDamage` and prints each component as whole,
thousandths and the rest. The engine has no `int()`, so the gsc takes the
whole part by halving steps (`whole`), each subtraction exact.

- VERIFIED, `-hitfine-uo-script.txt`: the wound at 30150 enters
  `torso_upper` at (1519.957764, 1612.592651, 22.911320) and the kill
  at 30200 at (1513.656006, 1608.460938, 22.762277), both for 53.
- vcod measurement (`the_hitlog_capture_matches_uo_on_mp_carentan`,
  `TURRET_REPORT=1`): every wire row of the capture matches. Ours enters
  the same bone (`bip01 l clavicle`, `torso_upper`) at (-0.0029, -0.0035,
  -0.0001) off retail's on the wound and (+0.0367, +0.0413, +0.0011) on
  the kill. Each difference lies almost along the round, so the box face
  sits elsewhere and the ray does not. The directions agree to 6e-8.
  13.7 finds the cause and closes the row.
- VERIFIED, the locational bone trace in each build
  (`SV_LocationalTraceModel`, cod11-combat.md 3.1): 1.1d `cod_lnxded`
  0x80c4564 (18 float stores), 1.5b `cod_lnxded` 0x80d2000 and UO
  `coduo_lnxded` 0x80c7394 (29 each). 1.5b and UO move each point through
  helpers that store each difference and each product sum: into the bone's
  frame at 1.5b 0x80d1f56 and UO 0x80c72ea, the sphere centre out of it at
  UO 0x80c723e. Each slab distance `(p - plane) * sign` and each
  `d1 / (d1 - d2)` is stored too.
- vcod measurement: the `-spawn-uo` kill replayed through that slab form
  enters at x 1513.000610 with every store rounded to f32 and 1513.000637
  with none. `bonetrace.rs` gives 1513.000610. The stores move the entry
  by 3e-5, against the 0.003 to 0.05 the pose is off, so the trace's
  arithmetic cannot close the row. vcod has since ported it
  (cod11-combat.md 3.1, "The arithmetic on 1.5 and UO"). VERIFIED, vcod
  measurement (`TURRET_REPORT=1`, 2026-10-19): the `-spawnlog` wound at
  30250 went from (0, -0.000122, -0.0000019) off retail's to its bits;
  every other hit row reads as before, the knockback-frame kills 0.001 to
  0.009 off.
- vcod measurement: on the kill frame ours poses legs 157 starting that
  frame at weight 0 over 1-2 idle clips (the knockback's first frame).
  Moving our pose clock 1 to 40 ms either way moves the kill's entry
  further from retail's, never onto it. INFERRED: the difference is in
  the pose's inputs on 1.5's and UO's eased controllers or the clip blend,
  not in the clock. No script builtin reads a bone on 1.1d, 1.5b or UO
  (VERIFIED, `strings`: none of the three game modules names a `getTagOrigin`), and a linked
  marker reads the pose a frame late. A grid of traces in the damage
  callback reads the pose the round met (13.6).

### 13.6 The pose a round meets: hit grids

Three more runs of the recipe on 2026-10-17 set `probe_hitgrid 1`:
inside the damage callback, before the stock callback lands the hit,
`probe_turret.gsc` casts a 9 by 9 grid of `bulletTrace`s (hitCharacters
on) along the round, 1.5 units apart across it and centred on its point,
and logs each cell's distance to the body as `fine1` prints it. The
callback runs inside the round's delivery, so the grid reads the pose the
round met. The runs are `mp_carentan-dm-turret-hitgrid.txt` (1.1d),
`-hitgrid-15.txt` (1.5b) and `-hitgrid-uo.txt` (UO 1.51b), each with its
`-script.txt` and `-target.txt` (recipe in the `-script` headers, ports
32752). In each the wound lands on the target's idle and the kill on the
next frame, the first of the knockback (legs 157 on that frame's end
frame, at weight 0).

- vcod measurement, before the fix below (`TURRET_REPORT=1`): on 1.5b
  every cell of both grids sits within 0.08 of retail's along its ray, most
  within 0.002. On 1.1d the same, but for the cells on the head model's
  own `bip01 head` box, all 0.118 further on ours. On UO the wound's cells
  ran 0.25 to -0.25 across the grid, a turn of the torso of about 2.5
  degrees, and the kill's entry sat 0.63 nearer the gun.
- vcod measurement: moving ours' pose clock 50 ms later put every UO wound
  cell on retail's to 0.004; 50 ms earlier or later on 1.1d or 1.5b moved
  theirs off by up to 0.5. So UO's idle on the target runs a frame ahead
  of ours, and 1.1d's and 1.5b's do not. Starting only the target's first
  legs clip after his spawn a frame early (its clock at the spawn frame's
  `level.time` less a frame) puts the UO wound's cells within 0.004 and
  the kill's within 0.031; the `-hitfine-uo` capture's points within
  0.0001 and 0.004; and the `-spawn-uo` kill on retail's 1512.
- VERIFIED, `game.mp.uo.i386.so` `ClientSpawn` (0x54df4): after the
  `gclient_t` memset it runs `ClientEndFrame` (whose `G_UpdateClientInfo`,
  0x52c81, frees and rebuilds the DObj when the models change, then runs
  `BG_PlayerAnimation`), then `ClientThink_real` on the last cmd, then
  `BG_PlayerStateToEntityState`. 1.5b's (0x4c9a8) runs the same three.
  VERIFIED, the legs setter (UO 0x2ce5f): a still clip gets no
  `XAnimSetTime`; only a movement clip whose weight reads 0 does. INFERRED:
  the node's time runs from wherever the tree started it, so on UO the
  idle's clock starts with the DObj the spawn builds, a frame before the
  idle takes weight; why 1.5b, with the same `ClientSpawn`, does not is not
  read.
- vcod measurement: that shift does not hold elsewhere. In
  `vehicle_armor_ab` (UO, mp_cassino) both clients spawn the same way, on
  idle 697 at 19350, and with the shift two of its silhouette gates fail
  (one cell 0.28 off, one ray missing the gunner) where without it they
  pass. The shift holds; it failed there because ours posed the
  script trace at the frame's own time, which the shift alone leaves a
  frame off on those bodies (13.7). The fall and the early join are not
  the cause: the `-hitgrid-uo-ground` and `-hitgrid-uo-spawn2` reruns
  (13.7) show the same frame ahead.
- VERIFIED, the 1.1d head cells: `xmodelalias/head_axis.gsc` names twenty
  heads, `basehead_axis_a_01..10` on `xmodelparts/basehead_axis11` and
  `basehead_axis_b_01..10` on `basehead_axis21`, and the character script
  attaches a random one. Ours drew a `b` head where retail's grid reads
  `basehead_axis11`'s boxes: with an `a` head every head cell matches to
  0.002. `probe_turret.gsc` now logs the target's attachments at each
  placement (`PROBE models`), and `probe_head <model>` swaps his head
  first, which the 1.1d gate sets.
- vcod measurement: on 1.1d (with the head pinned) and 1.5b every grid
  cell sits within 0.08 of retail's and every hit point within 0.002, and
  so does UO's with the shift above in place. The kill frame's cells read
  0.001 to 0.08 off where the wound's read under 0.004 on every server.
- vcod measurement, 2026-10-18 (`TURRET_REPORT=1`, each grid laid out
  9 by 9): on the kill frame the cells drift smoothly across the grid,
  about 3e-4 of a unit per 1.5-unit cell (a turn of about 2e-4 radians),
  -0.002..0.006 on 1.1d, -0.003..0.004 on 1.5b, -0.009..0.003 on UO; the
  0.01 to 0.08 cells are rays that graze a box edge, where that turn
  moves the entry most. INFERRED: the body is turned about a hundredth of
  a degree off retail's on the knockback's first frame, the frame its legs
  take clip 157 at weight 0, the same on all three; which input turns it
  is not found.
- vcod measurement, 2026-10-21 (`TURRET_REPORT=1`): with the legs and
  torso ramps starting a node at 0 from `0.001 * goal` (13.9, "The
  weights across frames") the kill lands on retail's bits on
  `-hitgrid-15`, `-hitgrid-uo-spawn2` and `-hitgrid-uo-ground`, and sits
  2e-6 off on `-hitfine-uo` (30200), 1.7e-4 off on `-hitgrid-uo` and on
  1.1d's `-hitgrid` (30450). Every grid cell is within 0.0011 of retail's
  on 1.1d and within 7e-5 on 1.5b and UO. INFERRED: the turn was clip 157
  at weight 0 where retail's setter had raised it to 0.001.
- vcod: `turret_ab`'s `hit_gate` replays each capture as `spawn_gate` does
  and holds the target's playerstate, each hit's point (to the bit, 13.8)
  and each grid cell (`GRID_NEAR`, 0.005) to retail's; the gunner's own rows are the
  `-spawn` gates'.

### 13.7 The frame ahead: UO's spawn DObj and the script trace

What 13.6 left open, read 2026-10-17. Addresses are module VAs as
`annotate_func.py` prints them.

- VERIFIED, `G_RunFrame`'s calls: 1.1d calls `Scr_RunCurrentThreads` at
  0x50653, `G_DObjUpdateServerTime` at 0x506ed, after the second thread
  pass at 0x506d0. 1.5b has one thread pass, at 0x50821, then
  `G_XAnimUpdate` at 0x50840. UO's are at 0x59e4d and 0x59e6c.
  INFERRED: on 1.5b and UO every script thread, a `bulletTrace` in a
  damage callback included, reads the players' DObjs at the last frame's
  anim time. On 1.1d the second pass reads them at the frame's own.
- VERIFIED, UO `G_DObjUpdate` (0x77df9): for an entity with a client
  (`cmp [ent+0x160],0` at 0x77e12) it calls `G_UpdateClientInfo` (0x77e21),
  which rebuilds the DObj when the models changed (`G_SafeDObjFree`) and
  runs `BG_UpdatePlayerDObj` and `BG_PlayerAnimation`. 1.5b's
  `G_DObjUpdate` (0x6b71c) does nothing for a client. INFERRED: the stock
  `spawnPlayer`'s `setModel` and `attach` calls (`_teams::model`, after
  the `spawn`) rebuild a UO player's DObj and set its clips inside the
  thread. `G_XAnimUpdate` then advances them a frame before the end frame
  that 1.1d and 1.5b start them on. On 1.5b the end frame's
  `G_UpdateClientInfo` sees the new models, frees the DObj and starts
  fresh.
- vcod measurement: together the two account for 13.6's frame ahead and
  for the seat-change switch in `coduo-vehicles.md` 13 ("Back from the
  gunner's seat"). A UO spawn's clips run a frame ahead, and a script
  trace poses a frame behind, so the two cancel for clips the spawn set.
  Before a seat change, every pose a script trace reads comes from the
  spawn's clips. After one, the seat's clips start on an end frame and the
  trace reads them a frame back. That is why `vehicle_armor_ab`'s bodies
  matched with neither rule and broke with only the first (13.6).
- vcod: a UO spawn sets `ClientSim::dobj_ahead`, and the next end frame
  starts its clips a frame early (`commit_pose`). On 1.5 and UO,
  `bulletTrace` poses bodies at `GameHost::dobj_time_ms`, which the thread
  pass leaves at the last frame. 1.1d's do too: its first pass runs
  before `G_DObjUpdateServerTime` (0x506ed) and poses the last frame, its
  second the frame's own. VERIFIED, vcod measurement (`leanhit_ab`,
  cod11-combat.md 3.6): with the net probe's framed cmds both 1.1d
  captures match posed at the last frame and miss 15 to 22 cells posed
  at the frame's own, so there is no 1.1d exception to explain. With both, every UO cell of the
  `-hitgrid-uo` grids, the `-hitfine-uo` points and the `-spawn-uo` kill
  match retail's (`GAPS_SPAWN_UO`, `GAPS_HITFINE_UO`, `GAPS_HITGRID_UO`
  removed), and so do `vehicle_armor_ab`'s phase-3 silhouettes at a
  quarter unit (`SILHOUETTE_GAPS` removed).
- Two more runs of the recipe on 2026-10-17 test 13.6's candidates. Both
  set `probe_spawnlog 1`, which logs the level time of the stock
  `spawned` notify (`PROBE spawned`) and the spawn point at the
  placement (`PROBE spawnat`). One is a plain rerun,
  `mp_carentan-dm-turret-hitgrid-uo-spawn2.txt`. The other,
  `-hitgrid-uo-ground.txt`, also sets `probe_ground 1`, which places the
  target 0.125 over the ground under the spot instead of 40 units above it.
  Each comes with its `-script.txt` and `-target.txt` (ports 32791-32792).
  VERIFIED, `-script.txt`: on both runs the spawn and the placement share
  a frame (11200), at spawn point (1872, 928) facing -180. VERIFIED,
  `-target.txt`: the grounded target drops 1.1 units, with no land event.
- vcod measurement: with the spawn point pinned, every grid cell and hit
  point of both runs matches retail's within the gates' 0.1 and 0.02. So
  the fall is not the cause, and the frame ahead holds with or without
  it. An earlier pair of these runs, whose spawn point was not logged,
  had the target spawn on a slope: retail's think there left a y velocity
  of 1 that drifted him 0.33 units by the landing, and ours, spawned
  elsewhere, did not. That is why the gates pin the spawn
  (`Capture::target_spawn`). The rig's joins answer the team menu a frame
  before the placement, where retail's probe spawned in the placement's
  frame; the pose does not depend on it.

### 13.8 The muzzle and the round's end on 1.5 and UO

What was left of `turret_ab`'s `HIT_NEAR` once the prone strike put the
end point on retail's bits (cod11-combat.md 3.1, "The prone strike on
mp_carentan"): on every capture the wound landed on retail's point or a
float step off it, and each kill on the knockback's first frame 0.0005 to
0.015 off (13.6). Read 2026-10-20 under gdb on retail UO 1.51b, three runs
of the `-hitfine-uo` recipe (13.5) on port 34231, each run's capture
replayed on ours. Scripts and dumps are in the gitignored `tmp/` of the
branch's worktree. Breakpoints: the locational trace (`coduo_lnxded`
0x80c7394, the world start and end of its caller's clip), after it
(0x809aa78, the fraction) and after the end point (0x809ab53); in the game
module, after the fire's muzzle stores (0x5aa90).

- VERIFIED, `game.mp.uo.i386.so` `BG_Bullet_Endpos` (0x3011c): the range is
  10000.0 (0x461c4000, stored at 0x3012e), not 1.1d's and 1.5b's 8192
  (1.5b `Bullet_Endpos` 0x6de56 stores 0x46000000). Each end component is
  `muzzle + forward * 10000` with the product on the x87 stack (0x30189),
  then the spread terms. Its callers are `Bullet_Fire` (0x7a8d5) and
  `Weapon_ArtilleryStrike_Fire` (0x7b77e). `Bullet_Fire` is called by the
  turret's fire (0x5ab94), `FireWeapon` (0x7be48) and three sites among
  the `CMD_VEH_*` turret commands (0x8f44c, 0x8fb8d, 0x900c3).
- VERIFIED, gdb: the clip's end is that sum to the bit. On the first run
  ours traced from the same muzzle to 8192, and its hit point came out a
  float step off in y. Retail's start, end and fraction give retail's
  point in the one-rounding form (cod11-combat.md 3.1).
- VERIFIED, the fire's aim on 1.5b (0x51193) and UO (0x5a99a):
  `AngleVectors` reads the turret entity's 0x68 plus 0x144 (UO 0x148),
  each sum stored, roll 0. 1.1d reads the gunner's `ps.viewangles`
  (6.3). VERIFIED, gdb: on UO 0x68 is `angles2` and 0x148 the gun's own
  angles ((9.997559, 30.002686, 0) and (0, 229, 0) on one shot). The
  forward it gives is the round's direction and the muzzle's. vcod's
  forward from the gunner's view read 4 float steps off in x.
- VERIFIED, gdb: `tag_player`'s world origin matches ours to a step in z,
  but `|tag_flash - tag_player|` reads 53.80326 and 53.80867 on two
  aims, where ours is the bind pose's 53.80957. So ours' muzzle sits
  about 0.006 further along the aim. VERIFIED, `xmodel/mg42_bipod`'s
  bones: `tag_player` hangs off `tag_aim` (under `tag_dummy`), and
  `tag_flash` off `tag_aim_animated`, `mg01`, `bi_base` and `tag_pivot`.
  The two chains pivot on different points, so the reach changes with
  the aim. VERIFIED, `turret_controller` (1.1d `game.mp.i386.so` 0x63348,
  UO 0x6c5e9): `G_DObjSetControlTagAngles` on `tag_aim` and
  `tag_aim_animated` with (`angles2[0]`, `angles2[1]`, 0), then on
  `tag_flash` with (`angles2[2]`, 0, 0). INFERRED: the server's DObj plays
  no anim on the gun, so both chains sit at their bind and a control
  replaces the bone's local turn in model space (cod11-combat.md 16.3);
  `tag_aim_animated`'s bind world turn is 0.001 degrees of pitch, so
  `tag_flash` lands on `tag_aim_animated + R(flash - tag_aim_animated)` and
  `tag_player` on `tag_aim + R(player - tag_aim)`, one barrel turn `R`
  about two pivots 0.04 apart. VERIFIED (vcod, `mg42_bipod` from the paks):
  that pose gives 53.803272 at the gdb aim (9.997559, 30.002686) and the
  bind's 53.80957 at (0, 0), so retail's 53.80326 is that aim's reach to
  1.2e-5 (`turret::muzzle`, unit test
  `the_muzzle_reach_moves_with_the_barrel`).
- vcod measurement (`turret_ab`, `TURRET_REPORT=1`): with the range and
  the aim, every first-frame wound of the 1.5b and UO captures but two
  landed on retail's bits. The two (`-hitfine-uo` 30150 and
  `-hitgrid-uo-ground` 30350) and 1.1d's `-hitgrid` 30400 were a float
  step off, along the round, from the bind reach. With `tag_flash` posed
  about `tag_aim_animated` (above) all three land on retail's bits, and
  every other first-frame wound of the 1.1d, 1.5b and UO captures stays
  on them. Each knockback kill stays as 13.6 leaves it (0.0005 to 0.013).

vcod: `combat::bullet_range` (`UO_BULLET_RANGE` on UO), `combat::bullet_end`,
`turret::AimRule::fire_angles`, `turret::muzzle`. `turret_ab`'s hit rows
must be retail's to the bit. Each gate names the rows it tolerates
(`KNOCKBACK_GAP`), and those stay within `HIT_NEAR` (0.001).

### 13.9 The gunner's own pose: the sync group's time and the blend out

The fire and aim groups are sync nodes (`standMG42_fire` `loopsync`,
`standMG42_aim` `nonloopsync`, section 10). VERIFIED, 1.5b `cod_lnxded`
0x80c84f0 and UO `coduo_lnxded` 0x80bd884, the arm for a node whose flags
carry a sync bit: the node steps its own time by `dtime` times the
weighted mean 0x80c639e (UO 0x80bb732) returns times its rate, wraps a
`loopsync` node by 1 and stops a `nonloopsync` one at 1, then copies the
time into every descendant (UO 0x80bd7e4). VERIFIED, 1.5b disassembly
0x80c86dc..0x80c86f8: the product `mean * rate * dtime` is stored as a
float (`fstps 0xc(%ebp)`) before the sum with the old time is stored.
VERIFIED, 0x80c639e: over the children in order, one with weight and a
non-zero mean of its own adds its weight to one stored float sum and
`mean * frequency * rate` to another; the quotient is stored, 0 with no
child counted; a leaf returns its frequency. VERIFIED, `pakuo00.pk3` and
1.1's paks (vcod's xanim reader): every `pb_*MG42gunner_fire_*` leaf has
five frames at 30 fps, frequency 6, looping; the aim leaves one frame,
frequency 0. So the fire group's time runs at 6 a second, a float step
off where the stored sums round (two leaves at 0.9995 and 0.0005 give
5.9999995).

VERIFIED, game.mp.uo 0x6ac06 (the placement): its first
`trap_XAnimClearTreeGoalWeightsStrict` takes the legs anim's node, then
it sets the goal weights of that node's rows and leaves at rate 1.0.
INFERRED: a group the legs leave (aim to fire as the trigger goes down,
fire to aim as it comes up) keeps the leaf weights its last placement
set while the anim setter fades it out, since nothing else touches them.

vcod: `turretpose::GunnerPlacement` carries the group (`node`), its
mean frequency (`frequency`, `turretpose::sync_mean` over the rows and
their leaves) and `looping`; the placement hands them to
`ClientSim::set_gunner_blend`, which puts the frequency on the legs
clock's current clip (`ChannelClock::set_sync_frequency`, stepped with
the stored increment), and keeps the group last left with its leaves
(`gunner_left`). `playerpose::pose_player` poses each legs layer: the
current one by the placement's rows and leaves, a turret group fading
out by the rows it was left with (`PoseInputs::turret_left`), any other
clip as itself; under `node_time` (1.5 and UO) every leaf samples its
group's node time. The client draws with the current group's rows only,
without the rows the search walks past (below).

#### The captures

VERIFIED live, 2026-10-20: `mp_carentan-dm-turret-gungrid-uo.txt` (UO
1.51b) and `-gungrid-15.txt` (1.5b), each with `-script.txt` and
`-target.txt`: the 13.6 hit-grid recipe with `probe_gungrid 1`, which
casts, on every frame the gunner stands off his placement spot, a 5 by 5
grid of `bulletTrace`s through his back along the gun's base forward
(rows 30..54 over his origin, 6 apart) and logs each cell's distance to
him (`probe_turret.gsc` gungrid). `turret_ab`'s
`the_gunner_grid_capture_matches_*` replay them on retail's clock and
hold every cell on a mounted frame within `GUNGRID_NEAR` (0.0005). A first
1.5b take had a 200 ms client stall at 27600 and a second the gunner off
the gun by 4.8 at the first snapshot; both were discarded (the captures
are the third).

vcod measurement, UO: before the change every cell of the fire phase
sat within 0.01 of retail's but the three frames after the aim-to-fire
change (2.6 to 8.8 off) and the three after fire-to-aim in the target
phase (0.2 to 2.2); the mount's eight frames were 9 to 24 off. With the
blend out on the group's own leaves the fire and target phases are
within 0.02 throughout, and the mount 1 to 10 (the idle's phase, below). Shifting the fire
leaves' node time by 0.02 (a third of a frame's step) puts fire-phase
cells 0.27 off and by 0.05 2.2 off, so the leaves' time is retail's
to well under a step. At the grid's resolution the node time and the
old seconds read alike: every fire leaf has frequency 6, so the mean
differs from 6 only in the last bit and `time * 5` frames tracks
`seconds * 30`.

#### The mount: the idle the blend fades out

VERIFIED, game.mp.uo 0x6b0e6..0x6b70c (the placement's second pass,
Ghidra's addresses, 0x10000 over the file's): each row and leaf goal
goes to `trap_XAnimSetGoalWeight` with a blend time of `1 / (1000 /
level.frametime * |weight - goal|)` (constants 1.0 at 0xb1578, 1000.0 at
0xb157c, `level+0x1f4`), or 0 when that rate is 0 (0.0 at 0xb1574), and
rate 1.0. VERIFIED, 1.1d `cod_lnxded` 0x080bd674 (1.5b 0x80cb20e, UO 0x80c05a2): the setter keeps
`|goal - weight| * time` as the node's remaining time, sets the weight
to the goal under 0.001, and raises a weight of 0 to `0.001 * goal`.
INFERRED: the remaining time is one frame, so every leaf (from
`0.001 * goal`) is on its goal at the next step, and the walk
normalizes the group to its own weight (cod11-combat.md 3.1), so the
leaves' first weights move no bone. What blends is the group's node
against the clip it replaces, over the setter's blend.

vcod measurement, UO: every cell of the eight mount frames sat 1 to 10
off, ours farther along the ray, in proportion to the weight left on
the idle (`pb_stand_alert`, frequency 0.1596): 1.6 on the frame before
the blend, 0.2 on its last. Shifting the idle's node time by +0.25
put the whole mount within 0.06. The cause was the rig, not the
blend: our gunner joined with the target (placed at 11450 where
retail's `PROBE place 1` reads 16550; the recipe starts him 5 s after
the target), and his random spawn on a slope sent the spawn think's
first anim to `pb_crouch_walk_forward_unarmed` for 350 ms, so his idle
started at 16900 with the toggle off (wire 185) where retail's first
snapshot reads 697. VERIFIED, both captures' first `!trace`: `legsAnim`
697 on UO and 634 on 1.5b, the idle with the toggle set, one anim set
since the spawn. `gungrid_gate` now joins the gunner when retail's did
(`rig_aligned`'s gunner place, `common::join_pair_staggered`) and pins
his spawn to the probe's spot on the ground (`GUNNER_SPAWN`; retail's
pick is not logged): our first snapshot reads 697, the idle's node time
0.5745 at 20100, and every mount frame but one cell is within 0.1 on
both servers (UO 20450 cell 14 0.28, 1.5b 19050 cell 18 0.10 and 19100
cell 14 0.28, the edge cells below).

#### The rows

VERIFIED, `coduo_lnxded` 0x80bdbdc (the tree walk, cod11-combat.md 3.1,
"The pose on 1.5 and UO"): a node with two or more weighted children
walks each into a buffer and a nested one scales its rotation sum to its
own weight (`weight / sqrt(|q|^2)`) before its parent sums it. The
turret group (`standMG42_aim`, `standMG42_fire`) is such a node over its
rows, and each row over its column pair. INFERRED: a row's two leaves
are normalized to the row's weight before the rows blend, which a flat
sum of the leaves at their products of weights does not do once the two
rows' column pairs sum to different lengths.

VERIFIED, game.mp.uo 0x6ac06 (1.5b game.mp 0x61389, the same calls): the
search's first pass sets, for every row it walks, the row's goal weight
to 1.0 and its column pair's to `1 - f` and `f` (the `f` leaf only when
`f` is not 0), each at goal time 1.0 (`trap_XAnimSetGoalWeight`, UO
syscall 100, `coduo_lnxded` 0x80c1072 into 0x80c05a2), before the
second `trap_XAnimClearTreeGoalWeightsStrict` and the second pass.
VERIFIED, 0x80c05a2: the goal goes to `+0x1c` of the node's half, the
remaining time `|goal - weight| * time` to `+0x18`; under 0.001
(0x80ea444, the float 0.001) the weight takes the goal and the time 0,
otherwise a weight of 0 is raised to `0.001 * goal` (`+0x20`).
VERIFIED, syscall 0x58 (0x80bfdca, each descendant through 0x80bfc22):
the strict clear zeroes goal, time and weight only of a node
0x80bfbdc reads inactive; a weighted one keeps its weight and gets goal
0 at the time given (0 here). INFERRED: a row the search walked past and
did not keep (every row before the pair it blends, every row but the
last when no row reaches the target) is raised from 0 to 0.001, its
leaves to `0.001 * (1 - f)` and `0.001 * f`, and the second pass touches
none of them, so they stand beside the blend. The rows walked first are
`15up` then `level` (engine child order, 7.2): a placement that blends
`level` and `15down` keeps `15up` at 0.001; one that breaks on `15up`
walks past nothing.

vcod: `GunnerPlacement::rows` (each row's weight and its leaves', engine
order), `passed` (the rows walked past, at the first pass's goals) and
`walked` (both sets' row nodes); `pose_player` hands the group to the
walk as a node over row nodes (`skeleton::Blend`,
`PoseBuffer::blend_tree`). On 1.5 and UO the rows posed are the carried
weights below.

vcod measurement (`TURRET_REPORT=1`): before either change every frame
on two rows sat 0.01 to 0.03 off, frames on `15up` alone exact. The
nested walk alone halved most of those; with the passed row at 0.001
every frame of the yaw rest, the target phase and the pitch rest is
retail's to under 0.0005 on both servers, and the right-arc cell (row
42, 12 right, 37.2635 on both servers) misses `bip01 r finger22` as
retail's does: ours had grazed the box's `z` face, at most 0.018 deep
along a ray nearly parallel to it, a pose 0.02 off moved it. The crouch
re-mount (UO up to 1.7 off before, 1.5b 0.27) sits within 0.021 and the
mount's frames within 0.05, every frame of both captures within 0.05
but one cell.

#### The weights across frames

VERIFIED, game.mp.uo (symbols): `G_RunFrame` (0x69c21) runs
`Scr_RunCurrentThreads`, then `G_XAnimUpdate` (0x69983), then the entity
pass and each client's `ClientEndFrame`, whose `turret_think_client`
reaches the placement (0x6bb49 into 0x6ac06). `G_XAnimUpdate` calls
`trap_DObjInitServerTime` (syscall 0x71) on every entity with a DObj,
then `G_DObjUpdateServerTime` (0x70), each with `0.001 *
level.frametime`.

VERIFIED, `coduo_lnxded`: syscall 0x71 is 0x808eb63 into 0x80bf11c and
the weight step 0x80bcc1e on the node record's second half (`+0x34` goal
time, `+0x38` goal, `+0x3c` weight; every caller of 0x80c0c82, which
picks the half, passes 1). The step copies the time to the old time, then
lands the weight on the goal and zeroes the goal time when the goal time
is under the step plus 0.001 (0x80ea1d4), else adds `(goal - weight) *
dt / goalTime`, raises a weight under 1e-6 (0x80ea1d8) to `0.001 *
goal`, and takes `dt` off the goal time. It recurses into the children
of a node with weight or goal; under a node with neither, 0x80bca32 lands
each descendant on its goal at once and frees those left at 0. VERIFIED,
1.5b `cod_lnxded` 0x80c788a and 1.1d 0x80bb130: the same step. VERIFIED,
0x80bef88 (`trap_XAnimGetWeight`, syscall 0x6c) reads the weight and
0x80be810 (`trap_XAnimCalcAbsDelta` 0x65 through 0x80bf696, which sets
0x84561a0) the goal; the tree walk 0x80bdbdc reads the weight. VERIFIED,
0x80bfc22 under the strict clear: a node counts as active when every
ancestor has weight (0x80bfa7c) and it has weight with a weighted leaf at
or below it (0x80bfafc). VERIFIED, 0x80c1072: a goal under 0.001
(0x80ea578) is taken as 0.

INFERRED, off those: the placement's search and the gunner's origin read
the goals, and a trace reads the weights. The second pass's goal time is
one frame, so the next step lands every kept node on its goal; the
strict clear sends every other node to 0 at once. Until that step, so
for every trace in the next frame's thread pass and cmds, a kept row or
leaf reads the goal the last placement but one gave it, a row or leaf
the placement added reads `0.001` times its first pass's goal, and one
it dropped keeps its weight. A group the legs leave keeps its rows at
their goals through every later step, and the DObj keeps them past a
dismount, so a re-mount on the same life finds them.

The legs and torso anims' own setter does the same (1.1d 0x080bd674,
1.5b 0x80cb20e, UO 0x80c05a2): a node at 0 starts its ramp at `0.001 *
goal`, its goal time the whole distance's (cod11-movers.md 15, "The
weight ramp"). INFERRED: a body traced before the next step reads a clip
just set at 0.001, not 0.

vcod: `turretpose::GunnerTree` holds a 1.5 or UO gunner's rows and
leaves as the anim info does: `GunnerTree::place` runs the strict clear,
the first pass, the strict clear and the second pass on
`GunnerPlacement::walked`, `GunnerTree::step` the weight step.
`ClientSim::set_gunner_blend` places and hands the pose the result at
once; `commit_pose` steps at each end frame, ahead of the placement. The
tree lives until the next spawn (INFERRED, off UO's `G_DObjUpdate`
rebuilding the DObj when the spawn sets the model, 13.7). 1.1d's gunner
keeps the placement's rows as before. `animclock`'s `Ramp::raised` starts
a node at 0 from `0.001 * goal`. `pose_player` matches the current and
the left turret group to legs layers by node: on a group's first frame
its layer reads weight 0 and drops out, and the layer before it (the
idle at the mount, the aim group at the first shot) poses as itself.

vcod measurement, 2026-10-21 (`TURRET_REPORT=1`, both gunner-grid
captures): before, 20 UO frames and 22 1.5b frames had a cell past 0.005,
up to 0.049 (UO 33700, the crouch re-mount) and 0.12 (1.5b 20200 cell
5); the mount's frames 0.01 to 0.04, each row or column change 0.007 to
0.036. The node match and the carried rows alone left the mount's frames
and the group changes at 0.008 to 0.039; the setter's 0.001 start on the
legs ramp put every cell but the re-mount's first frame (UO 0.0063)
within 0.005; keeping the rows past the dismount put every cell of both
captures within 0.0001 (UO 5.7e-5, 1.5b 8.4e-5). `turret_ab` holds the
gunner grids to `GUNGRID_NEAR` (0.0005) with no gap.

## 14. The client

How `cgame_mp_x86.dll` (1.1, image base 0x30000000) draws a turret, puts a
gunner's view on it and draws its reticle. Addresses are the module's VAs,
read from its decompilation and disassembly. `cent` is a `centity_t` (stride 0x228, array at 0x3020db80) whose
first 0xf0 bytes are its current `entityState_t`; the predicted playerstate
copy starts at 0x3020715c.

### 14.1 The draw

VERIFIED: the entity-type switch at 0x3001d5f0 sends case 0xb to 0x3001b2e0,
and its default prints `"Bad entity type: %i\n"`. INFERRED: 0x3001b2e0 is the
turret's draw, called once per frame per turret in the snapshot.

VERIFIED, 0x3001b2e0: a `test ah,0x1` on `es.eFlags` (0x3001b2ff), a load of
`es.index` (`+0x8c`, 0x3001b309) as an index into the table at 0x301d24fc
(0x3001b30f), a call to 0x3001d320 with `es.eType` and that entry, a store
of 0x80 into the ref entity's second dword, origin loads from `cent+0x1f8..+0x200`, an add of
32.0 (0x30069408) to the z of a second copy of the origin, and a call through
syscall 0x3d. INFERRED: `EF_NODRAW` (0x100) hides the gun; the model is the
entity's own `index` (`xmodel/mg42_bipod`, section 3), not the weapon file's,
which has no `worldModel` (section 2); 0x3d adds a ref entity at the
interpolated origin with `RF_LIGHTING_ORIGIN` 32 units up.

VERIFIED: 0x3001d320 calls 0x3001b080 only when its type argument is 0xb,
and 0x3001b080 creates a three-slot anim tree (syscall 0x81) and fills slot
1 from weapon def `+0x1c` and slot 2 from `+0x24` of
`weapons[es.weapon]` (`cent+200`). VERIFIED, the cgame's weapon field
table: `idleAnim` is `+0x1c` (record 0x30075594) and `fireAnim` `+0x24`
(record 0x300755ac), both strings. INFERRED: a turret's model plays the
`idleAnim` and `fireAnim` its weapon file names
(`standMG42gun_aim_foward` and `standMG42gun_fire_foward` on the stock gun).

VERIFIED, `xmodel/mg42_bipod`: the two gun surfaces (`metal@mg42`,
`viewmodel@mg42view`) skin only to bone 8, `tag_aim_animated`; the bipod
skins to `bi_base`, `bi_l` and `bi_r`. VERIFIED, the hierarchy: `tag_aim`
hangs off `tag_dummy` with `tag_player`, `tag_weapon` and `tag_butt` under
it; `tag_aim_animated` hangs off `mg01`, under `bi_base`, under `tag_pivot`,
with `tag_flash` under it. VERIFIED: `standMG42gun_aim_foward` is one frame
whose `tag_pivot`, `bi_base` and `mg01` rotations equal the bind locals, plus
a `tag_flash` translation; `standMG42gun_fire_foward` is seven frames at 30
fps, looping, keying the same bones.

### 14.2 The controller and the anim pick

VERIFIED: 0x3001d460 calls 0x3001b110 for an `eType` of 0xb (call at
0x3001d46f). VERIFIED, 0x3001b110: three calls to 0x3003c2a0, each on
`cent+0x68`/`+0x158`, `+0x6c`/`+0x15c` and `+0x70`/`+0x160` with the float at
0x30207140; tag lookups (syscall 0xae) on `tag_aim` (0x30064db8),
`tag_aim_animated` (0x30064da4) and `tag_flash` (0x30062088), each followed
by a call to 0x3001c0a0; the first two get the vector (first result,
second result, 0), the third (third result, 0, 0). VERIFIED: 0x3003c2a0
compares `b - a` against 180.0 and -180.0 (0x30069370, 0x30069378), adds or
subtracts 360.0 (0x30069374) and returns `(b - a) * f + a`. VERIFIED:
0x3001c0a0 takes the cosine and sine of each angle times 0x3006946c, which
reads pi/360, calls 0x3003bc80 twice, and stores the vector `vec3_origin`
(0x300608e8) and a 0 beside the result in a 0x20-byte slot per bone.

INFERRED: 0x3003c2a0 is `LerpAngle`; `cent+0x68..0x70` is the current
state's `angles2` and `+0x158..0x160` the next state's, lerped by
`cg.frameInterpolation`; 0x3001c0a0 builds a quaternion from half angles
and sets it as the bone's control rotation with no translation. So the
barrel's pitch and yaw turn `tag_aim` and `tag_aim_animated`, and
`angles2[2]` (always 0 in stock play, section 9) pitches `tag_flash`. The
order of the two quaternion products is not read.

VERIFIED, 0x3002fc80, per entity of the incoming snapshot: a copy of 0x3c
dwords into `cent+0xf0`, then a test of `cent+0x1e0` against 0 and of
`(new.eFlags ^ cent.eFlags) & 8`, either of which calls 0x3002f840; and
0x3002f840 opens with a copy of 0x3c dwords from `cent+0xf0` to `cent+0`.
INFERRED: an entity that is new or whose teleport bit flipped takes its next
state as its current one, so the controller lerps one `angles2` against
itself and the barrel snaps. The flip on a turret's first mounted frame
(6.2) is what keeps a gunner's view from sweeping down from the rest pitch
(-72 on carentan's gun, section 1) across the mount.

VERIFIED, the anim pick at 0x3001b25d..0x3001b295: a `test ch,0xc0` on the
dword at 0x302071dc, a compare of 0x302074d4 against `es.number`, a test of
0x30207158, `mov cx,1` when all three pass, and otherwise `es.eFlags & 0x400`
`setne` plus one; then syscall 0x8a with that slot, 1.0, 0.1 (0x3dcccccd) and
1.0. VERIFIED: 0x302071dc and 0x302074d4 sit 0x80 and 0x378 into the
playerstate copy at 0x3020715c, `ps.eFlags` and `ps.viewlocked_entNum` by
the netfield offsets. VERIFIED: 0x30033d3d and 0x30033d49 store 0 and 1 into
0x30207158 after a cvar test and a `pm_type` compare against 6. INFERRED:
0x30207158 is `cg.renderingThirdPerson`, and the gun the view rides in first
person always plays `idleAnim`; every other gun plays `fireAnim` while the
server's 0x400 is set (the frame it fired, section 6.3), and the switch
blends over 0.1 s.

### 14.3 The gunner's view

VERIFIED: 0x300333b0 copies the playerstate copy's `+0x14..0x1c`
(`ps.origin`) into 0x30209594..0x3020959c and its `+0xc0..0xc8`
(`ps.viewangles`) into 0x302095cc..0x302095d4, and has calls to 0x30033220
(0x300335c9), 0x30032540 and 0x30032ae0 at higher addresses, the last two
on the two sides of a test of 0x30207158. INFERRED: 0x300333b0 is
`CG_CalcViewValues`, 0x30209594 `refdef.vieworg` and 0x302095cc
`cg.refdefViewAngles`, and the turret step runs after the playerstate copy
and before the first- or third-person offset.

VERIFIED, 0x30033220: a test of `ps.eFlags` against 0xc000, a compare of
`ps.viewlocked_entNum` against 0x3ff and a test of 0x302074d0
(`ps.viewlocked`, offset 0x374); `ecx = 0x302095cc`, `ebx` = the gun's
`cent+0x30` (`apos`) and a call to 0x30005470 with the dword at 0x30207148
(0x3003325e..0x30033269);
a call to 0x3001c2c0 for `tag_player` (0x300622a0) and, on a zero return, a
call to 0x30020830 with 1 and `"Turret has no bone: tag_player"`
(0x3006227c); the two `LerpAngle`s of `angles2[0]` and `angles2[1]` added
into `refdefViewAngles[0]` and `[1]`; a test of 0x30207158 and a compare of
`ps.viewlocked` against 2, then two calls to 0x3004b189, each result times
2^-15 (0x300693b4), doubled, minus 1.0, added to pitch and to yaw; and
`vieworg` = the tag's x and y and its z minus the float at 0x3020722c
(`ps.viewHeightCurrent`, offset 0xd0).

INFERRED: 0x30005470 is `BG_EvaluateTrajectory`, so the view angles are the
gun's own base angles plus its interpolated barrel, not the player's;
0x3004b189 is the C runtime's `rand`, so a first-person frame whose
snapshot fired shakes each axis by up to a degree either way, once per
rendered frame; a gun model without `tag_player` is a fatal error.

VERIFIED, 0x30032ae0: `vieworg.z += ps.viewHeightCurrent` (0x3020722c), then
a test of `ps.eFlags & 0xc000` and a return before the bob. INFERRED: the
height 0x30033220 took off comes back here, so the eye is `tag_player`
itself, and a gunner's view has no bob.

VERIFIED: 0x30032ae0 calls the damage kick (0x30012cb0) and the sight's
bob (0x30012dc0) and adds their sum to `refdefViewAngles` (decompile), and
neither function tests `eFlags` (disassembly 0x30012cb0..0x30012e9f).
INFERRED: that runs ahead of the mounted return, so a hit gunner's view
kicks on top of the gun's, as an unmounted player's does. 1.5 and UO differ (`coduo-diff.md` 19, "The turret view": 1.5 drops
only the bob on a mount, UO both). vcod: `turret::TurretEye::kicked`.

1.5's turret view (`cgame_mp_x86.dll` 1.5, 0x300348c0) is not 1.1's.
VERIFIED disassembly 0x300348c0..0x30034ba4: it returns unless
`ps.eFlags & 0xc000` (0x3020af94), `viewlocked_entNum` (0x3020b28c) is not
0x3ff and `viewlocked` (0x3020b288) is not 0; `cent` is 0x30211940 +
number * 0x228, `currentState` at +0 and `nextState` at +0xf0; a failed
`tag_player` (0x30064480) lookup through 0x3001d540 is the fatal
0x3006445c. Four `LerpAngle`s (0x3003d9b0) at `cg.frameInterpolation`
(0x3020aef8) of es +0xd4 (`leanf`), +0x5c, +0x60, +0x64 (`origin2`); then
per pitch and yaw `d = AngleSubtract(refdefViewAngles[i] (0x3020d380),
lerpAngles[i] (cent+0x204))` (0x3003da20): past `leanf`/`origin2[0]` the
refdef angle becomes `lerpAngles` plus that, under `origin2[1]`/`origin2[2]`
`lerpAngles` plus that, else it stays (0x300349b0..0x30034a09). With
0x3020af10 0 and `viewlocked` 2, pitch gains `(rand() / 32768.0 * 2 -
1.0) * def+0x420` and then yaw the same of `def+0x41c`, the def the gun's
`es.weapon` (cent+0xc8) indexes (0x30034a0b..0x30034aa9; 32768.0 at
0x3006b5a8, 1.0 at 0x3006b588, `rand` 0x3004d4c9). The cgame's weapon
field table names +0x420 `vertViewJitter` and +0x41c `horizViewJitter`
(records at 0x30078110 and 0x30078104, type 6). Then `AngleSubtract(refdef
- delta_angles * 360/65536 (0x3006b5f4), [0x302108f8 + i])` per axis goes
to trap 0xf2 (0x30034aaf..0x30034b69), and `refdef.vieworg` (0x3020d348)
takes the tag's origin, its z less `viewHeightCurrent` (0x3020afe4).
VERIFIED: 0x302108f8 and 0x302108fc are summed into the angles trap 0x56
hands the engine (0x3003571a..0x3003572b). VERIFIED: the stock
`mg42_bipod_stand_mp`, `_duck_mp` and `_prone_mp` (1.5 paks) set both
jitters to 0.15. INFERRED: UO's view (`coduo-diff.md` 19, the arcs and
the clamp) without the eye pull-back and the sway, with 1.1's `viewlocked`
2 for a frame that fired; 0x3020af10 is `cg.renderingThirdPerson`; trap
0xf2 sets the client's view angles, so the cmd sends the held view, and
on a frame that fired the jitter goes into it too: a 1.5 gunner's own
view walks by up to 0.15 degrees a frame while the gun fires.

VERIFIED decompile, 1.5 0x3001c2b0 (the turret controller, `tag_aim`
0x30066fd4): a gun ridden in first person (`eFlags & 0xc000`,
`viewlocked_entNum` its number, 0x3020af10 0) turns `tag_aim` and
`tag_aim_animated` by the same clamp of the refdef against `lerpAngles`,
any other gun by its lerped `angles2`; no `viewlocked` test. The ridden
gun's anim is slot 1, any other's 2 when `eFlags & 0x400`.

VERIFIED decompile, `game.mp.i386.so` 1.5 0x620d3 (the manned aim,
called from 0x62305 each frame the gunner holds it outside a match
timeout): `angles2` is the view less the gun's angles clamped to the
record's ranges, with no 15-degree step and no `SetClientViewAngle`
(1.1's 0x5201c has both); and `es.leanf` = `rec+0x14` (bottom arc),
`origin2[0]` = `rec+0x18` (left), `origin2[1]` = `rec+0xc` (-top),
`origin2[2]` = `rec+0x10` (-right), the slots `G_SpawnTurret` (0x632be)
fills from `bottomarc`, `leftarc`, `toparc` and `rightarc`, the last two
times a float at 0x8b090 that the file leaves to a relocation. INFERRED:
that float is -1, as 1.1's arithmetic (section 2) and UO's capture
(`coduo-diff.md` 19, -40 and -45) read; nothing writes those four before
a gunner's first frame or clears them after, so the client reads its arcs
off the gun's own state.

VERIFIED live, `*-15-turret.bin` (1.5b, mp_carentan's MG42 298 at
yaw 229, weapon `mg42_bipod_stand_mp`): the gun's `leanf` and `origin2`
read 0 until the first gunner and 40, 45, -40, -45 from then on,
released or not; the followed gunner's playerstate holds pitch 60 on its
first mounted frames while `angles2[0]` reads 40, so the 1.5 server does
not snap the view to the barrel; 64 of the 120 mounted snapshots carry
`viewlocked` 2, each with the gun's `eFlags` 0x400. The recipe, on
`COD_VERSION=1.5` with `COD_DIR` at a stock 1.5 `main/`:

    COD_VERSION=1.5 PORT=31821 PROBE_SECS=100 tools/run_probe.sh client-probes/probe_turret mp_carentan \
        +set probe_teleport 1 +set probe_pose 1
    vcod --game-dir <1.5 install> --net-probe 127.0.0.1:31821 \
        --probe-team spectator --probe-follow --save-snapshots --capture-tag 15-turret --probe-secs 65
    vcod ... --probe-team axis --probe-secs 60                   # 2 s later, the target
    vcod ... --save-turret --probe-turret-target-ms 9000 --capture-tag 15-turret --probe-secs 55

the gunner's own `--save-turret` file discarded. `play::replay`'s
`a_followed_1_5_gunner_sees_down_the_gun` replays it through
`turret::v15_eye`.

vcod: `turret::v15_eye` on protocol 6, the gun turned by the same clamp in
the entity pass (`entities.rs`, as UO's), the eye at `tag_player`, the
damage kick on top (`TurretEye::kicks`), the jitter on `viewlocked` 2 off
the gun's `vertViewJitter`/`horizViewJitter`, and the held or jittered
view handed to the cmd (`TurretEye::snap`). vcod-server sends the arcs on
protocol 6 and 22 from a gun's first aim (`TurretRecord::aimed`).

UO's manned aim is the same clamp. VERIFIED decompile, `game.mp.uo.i386.so`
0x6b950 (called first from the fire tick 0x6bb49, which
`turret_think_client` 0x6be63 runs each frame the gunner holds the gun):
it stores the gun's number in `ps+0x5bc` (`viewlocked_entNum`),
`AnglesSubtract`s the gun's angles from `ps.viewangles` (`ps+0xe8`),
clamps pitch to `rec+0xc..+0x14` and yaw to `rec+0x10..+0x18` into
`angles2[0..1]`, zeroes `angles2[2]`, writes the four arcs into `leanf`
and `origin2` as 1.5 does, and flips `eFlags` 8 on a fresh mount (`rec+4`
0x800); no 15-degree step and no `SetClientViewAngle`. VERIFIED
decompile: nothing in the UO module writes `ps+0x5b8` (`viewlocked`)
other than 0 (`G_ClientStopUsingTurret` 0x6bcc8 and `StopFollowing`
0x5b2bf): `turret_use` (0x6c919) sets only `viewlocked_entNum`, and the fire
tick and its round (0x6bad3) never store a 2. 1.5's `turret_use` (0x62fcc) and
UO's both clamp the view into the arcs and `SetClientViewAngle` it on
the mount, as 1.1's does (VERIFIED decompile).

VERIFIED live, `fixtures/turret/mp_carentan-dm-turret-15.txt` and
`-uo.txt` (the 1.1 fixture's `--save-turret` recipe on 1.5b and UO 1.51b,
`probe_teleport 1`, no `probe_pose`): on every one of the 261 mounted
snapshots of each, `angles2` is the snapshot's view less the gun's (229
yaw) clamped to ±40 / ±45; on 140 of them the view sits past an arc, and
`delta_angles` holds still through the sweeps, the flick and the fire
(UO's moves 96 shorts of pitch on the crouched remount's
`SetClientViewAngle`). The flick lands the barrel at the edge in one
frame where 1.1 took three (12.3). 1.5's `viewlocked` reads 1 mounted
and 2 on the frames that fired; UO's reads 0 throughout, which is why the
probe's notes read `BROKEN mount: viewlocked 0 after the use tap` there.
Two shells and the server, as the 1.1 recipe, with `COD_VERSION` and
`COD_DIR` set (UO adds `--mod-dir uo` to both clients):

    COD_VERSION=1.5 PORT=31891 PROBE_SECS=200 tools/run_probe.sh client-probes/probe_turret mp_carentan \
        +set probe_teleport 1
    vcod --game-dir <install> --net-probe 127.0.0.1:31891 --probe-team axis --probe-secs 180
    vcod --game-dir <install> --net-probe 127.0.0.1:31891 --save-turret --capture-tag 15 --probe-secs 170

the gunner 3 s after the target. vcod-server: `turret::AimRule` per
protocol. `V1` keeps 0x5201c; `V15` and `Uo` clamp only and force no view,
`V15` skips the aim and the gunner placement in a match timeout
(0x62305, 14.8), and `Uo` writes no
`viewlocked` on the mount, the aim or the fire. `turret_ab`'s
`the_1_5_and_uo_barrels_follow_the_clamped_view` replays both fixtures'
views through `turret::aim` (no paks).

VERIFIED live, `*-turret-hit.bin` (1.1d, the gunner's own snapshots): a
mounted player hit and then killed. An allied `--save-hit` probe with
`m1carbine_mp`, which `probe_teleport 1` puts behind the gun like the
gunner, walks out to (1976 1848) and fires at the gunner's back: the round
at 23150 lands an `EV_PAIN` (parm 33) on the gunner's own ring with
`eFlags` 0xc018 and a `damageEvent` edge; the one at 23350 kills him, and
the same snapshot carries `EV_DEATH` and the release (`eFlags` 0x10, the
body back at the mount spot 1738 1860) with no second damage edge. A
shooter in front of the gun, where the probe puts an axis one, hits the
gun's box (`EV_BULLET_HIT_SMALL` at the barrel) and never the gunner.
The recipe, on 1.1:

    PORT=31894 PROBE_SECS=120 tools/run_probe.sh client-probes/probe_turret mp_carentan +set probe_teleport 1
    vcod --net-probe 127.0.0.1:31894 --save-turret --save-snapshots --capture-tag turret-hit --probe-secs 60
    vcod --net-probe 127.0.0.1:31894 --probe-team allies --probe-weapon m1carbine_mp --save-hit \
        --capture-tag turret-hit --probe-secs 50                  # 5 s later

keeping only the gunner's snapshot pair (its turret fixture and the
shooter's hit fixture discarded). `play::replay`'s
`a_mounted_gunners_hit_feeds_the_damage_once` replays it.

### 14.4 The reticle

VERIFIED: 0x30016760 (the weapon reticle) returns on 0x30207158, and on
`ps.eFlags & 0xc000` calls 0x30016610 unless `ps.viewlocked_entNum` is 0x3ff
(`cod11-hud-protocol.md` section 9).

VERIFIED, 0x30016610: tests of 0x301d902c against 0, 0x3029944c against 0
and 0x30207158 against 0; a load of the dword at 0x3020dc48 plus
`viewlocked_entNum * 0x228` (that is `cent+0xc8`, `es.weapon`) and a test of
it against 0; the weapon def's `+0xe8` first byte against 0; and a compare of
`cg_crosshairAlpha` (0x301db788) against 0.01 (0x300693f4); then syscall 0x48
with (1, 1, 1, `cg_crosshairAlpha`), and a call to 0x300310f0 with width and
height `reticleCenterSize` (`+0xf0`) times 0x301d1fc4 and 0x301d1fc8, x and
y `(0x30209584 - w) * 0.5 + 0x3020957c` and `(0x30209588 - h) * 0.5 +
0x30209580`, texture coordinates 0..1 and the handle at `0x301a6a9c +
weapon * 0x198`. VERIFIED: the cvar record holding 0x301d9020 (whose
`integer` is 0x301d902c) is `cg_drawCrosshair`, default `"1"`; the cgame
weapon field table puts `reticleCenter` at `+0xe8` (record 0x300757ec) and
`reticleCenterSize` at `+0xf0` (record 0x30075804, an int).

INFERRED: on a mounted gun the HUD draws the gun's `reticleCenter`
(`gfx/reticle/mg42_cross.tga` on the stock gun) and nothing else, white at
`cg_crosshairAlpha`, `reticleCenterSize` virtual units square (32 on the
stock gun) scaled by the screen, centred on the 3D view; the gun's weapon is
read off its entity, not off the playerstate, whose `weapon` is still the
carried one (section 4.4).

### 14.5 What the client drops on a mounted gun

- VERIFIED: 0x300371f0 guards the view weapon's draw with
  `ps.eFlags & 0xc000 == 0`. INFERRED: no view weapon while mounted; the gun
  the gunner sees is the turret entity.
- VERIFIED: 0x30004eb0 replaces `es.weapon` with 0 when `es.eFlags & 0xc000`
  before it attaches the weapon model to `tag_weapon_right` or
  `tag_weapon_left`. INFERRED: a gunner's carried weapon is not drawn.
- VERIFIED: `CG_FireWeapon` (0x30038b70) calls `CG_MuzzleFlash` (0x30036c90)
  for an `eType` of 0xb with `edx` 0 (0x30038bfe), the gun's `lerpOrigin`
  and the tag name from 0x3007486c (`tag_flash`); 0x30036c90 picks the
  handle at `0x301a69f8` for a zero `edx` and `0x301a69f4` otherwise.
  INFERRED: a turret's shot plays the weapon file's `worldFlashEffect` on the
  gun's own `tag_flash`, the gunner's included.
- VERIFIED: the player draw (0x30028210) calls 0x300279b0 when
  `es.eFlags & 0xc000` (0x300282bf). INFERRED: that is the client's copy of
  section 7's placement (7.3), which blends the mounted anim by the gun and
  moves the drawn body (14.7).

### 14.6 As implemented

`crates/client/src/turret.rs` holds the controller, the anim pick, the
`tag_player` eye and the shake; `entities::build_instances` draws each
`eType` 11 entity through them, and `hud::player::turret_reticle` draws the
reticle. UO's gate, view and drawn barrel differ (coduo-diff.md 19, "The
turret view"). Where vcod differs:

- The control rotation is composed onto the bone's local after the anim, in
  the bone's own frame. The order of 0x3001c0a0's two products is not read.
  VERIFIED (14.1): neither gun anim keys `tag_aim` or `tag_aim_animated`,
  and both bind locals are identity. INFERRED: with `angles2[2]` 0 (section
  9) `tag_flash`'s control is identity too, so either order poses the stock
  gun the same. `turret.rs`'s test checks the
  swing about `tag_aim` against the server's `tag_weapon_local` geometry.
- The anim switch cross-fades linearly over the 0.1 s, and `fireAnim`
  restarts each time it becomes the goal. How the engine's goal weights ramp
  and where a re-entered loop resumes are not read.
- No lighting origin: vcod's dynamic models are not lit per entity.
- The shake draws from msvcrt's `rand` sequence seeded 1. Retail's draws come
  from a stream every other cgame caller shares, so only the distribution
  carries over.
- The gunner tests (`eFlags`, `viewlocked`, `viewlocked_entNum`) read the
  newer snapshot of the interpolated pair rather than the predicted
  playerstate, and the HUD reads the gun's `weapon` off the newest snapshot
  rather than a persistent entity table.
- The reticle scales by vcod's aspect-preserving virtual screen, where
  retail stretches 640x480 by separate x and y scales.
- 0x300279b0 runs through `vcod_common::turretpose::place_gunner`, the
  server's port of 0x515a8, off `tag_weapon`'s bind position turned by the
  lerped barrel. The goal weights apply at once, which is retail's
  one-frame ramp (14.7). The leaf set is posed by sequential lerps, each leaf by its
  share of the running total; that is the exact weighted mean at full
  weight and close to it inside the 0.2 s anim-switch cross-fade.
- The client's trace down runs against the world and last frame's drawn
  player capsules (`play::predict::solid_bodies`), `MASK_PLAYERSOLID`,
  the gunner skipped; `cg_debuganim` 5 is not honoured. The drawn body is
  yaw-only like every vcod player, where retail's player draw takes `AnglesToAxis` of the
  whole result (14.7); a gun spawned with pitch or roll would tilt retail's
  gunner and not ours.

### 14.7 The client's placement (0x300279b0)

Read from the Ghidra export and the disassembly of `cgame_mp_x86.dll` (1.1)
over 0x300279b0..0x30028209. `cent` is the gunner's `centity_t`, whose
entity state starts at offset 0, `lerpOrigin` at `+0x1f8` and `lerpAngles` at
`+0x204` (both read by the player draw's refEntity setup at 0x30028210).

- VERIFIED: loads of `es+0x74` (0x300279bb) and `es+0x90` (0x300279d6),
  compares of the first against 0x40 and 0x3ff, and returns on both.
  VERIFIED, `fields_v1.rs`: offset 116 is `otherEntityNum`, 144 `clientNum`.
  INFERRED: the gun is the entity `otherEntityNum` names (section 10), and a
  number under 64 or of 1023 skips the placement.
- VERIFIED: returns on a zero word at `0x3018bc0c + ci * 0x448`, a zero legs
  anim word at `0x3018bf98 + ci * 0x448`, a null legs anim record at
  `0x3018bf9c + ci * 0x448` and a clear `+0x50 & 4` on that record; a return
  on a zero dword at `0x3020dd60 + gun * 0x228`; a return on a null DObj
  (trap 0xa2). INFERRED: the same `turretanim` gate as 0x515a8's (7.1), on
  the client's own clientinfo, plus a valid-entity test on the gun.
- VERIFIED: `0x3001c270` on the gun's `centity_t` with the warning `"WARNING:
  aborting player positioning on turret since 'tag_weapon' does not exist"`,
  `0x3003a3f0` (`vectosignedyaw`: 0 for a zero x and y, `atan2` scaled
  otherwise) on its result, and `0x3003c770` on the gun's angles
  (0x30027ad5). VERIFIED: `0x3003c770` calls `0x3003a790`, which stores
  forward as `(cp*cy, cp*sy, -sp)`, then writes `vec3_origin - right` into the
  second row. INFERRED: `0x3003c770` is `AnglesToAxis` and `0x3003a790`
  `AngleVectors`.
- VERIFIED: the column split loads the child count, multiplies by 0.5
  (0x30027c15, `0x3006930c` reads 0.5) and subtracts the tag's yaw divided by
  the weapon def's `+0x400` (0x30027c1f). INFERRED: the same `n / 2 - yaw /
  animHorRotateInc` as 7.2, and the same row search by height follows it:
  trap 0x9a (`XAnimCalcAbsDelta`) per row, `fVar5 <= delta.z` breaking, the
  `tag_aim` warning on the end-row arm, the `(target - z_prev) / (z - z_prev)`
  split otherwise.
- VERIFIED: every goal weight is passed with a time of `1 / ((1000.0 / g) *
  |w - goal|)`, or 0 when that product is not above 0, where `1000.0` is
  `0x30069478`, `w` is `0x300319a0`'s read of the node's current weight and
  `g` the int at `0x30207144`.
- VERIFIED, `CG_DrawActiveFrame`'s clock block: `0x300339c4` stores the new
  time into `0x30207148`, `0x300339e6` stores it minus `0x3020714c` into
  `0x30207144`, and on a negative difference `0x300339f0` stores 0 there and
  `0x300339f6` the new time into `0x3020714c`. INFERRED: `0x30207148` is
  `cg.time`, `0x3020714c` `cg.oldTime` and `g` `cg.frametime` in ms; the
  aim block's time step (`0x300373a3`, `g * 0.001`) reads it the same way.
- VERIFIED, `CoDMP.exe`: cgame trap `0x8c` (the wrapper `0x30031840` the
  placement calls) runs `0x004895c0`, which zeroes a time under 0.001
  (`0x00568e6c`) and passes it to `0x00488e80`. That stores the goal weight
  and, as the node's remaining blend time, `|goal - weight| * time`;
  under 0.001 it stores 0 and the weight is set to the goal outright.
  VERIFIED, `0x00485ba0`: with `dt` the global `0x00a9cc5c`, a node whose
  remaining time is at least `dt + 0.001` moves `(goal - weight) / remaining
  * dt` and loses `dt` of it; any other node takes the goal weight.
  VERIFIED: cgame trap `0x95` (`0x00487e10`) stores its argument into
  `0x00a9cc5c` before the update, and the cgame passes it `g * 0.001`
  (`0x3001bd4a`..`0x3001bd6d`).
  INFERRED: the remaining time the placement leaves is `|goal - w| / ((1000
  / g) * |goal - w|) = g / 1000`, one frame, so the next update with that
  frame's step reaches the goal (the step fails the `dt + 0.001` test);
  a frame of 0 ms sets it outright. The ramp is a one-frame snap, whatever
  the size of the change.
- VERIFIED, 0x300280b1..0x30028153: trap 0x9a on the whole anim, `0x3003bc10`
  (a 2D rotation of the translation by the tag's yaw), the tag's `+0x30` and
  `+0x34` added to x and y with the gunner's height above the gun as z,
  `0x3003bff0` (`RotationToYaw`) plus the tag's yaw into `0x3003c7c0`
  (`YawToAxis`: z row `(0, 0, 1)`), `0x3003ae70` (`MatrixMultiply43`) with
  the gun's axis, and `0x3003c810` with `cent+0x204` as its out pointer.
  INFERRED: `0x3003c810` is `AxisToAngles`, so the body's yaw (7.2) lands in
  `lerpAngles` on every call.
- VERIFIED: a compare of `0x3029848c` against 5 (0x30028158) skips the rest,
  and the cgame's cvar table entry whose `vmCvar_t` is 0x30298480 names
  `cg_debuganim` (default "0", flags 0x200). INFERRED: `0x3029848c` is that
  cvar's `integer` at `+0xc`. VERIFIED: stores of the placed origin into
  `cent+0x1f8..+0x200`, a `0x30029030` call with mask 0x2810011 (0x300281c7),
  start `(x, y, gun z)` and end the placed origin, the gunner's number as the
  skip, and a store of the result's `+0xc` into `cent+0x200` when the fraction
  is under 1.0 (0x300281f8). INFERRED: unless `cg_debuganim` is 5 the drawn
  origin moves to the placement and its z onto whatever the trace meets, as
  0x515a8's does.
- VERIFIED: the player draw calls `0x3003c770` on `cent+0x204` (0x300282cd,
  0x300282d7) after the 0x300279b0 call. INFERRED: the refEntity axis is the
  placement's angles, so retail turns a gunner's whole body to the yaw 7.2
  works out.

VERIFIED, vcod's port over the stock `mg42_bipod` and `standMG42_aim`
(`the_barrel_yaw_picks_the_columns_and_the_body_holds_still` in
`crates/common/src/turretpose.rs`): with the barrel at yaw 0, +45 and -40 the
blend picks `forward`/`15left`, `45right`/`30right` and `45left`, and the
body's yaw stays 0 to 7.5 degrees off the gun's base. VERIFIED, the leaves
posed on `playerbody_american_airborne`: `tag_weapon_right` at y 7.7 on
`45right_level`, -6.1 on `forward_level` and -14.4 on `45left_level`.
INFERRED: the leaves' root yaw cancels the barrel's, so the body stays
square to the gun and the column blend swings the arms after the barrel;
the server's view yaw, which follows the barrel, would turn the whole body
instead.

### 14.8 The 1.5 and UO replay

`turret_ab`'s `the_mount_sweep_fire_and_release_match_15b_on_mp_carentan`
and `..._uo_on_mp_carentan` replay `mp_carentan-dm-turret-15.txt` and
`-uo.txt` (14.3) through the whole server, as section 13.1 replays the
1.1 capture: the same probe gametype, the cmds and the diff, on a
protocol 6 server over a 1.5 `main/` and a protocol 22 one over UO's
`uo/` (`COD_DIR` at a UO install runs both). What the replay found:

- vcod: `defs::Defs::of` gave a protocol 6 server 1.1's tables, whose
  `proto` is protocol 1, so every reader of `host.defs.proto` (the aim
  rule, `TurretRecord::wire_arcs`, the `playSound` broadcast) ran 1.1's
  branch on 1.5: the barrel stepped 15 degrees a frame and the view was
  forced onto it, 160 `delta_angles` rows. `defs::V15` carries protocol 6.
- VERIFIED decompile, 1.5 `game.mp.i386.so` `PmoveSingle` 0x37eb7: the
  mounted arm (an `eFlags & 0xc000` test, `groundEntityNum = 0x3ff`)
  calls `PM_UpdateAimDownSightFlag`, `PM_UpdatePlayerWalkingFlag`, then
  0x332c4, 0x35aef and 0x34ae8. VERIFIED decompile, 0x332c4 (1.5's
  `PM_CheckDuck`): on `eFlags & 0xc000` it writes `pm_flags` 0x1/0x2 for
  the gun's stance, then runs the same tail as on foot: the target
  (`ps+0xcc`) steps while `ps+0xd4` is 0, 0x326a6 runs, and
  `PM_GetEffectiveStance`'s answer sets or clears `eFlags` 0x20/0x40.
  VERIFIED decompile, UO `game.mp.uo.i386.so`: the mounted arm of
  `PmoveSingle` 0x3d530 calls 0x37e16, whose body tests `eFlags & 0x4000`
  and ends on `PM_GetEffectiveStance` and the same `& 0xffffff9f` write.
  VERIFIED live, both captures: a player crouched (`eFlags` 0x30) who
  mounts the standing gun reads 0xc010 from the first mounted snapshot,
  and 0x18 on the release's. INFERRED: the eye's target steps to the
  gun's stance on the mount, so the crouch bit goes with it. vcod:
  `pmove`'s mounted arm runs `step_view_target` and the 1.5 eye under
  1.5 and UO rules; 1.1's 0x316f4 touches no eye (section 5).
- VERIFIED decompile, 1.5 0x62305: `G_IsInMatchTimeout` gates the aim
  (0x620d3), the placement (0x61389, the function holding "aborting
  player positioning on turret since 'tag_weapon' does not exist"), the
  `viewlocked` 1 write, the gun's `eFlags` 0x400 clear and the cooldown
  with the fire. vcod: `turret_think_client` skips the placement and
  keeps the last blend on 1.5 in a timeout (`turret_ab`'s
  `a_15_match_timeout_holds_the_gunner_where_he_stands`, which moves the
  gunner while the clock stands).
- Both joins land ours a frame after the spawn, so the spawn point's
  slide (-2, -11 units/s) rides into the placement's fall and ends 3.3
  units off the spot, where UO's retail capture also ended (1737.6
  1856.9) and 1.5's did not (1738.2 1860.2). INFERRED: join timing, not
  the gun; the rig starts ours on retail's spot.
- UO's 16-bit `delta_angles[0]` changes above its width on the crouched
  remount (the capture's 65632 is 96 with bit 16 set by the decoder,
  `msg::above_width`); the diff compares the low 16 bits.

Rows left, each in the gate's `GAPS_15` / `GAPS_UO`: the killing round's
impact (13.1's `KILLING_ROUND`; 13.3 measures the target's clock that
decides it). The kill snapshot's `EV_PAIN` row closed with 13.3's end-frame
copy. VERIFIED live: the target
phase's first round on 1.5, at 30000, lands at (1144 1098 9), on the
gun's ray at the view's 1.2 degrees down, where ours used to stop at
(1388 1413 17) on lump 24 patches 136 and 133. VERIFIED, retail
`bulletTrace` along that segment: 1.1d stops on patch 136 too, and 1.5b
and UO 1.51b pass through a crack along its facet seam, which 1.5b's
build of the patch code leaves and 1.1d's does not. vcod builds the
patches that way on a 1.5 and a UO server and the row is gone
(`bsp-ibsp59-format.md`, "Patch collision on 1.5 and UO").
