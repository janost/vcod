# CoD 1.1 MP: item pickup

How a player takes a weapon, ammo or health off an `ET_ITEM` entity: the
walk-over touch, the use key and the candidate it picks, the grab test, the
ammo and health arithmetic, the slot swap, what a pickup writes, how a drop is
launched and how long it lives, what the item carries on the wire, and what
script sees.

Evidence rules as everywhere in this directory. This document carries no
document-level default: every claim carries its own label. VERIFIED is a byte
read out of the module, an asset or a committed capture (an immediate, a
relocation, a string, a store at an offset, a table entry). INFERRED is
anything read off control flow, which includes the order of two stores and
every branch condition, and anything that says what a field means.

Module: `game.mp.i386.so`, the 1.1d Linux dedicated server's MP game module.
Addresses are the module's own VAs as `nm -D`, `objdump` and
`python3 tools/re/annotate_func.py <elf> <symbol>` print them, the convention
`cod11-combat.md` and `cod11-gsc-object-model.md` use. A Ghidra export that
loads the module at +0x10000 shows each function 0x10000 higher
(`Touch_Item` 0x4d5cc reads `0005d5cc` there). The module is
position-independent, so a call or data reference is only readable with its
relocation resolved; `annotate_func.py` does that. `Pickup_Weapon` (0x4ccd8)
and `GetFreeCueSpot` (0x4da44) carry no dynamic symbol of their own; the names
are the Q3/RTCW ones for what the bytes do.

Offsets used throughout. `ent` is a gentity (stride 0x314), `cl = ent+0x158`
is its client, and the playerstate sits at `cl+0`.

| offset | meaning | evidence |
|---|---|---|
| `ent+0x8c` | `s.index`, the wire `index` (entity netfield offset 140): the `bg_itemlist` index | VERIFIED, `LaunchItem` 0x4dbec and `G_SpawnItem` 0x4e796 store the item index there |
| `ent+0x90` | `s.clientNum` (netfield offset 144): the owner of a drop | VERIFIED, `LaunchItem` 0x4dcae stores its fourth argument, `G_SpawnItem` 0x4e7a5 and `DroppedItemClearOwner` 0x4efba store 0x3fe |
| `ent+0x250` | `count`: the item's reserve ammo, and the entity field `count` (592) | VERIFIED, entity field table offset 592 (`cod11-gsc-object-model.md`) |
| `ent+0x2cc` | the item's clip ammo | VERIFIED, `Drop_Weapon` 0x4e07b stores the player's clip there; the name is INFERRED |
| `ent+0x268` / `+0x26c` | `wait` / `random` | VERIFIED, `G_SpawnItem` 0x4e656 and 0x4e66f `G_SpawnFloat("random"/"wait", "0")` into them |
| `ent+0x16e` | pickup sound alias index | VERIFIED, `G_SpawnItem` 0x4e6ce stores `G_SoundAliasIndex(G_SpawnString("noise"))` |
| `ent+0x172` | "touch armed" byte on an item | VERIFIED, `Touch_Item_Auto` 0x4eeb3 writes 1, `Touch_Item` 0x4d5e5 tests it and 0x4d5f2 clears it |
| `ent+0x178` | `spawnflags` | INFERRED, from CoDExtended's `gentity_t` layout |
| `ent+0x17c` | `flags`; bit 0x10 is the dropped-item flag | VERIFIED, `LaunchItem` 0x4dd24 stores 0x10 and `Touch_Item` tests it at 0x4d9ad and 0x4da0e; the name is INFERRED |
| `ent+0x298` | `gitem_t *item` | VERIFIED, `G_SpawnItem` 0x4e692 and `LaunchItem` 0x4dc04 |
| `bg_itemlist` row | `+0x1c` quantity, `+0x20` giType, `+0x24` giTag (the weapon index) | VERIFIED, `.data` 0x7b9d8, stride 0x30: rows 67..69 read quantity 10/25/50 with giType 3, rows 65/66 giType 2 with giTag -1 |
| weapon def | `+0x8` displayName, `+0x78` weaponSlot, `+0x7c` slotStackable, `+0x198` startAmmo, `+0x1ac` maxAmmo, `+0x1b0` clipSize, `+0x1b4` sharedAmmoCapName, `+0x1bc` sharedAmmoCap, `+0x2d4` clipOnly, `+0x300` dropAmmoMin, `+0x304` dropAmmoMax | VERIFIED, the weapon field table's `{name, offset, type}` records in `.data` (`dropAmmoMin` 0x7d114 = {0x71585, 0x300, 4}) |
| `cl+0x30c` | `ps.weapons`, the held-weapon bit array | VERIFIED, the `Com_BitCheck` base in `BG_CanItemBeGrabbed` 0x2c56b and `Pickup_Weapon` 0x4cf31 |
| `cl+0x314..0x319` | `ps.weaponslots`, indexed by `weaponSlot` 1..5 | VERIFIED, `BG_TakePlayerWeapon` 0x36c51 and `Pickup_Weapon` 0x4d09b |
| `cl+0x10c` / `cl+0x20c` | `ps.ammo[]` / `ps.ammoclip[]` | VERIFIED, `Add_Ammo` 0x4ca56 and 0x4ca65 |
| `cl+0x384` / `+0x388` / `+0x38c` | `serverCursorHint` / `serverCursorHintVal` / `serverCursorHintString` | VERIFIED, player netfield offsets 900, 904 and 908 in `crates/common/src/net/fields_v1.rs` |
| `cl+0x3b8` | the entity number the use key would act on, 0x3ff for none | VERIFIED, `G_CheckForCursorHints` 0x4f5ce and 0x4f652 store it and `Cmd_Activate_f` 0x484d3 reads it |

## 1. Touch

VERIFIED: `ClientThink_real` calls `G_TouchTriggers` at 0x405b3,
`ClientImpacts` at 0x40614 and `Cmd_Activate_f` at 0x4064e. VERIFIED: a
`test [cl+0x21f0], 0x40` sits at 0x4063e, and `cl+0x21f0` is written at
0x4011d from `cmd.buttons & ~oldButtons` (0x40106..0x4011d). INFERRED: the
touch pass runs once per usercmd, and the use key acts after the same cmd's
touch pass, on its rising edge only (section 2). INFERRED: the touch pass is
skipped when `cl+0x21d8` is non-zero (0x405a6).

VERIFIED, `G_TouchTriggers` (0x3f88c): it compares `ps.pm_type` against 1 at
0x3f8a9, queries `trap_EntitiesInBox` over `ps.origin ± (40, 40, 52)` (range
vector `.data` 0x7dcdc) with contents mask 0x405c0008 (0x3f918), and calls
`BG_PlayerTouchesItem(ps, ent, level.time)` at 0x3fa03. INFERRED: the pass
returns for `pm_type > 1`, so only a live player (`PM_NORMAL` or
`PM_NORMAL_LINKED`) touches anything. INFERRED: an `eType` 3 entity is tested
by `BG_PlayerTouchesItem` instead of `trap_EntityContact`, so an item is
touched by a proximity box and not by its bounds.

VERIFIED, `BG_PlayerTouchesItem` (0x2e3bc): it evaluates the item's `pos`
trajectory at `level.time` and compares `ps.origin - itemOrigin` against 36
and -36 on x and y (`.rodata` 0x701b8, 0x701bc) and against 18 and -88 on z
(0x701c0, 0x701c4). INFERRED: a touch is `|dx| <= 36`, `|dy| <= 36` and
`-88 <= ps.z - item.z <= 18`: an item up to 18 below the player's origin
or up to 88 above it. INFERRED: since the box query reaches only 52 units in
z and an item's bounds are about ±1, the box query and not the -88 caps how
far above the player's origin an item can sit, at about 52.

VERIFIED: the pass holds a notify of the item `"touch"` with the player
(0x3fa42) and of the player `"touch"` with the item (0x3fa61), through
`scr_const+0x90`, which `GScr_LoadConsts` fills with `"touch"` at 0x58c28.
VERIFIED: it calls `ent->touch(ent, player, 1)` at 0x3fa84..0x3fa8b.
INFERRED: both notifies run on a touch, gated on `Scr_IsSystemActive`, and
come before the touch function, so they fire for an item the grab test then
refuses.

VERIFIED: every item's touch function is `Touch_Item_Auto`; the only two
relocations to it are in `G_SpawnItem` (0x4e788) and `LaunchItem` (0x4dcd0).
VERIFIED: `Touch_Item_Auto` writes 1 to `ent+0x172` and calls
`Touch_Item(ent, other, bTouched)` (0x4eeb3..0x4eec0).

## 2. Use and the cursor hint

VERIFIED, `Cmd_Activate_f` (0x48468): it calls `G_CheckForCursorHints(player)`
at 0x484c5 and reads `cl+0x3b8` at 0x484d3 against 0x3ff. VERIFIED: past an
`eType` compare against 3 (0x48557) it holds a notify of the item `"touch"`
with the player (0x4857b), a store of 1 into the item's `+0x172` (0x4859a) and
a call of `ent->touch(ent, player, 0)` (0x485a4..0x4869f). INFERRED: for an
item the use key runs the same `Touch_Item` with `bTouched = 0`, and no
`"touch"` goes to the player on a use.

