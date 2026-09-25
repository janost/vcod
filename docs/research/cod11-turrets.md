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
| `cl+0x374` / `+0x378` / `+0x380` | `ps.viewlocked` (8 bits) / `ps.viewlocked_entNum` (16 bits) / `ps.gunfx` (8 bits) | VERIFIED, player netfield offsets 884, 888 and 896 in `crates/common/src/net/fields_v1.rs` |
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
(`bgs + clientNum * 0x448`). VERIFIED, each by the instruction named:

- the gate: the word at `ci+0x9ba78` compared to 0 (0x515c7), the pointer at
  `ci+0x9ba7c` compared to null (0x515d4) and its `+0x50 & 4` (0x515e2);
- the anim handle: that word with bit 0x200 cleared (`and ah,0xfd`, 0x51671)
  under `Scr_GetAnimsIndex`'s result in the high half; the tree is the dword
  at `ci+0x9bb2c` (0x51638);
- `vectosignedyaw` on the first three floats of `tag_weapon`'s matrix
  (0x51682), and that matrix's floats at `+0x30`, `+0x34` and `+0x38`
  (0x51e9e, 0x51eaa, 0x51710);
- `AnglesToAxis(tur.r.currentAngles)` into a 4x3 whose last row is
  `tur.r.currentOrigin` (0x5169d..0x516c0); the dot of `ent.r.currentOrigin -
  tur.r.currentOrigin` with that axis's third row (0x516c5..0x51700), kept at
  `ebp-0x154`, and the same dot minus the tag's `+0x38`, kept at `ebp-0x140`;
- a loop over the anim's children, bounded by `trap_XAnimGetNumChildren`
  (0x5173e, 0x51a10). Per child: its goal weight 1.0 (0x51812); `x = 0.5 * n
  - yaw / weaponDef+0x400`, `n` the child's own child count
  (0x5184e..0x51878); compares against 0 and `n - 1` (0x5187e..0x518b8); a
  `fistp` under control word bits 0xc00, round toward zero (0x518c0..0x518eb);
  `f = x - trunc(x)` (0x518fd); grandchild `trunc(x)` given `1 - f`
  (0x5195b); a compare of `f` against 0 (0x5196b); grandchild `trunc(x) + 1`
  given `f` (0x519b6); `trap_XAnimCalcAbsDelta` on the child (0x519d4) with
  its translation at `ebp-0xc..-0x4`; a compare of the translation's z
  against `ebp-0x140` (0x519df) and a `je` out of the loop (0x519ea); stores
  of that z, `f` and `trunc(x)` (0x519ec..0x51a04);
- after the loop: `trap_XAnimClearTree` on the anim (0x51a33), the last
  child's two grandchildren given `1 - f` and `f` again (0x51aa8, 0x51b35),
  and compares of the loop index against 0 and the child count
  (0x51b3d..0x51b52); one arm looks up `tag_aim` (0x51b64) and gives the
  child 1.0 (0x51e57); the other computes `(ebp-0x140 - z_prev) / (z - z_prev)`
  (0x51bf0..0x51c14), gives it to the child (0x51c7a) and its complement to
  child `i - 1` (0x51d1a), and gives child `i - 1`'s stored grandchild pair
  the stored `1 - f` and `f` (0x51da8, 0x51e57). Every goal weight after the
  loop carries a blend time built from the child's current weight
  (`trap_XAnimGetWeight`), 1000.0 (0x758f4) and the int at `level+0x1f0`;
- `trap_XAnimCalcAbsDelta` on the whole anim (0x51e7b); `VectorAngleMultiply`
  of its translation by the tag's yaw (0x51e90); a local origin of that
  translation's x and y plus the tag's `+0x30` and `+0x34`, and `ebp-0x154`
  for z (0x51e9b..0x51ebf); `RotationToYaw` of the delta's rotation plus the
  tag's yaw into `YawToAxis` (0x51ec3..0x51edb); `MatrixMultiply43` of that
  local 4x3 with the turret's (0x51ef2); the product's last row stored to
  `ps.origin` (0x51efa..0x51f1b);
- `trap_Trace` with a zero box from (`ps.origin[0]`, `ps.origin[1]`,
  `tur.r.currentOrigin[2]`) to `ps.origin`, the player's number to skip and
  mask 0x2810011 (0x51f1e..0x51f8d); a compare of 1.0 against the fraction
  (0x51f97) and a store of the trace's `+0xc` to `ps.origin[2]` (0x51fb3);
- `BG_PlayerStateToEntityState` (0x51fc6), `ps.origin` into
  `ent.r.currentOrigin` (0x51fd7..0x51fec), `AxisToAngles` of the product's
  axis into `ent.r.currentAngles` (0x51ffb), `trap_LinkEntity` (0x5200a).

VERIFIED, the leaves (pak0, flag 0x2 root tracks, one key each): the 21
`pb_standMG42gunner_aim_*` root translations read z -39.57 on every `15down`
leaf, -47.46 on every `level` leaf and -55.41 on every `15up` leaf, x between
-11.92 and -17.21, y between 2.85 and 6.01; the root rotations are the
column's own yaw, `45left` +45 through `45right` -45, none on `forward`. The
`fire` leaves carry the same values, except the three `fire_30left` leaves,
which key a second translation at frame 5, (-15.99, 5.62) against (-15.33,
5.35) at frame 0.