VERIFIED: `Cmd_Activate_f` tests `Scr_IsSystemActive` (0x4847e) and the
player's own `+0x172` byte (0x4848e) before the hint call. INFERRED: a set
player byte turns the use key into something else entirely (it rewrites the
byte and returns at 0x484a6..0x484bc); section 11 lists it.

### 2.1 `G_GetActivateEnt`: the candidates and their score

VERIFIED, `G_GetActivateEnt` (0x4f14c):

- the forward vector is `AngleVectors(ps.viewangles)` (`cl+0xc0`, 0x4f18f);
- the muzzle is `CalcMuzzlePoint(player)` (0x4f19f);
- the query is `trap_EntitiesInBox` over `muzzle ± (192, 192, 96)` (`.rodata`
  0x7544c = 192.0, 0x75450 = 192.0, 0x75454 = 96.0) with at most 0x400
  results and contents mask 0x200000 (0x4f224..0x4f237);
- the per-entity tests read `eType` against 3 (0x4f287) and the contents
  byte `ent+0x11a` against 0x20 (0x4f290), and compare the player's own
  pointer against the entity (0x4f27f);
- the centre is `(absmin + absmax) * 0.5` (`ent+0x11c`, `ent+0x128`,
  0x75464 = 0.5);
- the distance is the return of `VectorNormalize(centre - muzzle)` (0x4f327),
  compared against 128.0 (0x75468) at 0x4f32f;
- the cosine is the normalized direction dotted with the forward vector,
  compared against 0 (0x4f363) and against 0.76 (0x75460, reads
  0.7599999905) at 0x4f372;
- the score is `(1 - (dot - 0.76) / 0.24) * 256` (0x75470 = 0.24, 0x7546c =
  256.0, 0x4f381..0x4f393), with an add of the distance at 0x4f3df;
- a call to `BG_CanItemBeGrabbed(ent, ps, 0)` (0x4f3b8) behind a second
  `eType` compare against 3 (0x4f39c), and an add of 10000.0 (0x75474) to the
  score beside an increment of a count (0x4f3ca..0x4f3d6).

INFERRED from those branches: a candidate is an item, or anything with
contents 0x200000, other than the player; it is kept when the distance is at
most 128 and the cosine at least 0.76 (about 40.5 degrees off the
crosshair); its score is `(1 - (dot - 0.76) / 0.24) * 256 + dist`, plus 10000
and counted when `BG_CanItemBeGrabbed` returns 0.

VERIFIED: the list is sorted with `qsort` and the comparator at 0x50f88
(0x4f421), and the ungrabbable count is subtracted from the list length
(0x4f42c). VERIFIED: the comparator returns the difference of the two scores
truncated to an int under a control word `| 0xc00` (0x50f94..0x50fb0).
INFERRED: the sort is ascending, and two scores less than 1.0 apart compare
equal, so their order is whatever `qsort` leaves.

VERIFIED, the second pass (0x4f450..0x4f565): a loop over the list that
traces from the muzzle to a candidate's centre with `trap_Trace`, contents
mask 0x11 and `ps.clientNum` (`cl+0xac`) as the pass entity (0x4f503..0x4f52e),
compares the result's entity word against 0x3fe (0x4f536), and holds an add
of 100000.0 (0x75478) to the score beside an increment of a second count
(0x4f53e..0x4f552). VERIFIED: a second `qsort` call (0x4f57d) and a subtract
of the second count (0x4f582) close the function.
INFERRED: the pass walks the list in score order and stops at the first
trace that does not end on the world (0x3fe), so every blocked candidate
ahead of it is pushed past the end and counted out; a candidate behind the
first clear one is never traced.

INFERRED, the net rule: the use key reaches the first unblocked candidate in
score order, and an ungrabbable item never, since the 10000 puts it behind
every grabbable one and the length it returns excludes it.

### 2.2 `CalcMuzzlePoint` truncates

VERIFIED, `CalcMuzzlePoint` (0x69218): it copies `r.currentOrigin`
(`ent+0x134..0x13c`), adds `ps.viewHeightCurrent` (`cl+0xd0`) to z
(0x69245), and calls `G_AddLean` (0x69253). VERIFIED: it rewrites each of the
three components through `fistp`/`fild` under a control word `| 0xc00`
(0x6926d, 0x69294, 0x692bc), which is rounding control 11, truncation toward
zero. INFERRED, off the three rewrites sitting after the call: the muzzle is
the leaned eye truncated on every axis, the same shape
`cod11-gsc-object-model.md` 23.1 records for `CalcMuzzlePoints`.

### 2.3 `G_CheckForCursorHints`: what the hint fields read

VERIFIED: `G_CheckForCursorHints` (0x4f59c) is called from `ClientEndFrame`
(relocation at 0x4111a) and from `Cmd_Activate_f` (0x484c5).

VERIFIED, the stores: `cl+0x384 = 0`, `cl+0x388 = 0` and `cl+0x3b8 = 0x3ff`
at 0x4f5ba..0x4f5ce; a compare of the player's `health` (`ent+0x230`) against
0 at 0x4f5d7 and of the player's `+0x172` byte against 0 at 0x4f5e4;
`cl+0x38c = -1` at 0x4f603; the call to `G_GetActivateEnt` at 0x4f622.
INFERRED: the writes past the two gates happen only for a live player
whose `+0x172` byte is clear; a dead player keeps whatever
`serverCursorHintString` last read.

VERIFIED: the function stores the first list entry's number into `cl+0x3b8`
(0x4f652). VERIFIED, the item arm's instructions: giType compares against 1, 2
and 3 (0x4f76e..0x4f77e); `mov edi, 7` (0x4f787); a `Com_BitCheck(ps.weapons,
giTag)` (0x4f7aa); `giTag` loaded with `add edi, 9` (0x4f7b9) and, at a second
site, with `add edi, 0x49` (0x4f7c6). INFERRED: giType 3 takes the 7, giType 1
takes the bit check and one of the two adds, and giType 2 jumps straight to
the `0x49` add (0x4f77c). INFERRED: an unowned weapon hints `9 + giTag`, an
owned weapon or an ammo item `73 + giTag`, health 7 (`HINT_HEALTH`, slot 7 of
the `hintStrings` table in `cod11-gsc-object-model.md`), and any other giType
0.

VERIFIED: past the arms, `ent+0xdc` is read (0x4f828) and moved into the hint
register at 0x4f836; the hint goes to `cl+0x384` (0x4f83e), 0 to `cl+0x388`
(0x4f844), the string index to `cl+0x38c` (0x4f854), and 0x3ff is stored into
`cl+0x3b8` at 0x4f85e. INFERRED: `ent+0xdc` replaces a non-zero computed hint
when it is above 0, so a `setCursorHint` on the item would override it; a zero
hint clears the chosen entity; an item's `serverCursorHintString` is -1; and
only the first candidate is looked at (the loop advances only past an
unusable turret, 0x4f870).

## 3. The grab test and the most a player can take

VERIFIED, `BG_CanItemBeGrabbed(es, ps, bTouched)` (0x2c4f0), the instructions:
an index range check with `Com_Error(1, "BG_CanItemBeGrabbed: index out of
range")` (0x70000, 0x2c50f); `es.clientNum` (`+0x90`) compared against
`ps.clientNum` (`+0xac`) at 0x2c535; giType 0 to `Com_Error(1,
"BG_CanItemBeGrabbed: IT_BAD")` (0x70029); the weapon arm's
`Com_BitCheck(ps.weapons, giTag)` at 0x2c572 and `bTouched` compare at
0x2c57e; the health arm's `stats[0]` (`+0xf4`) against `stats[2]` (`+0xfc`)
at 0x2c586.

INFERRED, the rule off those branches:

1. an item whose `clientNum` equals the player's is refused, which locks a
   drop against its dropper until `DroppedItemClearOwner` writes 0x3fe
   (section 8);
2. giType above 3 is refused;
3. weapon (giType 1): held, it answers `BG_GetMaxPickupableAmmo > 0`; not
   held, it answers `bTouched == 0`;
4. ammo (giType 2): held, `BG_GetMaxPickupableAmmo > 0`; not held, only a
   `clipOnly` weapon with `BG_GetMaxPickupableAmmo > 0` (0x2c596..0x2c5db);
5. health (giType 3): `stats[0] < stats[2]`, health below max health.

INFERRED, what that does in play: walking over a weapon the player carries
tops up its ammo; walking over one it does not carry does nothing, and only
the use key takes it; a carried weapon at full ammo is refused on touch and
never offered to the use key.

`BG_GetMaxPickupableAmmo(ps, weapon)` (0x36d98). VERIFIED: its three arms read
the shared-cap index `def+0x1b8` against 0, the `sharedAmmoCap` table at
0xa9a00, the `clipSize` table at 0xa9c20 with `ps.ammoclip`, and the
`maxAmmo` table at 0xa97e0 with `ps.ammo`. INFERRED, the arithmetic:

- a weapon on a shared cap: `sharedAmmoCap[idx]` minus, over every held weapon
  on the same cap, its clip (for `clipOnly`, each clip index once) or its
  reserve (otherwise, each ammo index once);
- else a `clipOnly` weapon: `clipSize[clip] - ps.ammoclip[clip]`;
- else: `maxAmmo[ammo] - ps.ammo[ammo]`. The loaded clip is not counted.

VERIFIED, weapon files: every stock frag (`fraggrenade_mp`,
`stielhandgranate_mp` and the rest) is `clipOnly 1`, `clipSize 3`,
`maxAmmo 3`, `sharedAmmoCap 3`, `sharedAmmoCapName grenades`. INFERRED: the
grenade cap is 3 across every grenade type together.

## 4. Ammo arithmetic

### 4.1 What the item hands out

`Pickup_Weapon` (0x4ccd8) derives the item's reserve and clip before it looks
at the player.

The reserve, `count` (`ent+0x250`). INFERRED: a negative `count` reads as 0
(0x4cd16). VERIFIED: `dropAmmoMax` (`+0x304`) is loaded at 0x4cd2d and
`dropAmmoMin` (`+0x300`) at 0x4cd33. INFERRED: a zero `count` loads them and
orders them so the larger is first (0x4cd3b..0x4cd41). INFERRED, from the x87
sequence at 0x4cd4b..0x4cdad: when both are 0, `count = 1 + trunc(0.5 *
(clipSize - 1) * (1 - rand() / 2^31) + 0.5)`. The constants are VERIFIED:
0x74bcc reads -2^-31 and 0x74bd0 reads 0.5, and the truncation runs under a
control word `| 0xc00`. INFERRED: a placed panzerfaust (`clipSize` 1, both
drop fields 0) always yields `count` 1. INFERRED, off 0x4cdc4..0x4cdd3: when
the two differ, `count = rand() % (max - min) + min`; when equal, `count =
min` (0x4cde3); a result at or below 0 becomes 0 (0x4cdf5). VERIFIED: the
reserve is compared against `BG_GetAmmoTypeMax` (0x4ce1a) and the max is
stored into `+0x250` at 0x4ce3a. INFERRED: the reserve is capped at the ammo
max after it is drawn.

The clip (`ent+0x2cc`). INFERRED: a negative clip reads as 0 (0x4ce5c).
INFERRED, off 0x4ce68..0x4ced1: a zero clip becomes `min(clipSize, count)`
and that amount leaves `count`; a zero clip on an item whose `count` field is
negative stays 0. INFERRED: the clip is then capped at `clipSize`
(0x4ceef..0x4cf0f).

INFERRED, the stock numbers: a placed `mpweapon_fg42` (`count 90`, no clip,
`clipSize` 20, `maxAmmo` 320) hands out clip 20 and reserve 70. VERIFIED, the
placements and files: `mpweapon_fg42` and `mpweapon_mp44` carry `count 90` in
every stock BSP that places them, `mpweapon_panzerfaust` carries no `count`;
`fg42_mp` is clip 20 / max 320 / drop 100..200, `mp44_mp` 30 / 240 /
150..210, `panzerfaust_mp` 1 / 1 / 0..0. VERIFIED, `default_mp.cfg` and a
retail run (section 12.1): a stock server sets `scr_allow_fg42 0` and has no
placed fg42 left after the map load.

### 4.2 `Add_Ammo(ent, weapon, count, fillClip)` (0x4ca10)

VERIFIED, the stores and calls in address order: `ps.ammo[a] += count`
(0x4ca76); `BG_GivePlayerWeapon` (0x4ca97); a subtract from the reserve and an
add to the clip (0x4cb31, 0x4cb36); a store of 0 into the reserve (0x4cb55);
`BG_GetAmmoTypeMax` stored into the reserve (0x4cbaa); `BG_GetAmmoClipSize`
stored into the clip (0x4cbf5); `BG_GetMaxPickupableAmmo` (0x4cc25); an add
into the clip (0x4cc54), a store of 0 into it and `BG_TakePlayerWeapon`
(0x4cc6b, 0x4cc7d); an add into the reserve and a store of 0 into it
(0x4cc94, 0x4cca8).

INFERRED, the order and the conditions: the reserve gets the count first; a
`clipOnly` weapon is given the weapon; with `fillClip` or `clipOnly`,
`min(clipSize - clip, reserve)` moves from the reserve to the clip
(0x4cae3..0x4cb36); a `clipOnly` reserve is then zeroed and any other
is capped at the ammo max; the clip is capped at `clipSize`; a shared cap over
its limit is taken off the clip for `clipOnly`, taking the weapon away at 0,
else off the reserve. The fill also checks the weapon index is within
`1..BG_GetNumWeapons()` (0x4cae3..0x4caf1).

VERIFIED, the tail (0x4ccaf..0x4ccd5): it reads `ps.ammo[a]` and
`ps.ammoclip[c]`, subtracts the two values saved on entry (0x4ca5f, 0x4ca70),
and returns the sum. INFERRED: `Add_Ammo` returns
`(ammo + clip) after - (ammo + clip) before`, except the `clipOnly`
shared-cap arm that takes the weapon, which returns 0 (0x4cc82).

### 4.3 How `Pickup_Weapon` spends it

VERIFIED: `Pickup_Weapon` calls `Com_BitCheck(ps.weapons, W)` at 0x4cf37 and
branches on it at 0x4cf44 to 0x4d215.

The owned arm, 0x4d215. VERIFIED: it holds a store of 0x94 (`EV_AMMO_PICKUP`,
148) through the event pointer (0x4d218), a call of `Add_Ammo(player, W,
reserve + clip, 0)` (0x4d22f), and `trap_SendServerCommand` of `f
"GAME_PICKUP_AMMO\x14%s"` (0x74b21) or `f "GAME_PICKUP_CLIPONLY_AMMO\x14%s"`
(0x74b00) with the weapon's displayName (0x4d23d..0x4d28f). INFERRED: the
string goes only on a non-zero gain. INFERRED, off 0x4d297..0x4d2ff: when the
gain is the whole amount it goes straight to the notify at 0x4d3a1; a partial
take subtracts the gain from `count`, then the overflow from the clip, writing
-1 for an emptied `count` or clip; with ammo left over it reaches the notify
when `g_weaponAmmoPools` is 0 (the item is consumed and the remainder lost),
and returns 0 at 0x4d2ff when it is 1 (the item stays with the reduced counts,
no event, no notify). VERIFIED: the `g_weaponAmmoPools` default is "0".
INFERRED: on a stock server the owned arm reaches the `"trigger"` notify on
every taken pickup.

VERIFIED, the unowned arm's calls: `BG_GivePlayerWeapon` (0x4d1cd) and
`trap_SendServerCommand(client, 1, "a %i", W)` (`.rodata` 0x74bc7,
0x4d1e5..0x4d203) beside a compare of `bTouched` (0x4d1d5). INFERRED: the arm
runs the slot logic (section 5), gives the weapon, and sends `a` only on a use
(`bTouched == 0`). VERIFIED: the clip is compared against `clipSize`
(0x4d322), the excess added into the reserve (0x4d33c..0x4d345), the result
stored into `ps.ammoclip[c]` (0x4d37f), and `Add_Ammo(player, W, reserve,
clip == -1)` called at 0x4d399. INFERRED: the
item's clip overwrites the player's clip for that index rather than adding to
it. INFERRED: the `clip == -1` argument is always 0, since the clip was
floored at 0 at 0x4ce5c, so `fillClip` never fires from here. INFERRED, from
`cgame_mp_x86.dll` 1.1 `FUN_3002e0d0` case `'a'` calling `FUN_30037f20`,
which writes `cg_weaponSelect`: the client switches to the weapon it picked
up.

## 5. Slots and swaps

VERIFIED: `ps.weaponslots` is `cl+0x314..0x319`, indexed by the weapon file's
`weaponSlot` 1..5 (`primary`, `primaryb`, `pistol`, `grenade`,
`smokegrenade`, table `.data` 0x7c940, also in `cod11-gsc-object-model.md`).
INFERRED, from `BG_GetEmptySlotForWeapon` (0x3aaac) and `BG_GivePlayerWeapon`
(0x36a38): a `primary`-class weapon (slot 1 or 2 in its file) takes slot 1,
else slot 2, and classes 3..5 take their own slot, so a player holds two
primaries, a pistol, a grenade and a smoke grenade at most. VERIFIED: no stock
`weapons/mp/*` sets `slotStackable` to 1 (four grenades set 0, the rest omit
it). INFERRED, from its first test of `def+0x7c`: `BG_GetStackSlotForWeapon`
(0x36cc8) never offers a slot on stock content. VERIFIED: `panzerfaust_mp` is
`weaponSlot primary`, `clipOnly 1`.