### 7.2 What it does

INFERRED, each from 7.1's control flow:

- The placement runs only when the legs anim carries `turretanim`. That anim
  is taken as a two-level blend, its children the rows and theirs the
  columns, in the engine's child order, the reverse of the file's
  (`player-model-anim-system.md`, "Animation indices: the animtree"). For
  `standMG42_aim` that is rows `15up`, `level`, `15down`, root z increasing,
  and columns `45right` through `45left`.
- The column position is `n / 2 - yaw / animHorRotateInc`, clamped to the
  row and split between the two columns either side. With 7 columns and 15
  degrees the gun's yaw 0 lands halfway between `forward` and `15left`, not
  on `forward`: the 0.5 multiplies `n`, not `n - 1`.
- The barrel's pitch is not read. The row is found by height: the target is
  the player's height above the gun minus `tag_weapon`'s height in the gun's
  frame, which the barrel's pitch moves about `tag_aim`. The first row whose
  root z reaches the target is blended linearly with the row before it so the
  blend's z equals the target; a target below the first row or above the
  last takes that row alone.
- The origin is the blended root translation turned by `tag_weapon`'s yaw and
  added to its x and y, at the player's own height, carried into the world by
  the gun's axis and origin. The body's yaw is the blend's root yaw plus the
  tag's, in the gun's frame.
- The trace lifts the origin onto whatever lies between the gun's height and
  that spot; a clear trace leaves the player's own height.

VERIFIED, the replay: `every_captured_gunner_origin_replays`
(`crates/server/src/game/turret.rs`) runs vcod's port of 7.2 over the gun's
`angles2` and the gunner's origin of all 260 mounted snapshots on a turret
anim in the 12.2 capture, both tables and every snapshot between, through
mp_carentan's collision: max 0.099 units, mean 0.049, z -23.9 on every one.
The fixture prints one decimal, so that is its rounding. Walking the
children in file order instead misses by up to 3.0 (mean 1.8).

Not determined:

- Which time `XAnimCalcAbsDelta` samples a root with more than one key at.
  vcod reads frame 0. Only the three `fire_30left` leaves differ, by 0.7 at
  their last frame, and no captured firing snapshot blends them.
- The body's yaw. VERIFIED, 7.1: `BG_PlayerStateToEntityState` is called at a
  lower address than the `AxisToAngles` store. INFERRED: the entity state is
  built before the store, so the yaw never reaches the wire and no capture
  can check it. vcod blends the leaves' yaw rotations by weight.
- The blend times, and whether the goal weights pose anything besides this
  routine's own deltas (the gunner's server-side body for a locational hit,
  for one). vcod does not keep them.

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

The triggers, INFERRED from the call sites:

- the use key: `Cmd_Activate_f` sets the busy byte to 2 (section 4.1), and
  the next `turret_think_client`, the same server frame's (12.7), finds it
  not 1 (0x5235c) and releases;
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

INFERRED: the use cmd runs when it arrives, and the probe's cmds arrive about
18 ms behind their `serverTime` (the probe's `cmdLag`), so cmd 24736 ran after
frame 24750 and cmd 38282 just before frame 38300. On the stand mount more
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
`ClientEndFrame` pass. Body placement (section 7) runs between the aim and
the fire: `vcod_common::turretpose::place_gunner` from the gunner's legs
anim, then the trace down onto the map's collision, then the origin and the
body's yaw mirrored to script.

The release (section 8) is `game::turret::release` on the record and
`release_sim` on the gunner, reached from four places:

- `turret_think_client` releases instead of running the frame when the busy
  byte is not 1 or the gunner is dead or not playing. The use key's busy 2 is
  set in the cmd's own item pass, ahead of the touch pass's `pm_type` gate.
  INFERRED, section 4.1: `Cmd_Activate_f` has no `pm_type` gate of its own.
- the turret pass releases, after its rounds' damage callback, any gunner
  those rounds killed, so the victim's death snapshot is already unlocked.
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

- The placement's trace sees the world's brushes and terrain only, where
  retail's `trap_Trace` also clips entities. The yaw goes to script as the
  whole of `angles`, pitch and roll 0, where retail's `AxisToAngles` carries
  a tilted gun's pitch and roll too.
- INFERRED: the muzzle takes `tag_flash`'s distance from `tag_player` off the
  model's bind pose, where 0x51488 reads both tags off the animated model
  (section 6.3).
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
- A round stops at the first player it hits. The pass-through 12.5 shows (a
  second impact on the world behind the target) is not modelled, the same as
  for a carried rifle's round.
- The hit-location product is taken wider than a float before the
  truncation, as the x87 takes it: `60 * 0.9` reads 53 as in 12.6, where a float
  product rounds to 54 first.
- A gun with no `stopFireSound` keeps its loop on through the frame the
  timer runs out, as section 6.4's `je` past the clear reads.