VERIFIED, `BG_GivePlayerWeapon` (0x36a38), the instructions: a
`Com_BitCheck` of the weapon (0x36a52) with a `jne` to 0x36a73, which is
`xor eax, eax` and a jump to the epilogue; compares of `def+0x74` against 6
(0x36a69, `je` to 0x36a73) and 7 (0x36a6e); a `Com_BitSet` of the weapon
(0x36a90); byte compares against 0 and stores of the weapon at `cl+0x315`
(0x36ac8, 0x36ad3), `cl+0x316` (0x36ae3, 0x36aee) and `cl+0x314 + weaponSlot`
(0x36aff, 0x36b07); and `def+0x2fc` read at 0x36b0a and 0x36b42 beside a
`Com_BitCheck` (0x36b5a) and a `Com_BitSet` (0x36b22) of the index it holds.
INFERRED, off those branches: a weapon already held is not given again; a new
one takes the first empty slot its class allows, or none; and every weapon on
its alt-fire chain that is not yet held is held beside it with no slot, which
is the `fg42_semi_mp` bit the capture reads beside the fg42's (section 12.4).
INFERRED: a weapon whose `def+0x74` reads 6 or 7 is never given; what that
field is I have not read.

VERIFIED, `BG_IsPlayerWeaponInSlot(ps, weapon, followAlt)` (0x3a9f4), the
instructions: byte loads of `cl+0x315` and `cl+0x316` (0x3aa53, 0x3aa5e) and
of `cl+0x314 + weaponSlot` (0x3aa7b), each compared against the weapon index;
a test of the third argument (0x3aa87) beside a read of `def+0x2fc`
(0x3aa8d); and a compare against the starting index with a `jne` back to the
slot compares (0x3aa99, 0x3aa9c). INFERRED: with `followAlt` set, a weapon
counts as in a slot when it or any weapon on its alt-fire chain sits in one,
so an alt mode in hand is in its base weapon's slot. VERIFIED: `Pickup_Weapon`
pushes 1 as that argument (0x4cf77).

INFERRED, the slot conflict in `Pickup_Weapon`, off the branches at
0x4cf44..0x4d160, for a new weapon W and the held weapon H (`ps.weapon`,
`cl+0xb0`):

1. W has an empty slot: give it, drop nothing (0x4cff7..0x4d00f).
2. Otherwise W's `weaponSlot` equals H's: drop H (0x4d06a..0x4d0a6).
3. Else W's slot is 3..5: drop what sits in `weaponslots[W.slot]`
   (0x4d082..0x4d0a6).
4. Else W is a primary, H is not, and both primary slots are full: the loop
   over slots 1..2 drops that slot's weapon when `ammo[ammoIndex(W)] == 0 &&
   clip[clipIndex(W)] == 0` (0x4d0d0..0x4d128). The test reads W's indices,
   not the slot weapon's, so it does not vary with the loop: either slot 1 is
   dropped or the loop runs out and the player gets
   `f "GAME_CANT_GET_PRIMARY_WEAP_MESSAGE"` (0x74ba0, sent at 0x4d152) and
   nothing is picked up.
5. H is held but sits in no slot and W has no empty or stack slot:
   `Com_Printf` of the "cannot swap out a debug weapon" line (0x74b40) and
   the pickup fails (0x4cf74..0x4cfeb).

VERIFIED, `Pickup_Weapon`'s instructions before case 1: a `test` of
`ps.weapon` (`cl+0xb0`, 0x4cf56) with a `je` to 0x4cff7; a `Com_BitCheck` of
it (0x4cf68) with a `je` at 0x4cf72 to 0x4cff0, which is `xor eax, eax` and a
jump to the epilogue. INFERRED: while `ps.weapon` names a weapon the player no
longer holds, every pickup of an unowned weapon fails with no message.

VERIFIED: the stores of `Drop_Weapon`'s return into `ebp-0x2c` (0x4d0ab,
0x4d0c5); a compare of `ebp-0x2c` against 0 at 0x4d160 with a `je` to 0x4cff0
(0x4d164); `BG_GivePlayerWeapon` called at 0x4d1cd. INFERRED: a swap whose
drop yields no entity returns 0 before the give, with the old weapon already
taken; an empty `clipOnly` weapon, which `Drop_Weapon` takes without dropping
(section 8), is the stock way there, and the next use finds the slot empty
and takes the item.

VERIFIED: the drop is `Drop_Weapon(player, weapon, NULL)` (0x4d0a6, 0x4d0c0),
and `G_SetOrigin` with the picked-up item's `currentOrigin` (`+0x134`),
`G_SetAngle` with its `currentAngles` (`+0x140`) and `trap_LinkEntity` are
called on the dropped entity at 0x4d16a..0x4d1b7. INFERRED: a swap puts the
old weapon exactly where the new one lay, at rest, since `G_SetOrigin`
replaces the launch trajectory.

## 6. Health

`Pickup_Health` (0x4d400). VERIFIED: it compares the item's quantity against
5 and 100 (0x4d418, 0x4d41d) and holds an `add edx, edx` on `stats[2]`
(0x4d43c); it reads `count` and the quantity (0x4d441, 0x4d459); it
multiplies the amount by `stats[2]` and by 0.01 read as a float at 0x74bf0
(0.0099999998) and converts the product under a control word `| 0xc00`
(0x4d492); it compares the new health against the cap at 0x4d4a1.

INFERRED, the arithmetic:

- cap = `2 * maxHealth` for a quantity of 5 or 100, else `maxHealth`;
- `health += trunc(amount * maxHealth * 0.01f)`, a truncation, not a rounding;
  with the float constant below 0.01, `25 * 100 * 0.01f` truncates to 24;
- over the cap, health becomes the cap;
- otherwise `newPct = trunc(health * 100 / maxHealth)` clamped to 1..100 and
  `oldPct = max(1, trunc(oldHealth * 100 / maxHealth))`; when
  `newPct != min(oldPct + amount, 100)`, health becomes
  `min(oldPct + amount, 100) * maxHealth / 100` in integer division
  (0x4d4a9..0x4d557).

INFERRED, at `maxHealth` 100: the re-rounding undoes the truncation's lost
unit, so the result is exactly `min(health + amount, cap)`, which is what
`health + round(maxHealth * amount / 100)` capped also gives there. At any
other `maxHealth` the two can differ by a point (150 and 25 from 50: retail 87,
the rounded model 88).

VERIFIED: it writes `ps.stats[0]` (0x4d569), sends
`f "GAME_PICKUP_HEALTH\x15%i"` (0x74bd4) with the amount, not the gain
(0x4d575), notifies the item `"trigger"` with one argument, the player
(0x4d5a0..0x4d5b6), and returns -1 (0x4d5bb).

VERIFIED, stock scripts: `dm.gsc` 534 and `tdm.gsc` 629 call
`self dropHealth()` in `Callback_PlayerKilled`, and `dropHealth()` (dm 1067,
tdm 1196) spawns `item_health` at `self.origin + (0, 0, 1)` with a random yaw
into a 16-entry `level.healthqueue` that `delete()`s the entry it overwrites;
both `precacheItem("item_health")` (dm 169, tdm 178); `sd`, `re` and `bel`
drop no health. INFERRED: health drops are script, not engine, and only item
68 (`item_health`, "Med Health", quantity 25) is used in stock play.

## 7. What a pickup writes

`Touch_Item` (0x4d5cc). VERIFIED, the instructions: the item's `+0x172` is
tested and cleared, `other->client` is tested, and `other->health` (`+0x230`)
is compared against 0 (0x4d5e5..0x4d60e). INFERRED: a pickup needs the byte
armed, a client, and a live one.

VERIFIED: `Touch_Item` references `f "GAME_PICKUP_CANTCARRYMOREAMMO\x14%s"`
(0x74c20) and a per-slot `f
"GAME_CANT_GET_{PRIMARY,PISTOL,GRENADE,SMOKER}_WEAP_MESSAGE"` through the jump
table at `.rodata` 0x74d2c (slots 1 and 2 to 0x74ba0, 3 to 0x74c60, 4 to
0x74ca0, 5 to 0x74ce0). INFERRED: they are sent on a refused grab with
`bTouched == 0` on a weapon item that is not the player's own drop, the first
when the weapon is held. INFERRED: both are unreachable in stock play, since
the use key never selects an ungrabbable item and a use on an unowned weapon
always passes the grab test.

VERIFIED: `Touch_Item` calls `G_LogPrintf("Weapon: %i %s\n", playerEntNum,
def+4)` (0x74d06) or `"Item: %i %s\n"` (0x74d15) with the classname.
INFERRED: an accepted grab logs the first for a weapon and the second
otherwise, and the line goes to `games_mp.log` before the pickup
function runs, so a refused swap still logs. VERIFIED, the retail capture
(section 12.4): `def+4` is the weapon's own name, not the item classname; the
line reads `Weapon: 0 fg42_mp`, not `mpweapon_fg42`.

VERIFIED: `Touch_Item` calls `Pickup_Weapon` at 0x4d798, `Add_Ammo` at
0x4d7b7 and `Pickup_Health`, and loads 40 (`mov edi, 0x28` at 0x4d861).
INFERRED, the dispatch on giType: 1 goes to `Pickup_Weapon`; 2 calls
`Add_Ammo(player, giTag, count ? count : quantity, 0)`, sends the same two
ammo strings, notifies the item `"trigger"` with the player, and takes a
respawn value of 40; 3 goes to `Pickup_Health`.

The event. VERIFIED: 0x92, `EV_ITEM_PICKUP` 146, is stored at 0x4d5de;
`Pickup_Weapon` stores 0x94, `EV_AMMO_PICKUP` 148, through the event pointer
at 0x4d218; 0x93, `EV_ITEM_PICKUP_QUIET` 147, and a `G_PlaySoundAlias(player,
noise)` sit at 0x4d890..0x4d8a3 beside a read of `ent+0x16e`. INFERRED: the
owned arm is what writes 148, and an item with a `noise` key takes 147 and
plays the alias. VERIFIED: `G_AddPredictableEvent` and `G_AddEvent` are both
called on the player with the item's `s.index` (`ent+0x8c`) as parm
(0x4d8ab..0x4d8df), beside a read of `cl+0x2124`; `cl+0x2124` is the
`cg_predictItems` userinfo value (`ClientUserinfoChanged` 0x421bc, 0x421ce;
string 0x730b1). VERIFIED: both write `ps.events[seq & 3]` and
`ps.eventParms[seq & 3]` and bump `ps.eventSequence` (`G_AddEvent` 0x67ca4;
`G_AddPredictableEvent` 0x67c7c through `BG_AddPredictableEventToPlayerstate`
0x2e314). INFERRED: the event fires only when the pickup function returned
non-zero, the prediction flag picks which of the two runs, and no event lands
on the item entity.

Removal (0x4d8e7..0x4da33). INFERRED, off the branches:

- `wait == -1`: `flags |= 0x1000`, `s.eFlags |= 0x100`, `r.contents = 0`,
  `unlinkAfterEvent` (`+0x188`) = 1, and nothing else;
- otherwise the respawn value is the pickup function's return; a non-zero
  `wait` replaces it with `(int)wait`; a non-zero `random` adds
  `crandom() * random` with a floor of 1; a dropped item (`flags & 0x10`) gets
  `freeAfterEvent` (`+0x184`) = 1; then `r.svFlags |= 1`, `flags |= 0x1000`,
  `contents = 0`; a respawn above 0 arms `RespawnItem` at `level.time +
  respawn * 1000`, else the think is cleared; a dropped item's think is
  overwritten with `G_FreeEntity` at `level.time + 100`; `trap_LinkEntity`.

VERIFIED: the dropped-item think store is `G_FreeEntity` into `+0x200` and
`level.time + 0x64` into `+0x1fc` (0x4da17..0x4da29). VERIFIED: `r.svFlags`
bit 1 is the bit the snapshot builder skips on (`docs/protocol-1.1.md`,
entity selection), and `flags |= 0x1000` is the bit `ScrCmd_Hide` sets
(`cod11-combat.md`, 0x5dcdf). INFERRED: a taken item leaves every snapshot on
the frame it is taken; a dropped one is freed 100 ms later; a placed or
script-spawned one stays allocated and hidden.

Respawn values. VERIFIED: `Pickup_Weapon` tests `spawnflags & 8`
(`ent+0x178`, 0x4d3df) and loads `g_weaponRespawn` or -1 (0x4d3e8, 0x4d3f0);
`Pickup_Health` returns -1 (0x4d5bb); the ammo arm loads 40. VERIFIED, cvar
defaults: `g_weaponrespawn` "5", `g_weaponAmmoPools` "0". VERIFIED, stock
BSPs: no `mpweapon_*` has spawnflags 8 (only the eight `mpweapon_panzerfaust`
on `mp_railyard`, `mp_rocket` and `mp_ship` carry spawnflags 1) and none has
`wait` or `random`. INFERRED: no stock placed weapon respawns, and a taken
placed weapon stays hidden for the rest of the level.

VERIFIED, `RespawnItem` (0x4ec7c): `r.contents = 0x407c0008` (0x4ece9, where
spawn writes 0x407c0108), `flags &= ~0x1000`, `svFlags &= ~1`, a link,
`G_AddEvent(ent, EV_ITEM_RESPAWN 197, 0)` (0x4ed0f) and `nextthink = 0`.
VERIFIED: `Use_Item` (0x4efc8) has the same body. INFERRED: `EV_ITEM_RESPAWN`
rides the item entity, and stock never reaches it.

The `"trigger"` notify on a weapon. VERIFIED, 0x4d3a1..0x4d3d7: a compare of
the dropped-entity local against 0, an `Scr_AddEntity` of it, an
`Scr_AddUndefined`, an `Scr_AddEntity(player)` and
`Scr_Notify(item, scr_const+0x92, 2)`; `GScr_LoadConsts` fills `+0x92` with
`"trigger"` at 0x58c40. INFERRED: the first argument pushed is the
swapped-out item when there is one, else undefined, and the player is pushed
second. INFERRED: both arms reach it (the owned arm with
undefined for the swapped-out item), so a taken weapon always notifies on a
stock server (section 4.3).

## 8. Drops and the ring

`Drop_Weapon(ent, weapon, tag)` (0x4dd40).

VERIFIED: it calls `Com_BitCheck(ps.weapons, weapon)` (0x4dd72), compares
`def+0x2d4` (`clipOnly`) and the player's clip against 0 (0x4ddc3..0x4dde5),
and holds a `BG_TakePlayerWeapon` call and a return of 0 (0x4ddef,
0x4ddf4). INFERRED: a weapon the player does not hold, or an empty `clipOnly`
one, is taken and nothing drops.

VERIFIED: the launch velocity is `forward(yaw) * 150` (0x74d4c) with
`z = 200 + crandom() * 50` (0x74d58, 0x74d54); the origin is `currentOrigin`
with z raised by half the entity's height (0x74d5c = 0.5); the call is
`LaunchItem(item, origin, velocity, ent->s.number)` (0x4deb4).

VERIFIED, the player's ammo (0x4dec1..0x4df18): `ps.ammo[a]` is read into the
item's reserve and stored 0 (0x4ded7, 0x4deda), `ps.ammoclip[c]` is read into
the item's clip and stored 0 (0x4defb, 0x4defe), `BG_TakePlayerWeapon` is
called at 0x4df10, the `jmp` at 0x4df18 targets the stores into `+0x250` and
`+0x2cc` (0x4e075, 0x4e07b), and -1 is stored into each at 0x4e085 and
0x4e098. VERIFIED: no instruction between the reads and the stores jumps
backwards. INFERRED: an empty count or clip is written as -1. INFERRED: the
item takes
the player's whole reserve for the weapon's ammo index and the whole clip;
the reserve is not split with other held weapons that share the ammo index,
which lose it with the dropped one. INFERRED: 0x4df20..0x4e073 is the branch
for an entity without a client, which draws the counts from
`dropAmmoMin`/`dropAmmoMax`; no stock path reaches it.

VERIFIED, `BG_TakePlayerWeapon` (0x36b78): it clears the slot byte in
`ps.weaponslots` (0x36c51, 0x36c63), clears the weapon's bit (0x36c73) and
follows `def+0x2fc` clearing each chained weapon's bit (0x36c7e..0x36cb4).
INFERRED: the chain is the alternate-mode weapon.

VERIFIED: the tag argument (`ebp+0x10`) is compared against 0 at 0x4e0a2, with
a `je` to 0x4e273. INFERRED: the block below runs only when a tag is passed,
which `dropItem` always does (its default is `"tag_weapon_right"`, `.rodata`
0x731d4, loaded at 0x43720 in `PlayerCmd_dropItem`) and the swap never does
(it passes NULL). VERIFIED, the tag block's instructions: `G_DObjGetWorldTag`
(0x4e0bb), a capsule trace with mask 0x411 from the entity's centre to the tag
that moves `pos.trBase` and `currentOrigin` and sets `trTime = level.time`
(0x4e14b..0x4e194); angles from the tag axis with roll +90 (0x74d64);
`apos.trType = 2`, `apos.trTime = level.time`, `apos.trDelta = crandom() *
(50, 40, 60)` (0x4e252..0x4e270, constants 0x74d54, 0x74d68, 0x74d6c).

VERIFIED, `LaunchItem(item, origin, velocity, owner)` (0x4db98), the stores:
`RegisterItem(index, 1)`; a slot from `GetFreeCueSpot` (0x4dbc7) stored with
the entity number into the ring at `level+0x1d5c` (0x4dbde); `eType` 3
(0x4dbe5); `s.index` (0x4dbec); classname; bounds out of the two constants
-1 (0x74d44) and 2 (0x74d48);
`svFlags |= 0x200`; `s.eFlags |= 0x10` (`or byte [ent+0x8], 0x10` at
0x4dc93); `contents 0x407c0108`; `clipmask 0x81`; `s.clientNum = owner`
(0x4dcae); the world model; touch `Touch_Item_Auto` (0x4dcd0);
`pos.trType 5`, `trTime = level.time`, `trDelta = velocity`; think
`DroppedItemClearOwner` at `level.time + 1000`; `flags = 0x10` (0x4dd24).
INFERRED: a weapon item gets bounds `(-1,-1,-1)..(1,1,1)` and any other
`(-1,-1,0)..(1,1,2)`.

VERIFIED, `DroppedItemClearOwner` (0x4efb4): its whole body writes 0x3fe into
`s.clientNum` (0x4efba). INFERRED: the dropper can pick its own drop up again
1000 ms after the launch.

VERIFIED, `GetFreeCueSpot` (0x4da44..0x4db97), the instructions: a slot loop
bounded by 0x1f (0x4db55); a test of the ring entry against 0 (0x4da6c); a
test of the entity's `inuse` byte (`+0x160`, 0x4da8c) beside a store of 0 into
the ring entry (0x4da97); a starting distance of 99999.0 (0x74d40), a
`VectorDistance` call, and compares of `cl+0x20ec` against 2 and `ps.pm_type`
against 5 (0x4dade, 0x4dae7); an `fcom` of each slot's distance against the
best so far (0x4db3a); a store of `G_FreeEntity` and `level.time + 1` into the
chosen slot's entity (0x4db75..0x4db85). INFERRED: an empty ring entry returns
its slot, as does an entry whose entity is gone, after zeroing it; a live
entry's distance is the smallest to any client at `cl+0x20ec` 2 and `pm_type`
5, else 99999; the largest distance wins on a strict greater-than. INFERRED: a
free slot, or one whose entity is gone, is taken first; with all 32 held, only
intermission clients count toward the distance, so in play every slot scores
99999, slot 0 is the only one strictly above the starting 0, and slot 0 is
evicted every time. INFERRED: the new drop then takes slot 0, so once the ring
is full, each further drop evicts the one before it.

VERIFIED, `G_RunItem` (0x4eb18): a `trap_PointContents` with mask 0x80000000
(0x4ec47..0x4ec4f) and a `G_FreeEntity` (0x4ec5f). INFERRED: an item whose
origin lands in `CONTENTS_NODROP` is freed. INFERRED, from the absence of any
other free: a drop lives until it is picked up, until a 33rd drop evicts it,
or until it lands in `CONTENTS_NODROP`. VERIFIED: the two `0x7530` immediates
in `.text` sit in `Cmd_CallVote_f` (0x47fac) and `fire_rocket` (0x54666), so
no 30-second item timer exists.

VERIFIED: `ClientEvents` (0x3fd24) turns `EV_DROPWEAPON` (196) into
`Drop_Weapon(player, ps.weapon, "tag_weapon_right")` (jump table `.rodata`
0x72c84, 0x3fe50..0x3fe8c). INFERRED: nothing in the module raises that event,
since an immediate search finds no raiser.

## 9. The item on the wire

VERIFIED, the stores: `eType` 3; `index` the `bg_itemlist` index (a weapon's
index 1..64 through configstring 7, 65..69 fixed); `eFlags |= 0x10` in
`G_SpawnItem` (0x4e774), `FinishSpawningItem` (0x4e32b) and `LaunchItem`
(0x4dc93); `clientNum` 0x3fe for a placed or spawned item (0x4e7a5), the
dropper's entity number on a drop (0x4dcae) and 0x3fe once
`DroppedItemClearOwner` runs (0x4efba). VERIFIED: the entity netfield
`clientNum` is 8 bits, so 0x3fe arrives as 254, the "constant 254 sentinel"
of `clientstate-wire-format.md`.

VERIFIED, against `crates/server/tests/fixtures/entities/mp_carentan-dm.txt`:
placed items read `eFlags 16`, `clientNum 254`, `groundEntityNum 1022`, `index
23` (the panzerfaust) and `apos.trBase[2]` 0x42b40000 (90.0). VERIFIED:
`FinishSpawningItem` traces a capsule 4096 units down with mask 0x411
(0x4e350, 0x74db4), calls `G_SetOrigin`, compares the item's giType against 1
at 0x4e4c6, and adds 90 (0x74dbc) to the roll at 0x4e4cf. INFERRED: the roll
add runs only for a weapon item (giType 1). INFERRED: a placed item is dropped
to the floor with `pos.trType` 0, which is what the fixture carries.

VERIFIED, `G_SpawnItem` (0x4e634): a compare of `level.spawning`
(`level+0x1344`) against 0 at 0x4e7c3; an arm that stores `FinishSpawningItem`
as the think at `level.time + 200` (0x4e7dc..0x4e7ec); a test of
`spawnflags & 1` at 0x4e7f8 with a `jne` to 0x4e820; a store of 0x3ff into
`groundEntityNum` (`ent+0x7c`, 0x4e801); a giType compare against 1 at
0x4e808; an add of 90 (0x74e40) to the roll (`ent+0x148`, 0x4e814); then
`G_SetAngle`, `G_SetOrigin` and `trap_LinkEntity` (0x4e82b..0x4e847).
VERIFIED: `level.spawning` is written only by `G_SpawnEntitiesFromString`
(0x622e4, 0x62326). INFERRED: during the map load an item waits 200 ms for
`FinishSpawningItem`; outside it (a script `spawn`) the item is linked in
place, and unless `spawnflags & 1` it gets `groundEntityNum` 0x3ff and, for a
weapon, the +90 roll. INFERRED: a script spawn with `spawnflags & 1` skips
both, since the `jne` at 0x4e7ff passes over the 0x3ff store and the roll
add. INFERRED: `G_RunItem` then switches an airborne item to `pos.trType` 5
at `level.time` (0x4eb24..0x4eb3f), so it falls and settles.

VERIFIED: the bounds (±1), `contents 0x407c0108` and `svFlags 0x200` are
server-side only; none is an entity netfield. INFERRED: the touch never uses
the bounds, only `BG_PlayerTouchesItem`'s box (section 1).

INFERRED: a pickup takes the item out of snapshots through `svFlags & 1`
(section 7) and puts no event on the item. VERIFIED: `EV_ITEM_POP` (198) is
raised only in `Blocked_DoorRotate` (0x56952) and `Blocked_Door` (0x578be).
INFERRED: it is a door event, not a pickup one.

## 10. The script side

VERIFIED, from their relocations: `Touch_Item`, `Pickup_Weapon`,
`Pickup_Health` and `G_TouchTriggers` call only `Scr_AddEntity`,
`Scr_AddUndefined` and `Scr_Notify` into script. INFERRED: no `CodeCallback_*`
is involved in a pickup.

- `"touch"` (`scr_const+0x90`). VERIFIED: item with the player and player with
  the item on every touch-pass contact (0x3fa42, 0x3fa61); item with the
  player on a use (0x4857b). INFERRED: raised before the grab test, so it
  fires for an item that is then refused.
- `"trigger"` (`scr_const+0x92`) on the item. VERIFIED: two arguments on a
  weapon (the swapped-out item or undefined, then the player; section 7), one
  (the player) on an ammo item and on health. INFERRED, off the push order:
  script reads it as `waittill("trigger", player, droppedItem)`, and it fires
  after the grab.

VERIFIED, the builtins stock scripts use: `precacheItem`,
`spawn("item_health", ...)`, `dropItem(weapon [, tag])`, `delete` and
`getcurrentweapon`. VERIFIED: `PlayerCmd_dropItem` (0x43684) calls
`BG_GetWeaponIndexForName` (0x436f2) and tests its result against 0
(0x436fd, `je` to 0x43735), calls `Drop_Weapon` (0x4372b), `BG_FindItem`
(0x43739) and `Drop_Item` (0x4ed30, called at 0x4374b), and passes the
result to `GScr_AddEntity` (0x4375b). INFERRED: a weapon name goes to
`Drop_Weapon`, any other name through `BG_FindItem` to `Drop_Item`, and the
builtin returns the dropped entity (undefined when `BG_FindItem` finds
nothing). VERIFIED, stock
scripts: every gametype calls `self dropItem(self getcurrentweapon())` on
death (dm 531, tdm 626, sd 832, re 938, bel 656). INFERRED: the death weapon
drop is script, like the health drop.

## 11. Not modelled

What vcod leaves out, each with the retail reading it skips:

- The launch flight and the `G_RunItem` settle: a drop in vcod snaps to the
  floor (`pos.trType` 0) instead of flying the `LaunchItem` trajectory and
  settling (section 8), and the tag's `apos` spin goes with it.
- Respawn: `spawnflags & 8`, `RespawnItem` and `EV_ITEM_RESPAWN` (section 7);
  no stock BSP sets the flag.
- The `CONTENTS_NODROP` free in `G_RunItem` (section 8).
- `cg_predictItems`: the choice between `G_AddPredictableEvent` and
  `G_AddEvent` (section 7); both write the same ring.
- `trigger_use` competing with items for the use key inside
  `G_GetActivateEnt`: vcod keeps `trigger_use` on the touch pass.
- The unreachable refusal lines: `GAME_PICKUP_CANTCARRYMOREAMMO` and the
  per-slot `CANT_GET` lines for pistol, grenade and smoke grenade (section 7).
- The ammo items 65 and 66 (giType 2).
- The intermission-client scoring in the drop ring (section 8): in play slot 0
  is always the one evicted.
- `Pickup_Health`'s re-rounding at a `maxHealth` other than 100 (section 6).
- The `noise` key's quiet event 147 and its sound alias (section 7).
- The player byte `ent+0x172` that both `Cmd_Activate_f` (0x4848e) and
  `G_CheckForCursorHints` (0x4f5e4) test (section 2).

## 12. The retail capture

One run on 2026-09-24 against the retail 1.1d dedicated server on
mp_carentan, with `client-probes/probe_pickup` as the gametype under
`+set probe_teleport 1 +set scr_allow_fg42 1`, wrote two fixtures in
`crates/server/tests/fixtures/items/`: `mp_carentan-dm-pickup.txt`, the
`--save-pickup` client ("the client fixture" below), and
`mp_carentan-dm-pickup-script.txt`, the server's own `games_mp.log` ("the
script fixture"). Times are server times: a `!trace`'s `serverTime` and a
`PROBE` line's `getTime()`. The client fixture records an item entity's
`index`, `clientNum`, `eFlags`, `groundEntityNum` and trajectories, and none
of its event fields. The probe's client number is 0.

### 12.1 A stock server has no fg42 on mp_carentan

VERIFIED, `default_mp.cfg` in `localized_english_pak0.pk3`: it carries
`set scr_allow_fg42 0`, the only `scr_allow_*` line set to 0
(`crates/server/src/cvars.rs` already mirrors it). `cod11-gsc-language.md`
section 9 and `cod11-gsc-object-model.md` section 17 record that
`_teams::restrictPlacedWeapons` deletes every `mpweapon_fg42` on that value.
VERIFIED, a first run of this recipe without the override (its log is not
committed): the census logged the eight panzerfausts and no fg42, and the
teleport thread logged `teleport unsupported mp_carentan`. INFERRED: a stock
`dm` round on mp_carentan has no fg42 to pick up, and the capture needs the
override.

VERIFIED, the script fixture's census: the fg42s are entities 252 at
(468, -822, 32.53) and 258 at (838, 2222, -22.83), and the panzerfausts
247..250 and 253..256. Numbering the lump's item blocks alone gives 251 and
257, one low. VERIFIED, the entity lump: an `ammo_panzerfaust_box3`
`script_model` sits just before each fg42, and
`crates/server/tests/fixtures/entities/mp_carentan-dm.txt` carries entity 257
as `eType 8`, `index 56`.

### 12.2 Standing on an unowned weapon

VERIFIED, client fixture, `[phase stand]` and `[phase aim]`: no `!server`
line, `eventSequence` 0 throughout, and entity 252 on the wire with `index 6`
and `clientNum 254`. VERIFIED, script fixture: `touch 252 <t> 0` and
`ptouch 0 <t> 252` at every 50 ms from 24050 to 29300, and no `trigger` line
for 252 before 29300. INFERRED, as sections 1 and 3 predict: the touch pass
notifies for an unowned weapon and the grab test then refuses it on a touch.

VERIFIED: the log carries one line of each per 50 ms server frame, while the
client sent a usercmd every 16 ms. INFERRED: the log cannot count notifies
inside one frame, so section 1's once-per-usercmd touch pass is neither
confirmed nor refuted here.

VERIFIED, script fixture: on every grab frame the `Weapon:` line comes first,
then the `trigger` line, then that frame's `touch` lines. INFERRED: script
line order inside a frame is not the order the engine raised the notifies in
(sections 1 and 2 read `"touch"` as raised first), so a comparison against
this file treats one frame's lines as a set.

### 12.3 The cursor hint

VERIFIED, client fixture, the `hint=` column
(`serverCursorHint:Val:String`):

- `0:0:255` from 26950 to 28000, through `stand` and the first `aim`
  snapshot, the view level and the fg42 under the feet;
- `15:0:255` from 28050 to 29250, the view pitched 87.9 down at fg42 #1:
  9 + 6;
- `0:0:255` from 29300 to 31250, fg42 #1 taken and the view still on its
  spot;
- `79:0:255` at 31300 only, the first snapshot on fg42 #2 with an fg42 held:
  73 + 6;
- `0:0:255` from 31350 to 34400, fg42 #2 taken, through the rest of `touch2`
  and all of `switch`;
- `32:0:255` from 34450 to 34700 in `use2`, aimed at a panzerfaust: 9 + 23;
- `0:0:255` from 34750 to 35650, with the dropped carbine owned by the probe
  and a panzerfaust held;
- `21:0:255` (9 + 12) from 35700 to 36200, from the snapshot the carbine's
  `clientNum` first reads 254;
- `32:0:255` from 36250 to the last trace at 42200, the carbine held again
  and the aim still on the spot where the panzerfaust dropped;
- `serverCursorHintVal` 0 and `serverCursorHintString` 255 on every trace.

INFERRED: every value matches section 2.3's encoding (unowned weapon
`9 + giTag`, owned `73 + giTag`), and an item the grab test refuses hints
nothing. Retail sends a hint for an item, so vcod sends the hint, in this
encoding.

### 12.4 The use key on an unowned weapon

VERIFIED, client fixture, `[phase use1]`: the tap's two cmds carry
`buttons=64` (st 29266 and 29283); the snapshot at 29300 carries
`!server a 6`; `events[0]` 146 with `eventParms[0]` 6 (`eventSequence` 0 to
1); `weapons[0]` 4368 to 4560, bits 6 and 7 added (`fg42_mp` and its alt
`fg42_semi_mp`); `weaponslots[0]` 0x04000C00 to 0x04060C00, which is slot 2
(`primaryb`) from 0 to 6 with slot 1 keeping the carbine's 12; clip `5:20` and
ammo `5:70` added; and entity 252 gone from the `!item` lines. `weapon` stays
12 on that frame. VERIFIED: the putaway (156) at 29350 and the raise (155) at
30000 bring `weapon` to 6, and every cmd from st 29300 on carries weapon byte
6. INFERRED: those two events are the probe's answer to `a 6` (the client
switch in section 4.3), not part of the pickup. VERIFIED, script fixture:
`Weapon: 0 fg42_mp` and `PROBE trigger 252 29300 0 undefined`.

INFERRED, all as predicted: clip 20 and reserve 70 out of `count 90`
(section 4.1), the empty `primaryb` slot (section 5), `a` on a use and the
event on the player with the item index as its parm (sections 4.3 and 7), the
undefined for the swapped item (section 7). VERIFIED: the item left the wire
on the event's own frame, as section 7 predicts. Section 7 now notes the `Weapon:` line's name.

### 12.5 Walking onto an owned weapon

VERIFIED, client fixture, `[phase touch2]`: the probe stands at
(838, 2222, -21.8) from 31300, after the script fixture's
`PROBE teleport 0 2`; the snapshot at 31350 carries `events[3]` 148
with parm 6 (`eventSequence` 3 to 4), `!server f
GAME_PICKUP_AMMO\x14WEAPON_FG42`, ammo `5:70` to `5:160` with clip `5:20`
unchanged, and entity 258 gone. VERIFIED, script fixture: `Weapon: 0 fg42_mp`,
`PROBE trigger 258 31350 0 undefined`, and one `touch 258` and one
`ptouch 0 ... 258`, both at 31350.

INFERRED, as predicted by section 4.3: the owned arm hands the item's whole
90 (clip 20 plus reserve 70) to `Add_Ammo` with `fillClip` 0, so all of it
lands in the reserve. VERIFIED, the `trigger` line above: a pickup through
the owned arm notifies `"trigger"` on the item, with undefined for the
swapped item, which section 7 had only read off control flow. VERIFIED: the
probe is on the spot at 31300 with hint 79 and no event, and the 148 arrives
at 31350. INFERRED: the teleport's `setOrigin` ran in the 31300 script frame,
after that frame's touch pass, so 31350 was the first touch pass at the new
spot.

### 12.6 The swap

VERIFIED, client fixture, `[phase use2]`: the tap's cmds at st 34716 and
34733; the snapshot at 34750 carries `!server a 23`, events 146 (parm 23) and
155 (parm 0) in slots 2 and 3 (`eventSequence` 6 to 8), `weapon` 0 with
`weaponstate` 1, `weapons[0]` 8389072 (bit 12 cleared, bit 23 set),
`weaponslots[0]` 0x04061700 (slot 1 from 12 to 23), clip `18:1` added and
clip `10:15` and ammo `10:400` gone, entity 255 gone, and a new item 170 with
`index 12`, `clientNum 0`, `eFlags 16`, `groundEntityNum 0`, `pos` type 0 at
(826, 2274, -22.8) and `apos` (0, 270, 90), both entity 255's own. The
snapshot at 34800 reads `weapon` 23. VERIFIED, script fixture:
`Weapon: 0 panzerfaust_mp`, `PROBE trigger 255 34750 0 170:mpweapon_m1carbine`,
one `touch 255 34750 0` and no `ptouch`, then
`PROBE item 170 mpweapon_m1carbine 34750`. VERIFIED: the run has no
`PROBE other` line besides `other 0 noclass 23650`. INFERRED: that line is
entity 0, the probe's own slot seen before its first spawn, so the watch list
covered the drop.

INFERRED, as predicted: the same `weaponSlot` drops the held weapon (section
5, case 2), laid where the new one lay and at rest (section 5); the drop
takes the dropper's whole reserve and clip (section 8); a placed panzerfaust
yields clip 1 and no reserve (section 4.1); the `"trigger"` notify carries the
dropped item next to the player (section 10); and a use sends `"touch"` to the
item alone (section 2). VERIFIED, two readings sections 5 to 9 did not have:
the swap frame reads `weapon` 0 with `EV_RAISE_WEAPON` (155) on the ring
beside the pickup event, and a swap's drop reads `groundEntityNum` 0 where a
placed item reads 1022.

VERIFIED: the capture reads the drops' 0 only between level times 34750 and
42200, while `crates/server/tests/fixtures/entities/mp_carentan-dm.txt`'s
header says items read 0 for the first minute after a map load; the placed
items here already read 1022 at 26950. INFERRED: the capture alone cannot
tell "a drop reads 0" from an early-level effect. VERIFIED, the listings:
`LaunchItem` has no store into `groundEntityNum` (`ent+0x7c`); the stores in
the item functions are `G_SpawnItem` (0x3ff at 0x4e801), `FinishSpawningItem`
(0x4e437) and `G_BounceItem` (0x4ea1f); `G_RunItem` compares it against 0x3ff
at 0x4eb24 and compares `pos.trType` against 0 and 8 before a `G_RunThink`
call (0x4eb42..0x4eb57). INFERRED: `G_RunItem` only thinks for a stationary
item, so after a swap's `G_SetOrigin` puts the drop at `trType` 0 nothing on
its path writes the ground entity, and the 0 is whatever the allocation left,
not a landing. Where `G_Spawn` gets that 0 I have not read, so the
early-level reading is not ruled out either.

VERIFIED, `G_BounceItem` (0x4e858): a 16-bit load of the trace's +0x28
(0x4ea1b) stored into `ent+0x7c`, `groundEntityNum` (0x4ea1f). INFERRED: a
drop that lands through `G_BounceItem` takes the entity it landed on as its
ground, the world's 1022 on a floor; a `dropItem` drop is launched at
`pos.trType` 5 (section 8) and lands that way. INFERRED: the swap's drop is
placed with `G_SetOrigin` at `trType` 0 and never reaches `G_BounceItem`, so
its 0 is consistent with the value `G_Spawn` left unwritten, though the
early-level reading above stays open. No capture of a death drop measures
its `groundEntityNum` yet.

### 12.7 The dropper's lockout

VERIFIED, client fixture: item 170's `clientNum` reads 0 on every snapshot
from 34750 to 35650 and 254 from 35700. The first `!item` reading 254 is at
probe ms 8754 and the use2 tap's first `!cmd` at ms 7761, 993 ms apart,
inside the predicted 1000 ± 50. INFERRED: the grab ran at `level.time` 34700,
since the tap's cmd time 34716 falls between the frames at 34700 and 34750,
and `DroppedItemClearOwner` ran at 35700, exactly section 8's 1000 ms.

VERIFIED, `[phase early]`: the tap's cmds at st 35217 and 35234 raise no
pickup event, no `!server` line and no `trigger` line, and the fixture carries
no `# early took` note. INFERRED: the carbine was still locked and every
placed panzerfaust in reach was owned and full, so nothing could be grabbed.

VERIFIED, `[phase late]`: the tap at st 36217; the snapshot at 36250 carries
`!server a 12`, events 146 (parm 12) and 155 in slots 0 and 1, `weapon` 0,
the carbine back in slot 1 (`weaponslots[0]` 0x04060C00, `weapons[0]` 4560)
with clip `10:15` and ammo `10:400`, clip `18:1` gone, item 170 gone, and a
new item 171 with `index 23`, `clientNum 0`, at item 170's origin and angles;
171's `clientNum` reads 254 from 37200. VERIFIED, script fixture:
`Weapon: 0 m1carbine_mp` and `PROBE trigger 170 36250 0 171:mpweapon_panzerfaust`.
INFERRED: the drop kept the 400 and 15 it was dropped with and handed them
back whole (sections 4.3 and 8).

### 12.8 Where the event rides

VERIFIED: all four pickup events in the run (146 at 29300, 34750 and 36250,
148 at 31350) arrive
in the playerstate's `events` ring with the item's `index` as the parm.
VERIFIED: each taken item left the wire on the snapshot that carries its
event, and the client fixture records no item event field. INFERRED: an event
on the item's own state is not observable in this capture, so section 7's "no
event lands on the item entity" stays INFERRED.

### 12.9 The watcher threads

VERIFIED: the server ran its full 150 s with no `script runtime error` in its
console, and the watchers on item 170, spawned mid-run, logged its `touch`
and `trigger` lines. INFERRED: a watcher's `endon("death")` on an item that is
taken or freed ends or parks without an error.

## 13. As implemented

`crates/server/tests/pickup_ab.rs` replays section 12's capture against
vcod: the probe as the gametype under the recipe's `probe_teleport 1` and
`scr_allow_fg42 1`, the capture's cmds on retail's clock, the view each
retail snapshot reports, and a diff of the weapon, the cursor hint, the
item and weapon events and the origin per snapshot, the inventory, the
pickup events, the commands and the items per phase, and the script log's
`Weapon:`, `trigger`, `touch` and `ptouch` lines per phase as a set. The
rulings it needed are below.

### 13.1 An item notify's waiters run before the frame's waits

VERIFIED, `probe_pickup.gsc`: `watch_trigger` is the only writer of
`level.probe_taken`, and `try_teleport` runs in the `watch_teleports` loop,
started in `main` before any item watcher, and calls `wait 2` between reading
the flag and the second `setOrigin`. VERIFIED, the client fixture: the second
teleport is on the 31300 snapshot, 2000 ms after the 29300 trigger notify.
INFERRED:
the loop, parked on `wait 0.05`, read the flag in the 29300 frame, so the
`"trigger"` waiter ran before that frame's `wait`s came due. VERIFIED, the
script fixture: the census loop's `PROBE item 170` line, from a thread also
started before the watchers, follows the 34750 `trigger` and `touch` lines.
INFERRED: the same order.

vcod delivers the item pass's notifies at the top of the script frame, on
the frame's clock, and on a frame that has any runs their waiters there,
before the entity thinks, the movers and the frame's `wait` pass
(`ScriptRuntime::run_frame`); any other thread already runnable at that
point runs there with them. The thread pass had run them in thread age, and
the gate read the teleport at 31350 and the ammo pickup at 31400. Where
retail's drain sits relative to the entity thinks is not measured. INFERRED,
unmeasured: where retail drains a trigger's notifies relative to the `wait`
pass; vcod keeps them where they were so the trigger and S&D gates keep
their baselines.

### 13.2 The swap disarm lands a frame late

VERIFIED, sections 12.6 and 12.7: each swap snapshot (34750 and 36250) reads
`weapon` 0 with 155 beside the 146, and the next reads the new weapon.
VERIFIED, the gate on vcod: the 146 is on the same snapshot with `weapon`
still the old one, and the 155 is on the next beside the new weapon; no
snapshot reads `weapon` 0. INFERRED: vcod runs every cmd's pmove before the
deferred touch pass, so the disarm that retail's tap frame runs on the unheld
`ps.weapon` (section 5) runs on the next tick's first cmd, and that tick's
later cmd, which already carries the new weapon byte, raises the new weapon
before the snapshot goes out.

Ruling: accepted, not restructured. Moving the touch pass inside each cmd's
move is a tick-order redesign for a one-frame raise delay; a retail client
sees the raise one snapshot late and no `weapon` 0 frame. The gate's `GAPS`
row `swap disarm one frame late` excuses exactly this shape, one frame, on
`weapon` and 155 only.

### 13.3 The cursor hint on the locked drop

VERIFIED, the gate on vcod: the hint reads 32 from 36250 to 37150, while
item 171 is locked to its dropper, and the values match section 12.3
snapshot for snapshot. VERIFIED, temporary instrumentation of
`cursor_hint_pass` during a gate run, since reverted: the 32 on that stretch
comes from the placed panzerfaust 256 at (821, 2274), 5 units from 171, and
from 37200 the hint comes from 171. INFERRED: retail's 32 on that
stretch is a placed panzerfaust too; the capture does not record which
entity a hint came from.
