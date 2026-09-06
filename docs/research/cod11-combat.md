# CoD 1.1 combat: the weapon state machine and the damage path

What happens between a held fire button and a dead player: the pmove weapon
machine that owns `weaponstate`, `weaponTime`, `weaponDelay` and `weapAnim`;
the bullet trace and its spread; the hit-location table; `G_Damage` and the
script callback that actually subtracts health; `player_die` and the corpse
clone; and the damage-feedback fields the client reads to shake the view.

Evidence rules as everywhere in this directory, and this document carries no
document-level default because its provenance is not uniform. Three sources
are mixed and each claim says which one it rests on:

- `game.mp.i386.so`, the 1.1d Linux dedicated server's MP game module, which
  carries a full dynamic symbol table. Addresses are module-relative (image
  base 0), the same convention `cod11-gsc-object-model.md` uses. It is
  position-independent, so a plain `objdump -d` hides every call and cvar
  target; `python3 tools/re/annotate_func.py <elf> <symbol>` resolves them.
  Pointers stored in `.data` read as 0 in the file unless `.rel.data` is
  resolved, and `.data`'s virtual address runs `0x1000` above its file offset
  in this module (`readelf -S` gives it `Addr 0x7b3a0 / Off 0x7a3a0`), which is
  the second way a raw read of a table comes back wrong. **Every address in
  this document is a virtual address**, including every `.data` table, so
  dumping one out of the file means subtracting `0x1000` first.
- `cgame_mp_x86.dll` 1.1, the MP client game, decompiled with Ghidra. Pmove is
  shared code, so the whole `PM_Weapon` family is in it, compiled from the
  same sources. Its image base is `0x30000000`. Where a claim was read there
  it says so, and where the `.so` confirms it the `.so` address is given too.
- `cod_lnxded`, the 1.1d Linux dedicated server's **engine** executable. It
  is stripped, so every engine function named below is my name for it and is
  marked as such the first time it appears. Its first LOAD segment sits at
  `0x08048000` with file offset 0, so a virtual address is the file offset
  plus `0x8048000`.
- `private/reference/CoDExtended/src/shared.h`, the community's struct
  headers. Only three things below rest on it, each labelled, and each is
  cross-checked against an offset the binary uses.

Most of what follows describes a run of instructions, and each such passage is
written as a list introduced by a pair of labelled sentences rather than by a
label per item. That is because each carries exactly two kinds of claim and
they need opposite labels: the operands, offsets, immediates and call targets
are read out of the instructions, and the ordering and the conditions are read
off the branches.

The measured side of the weapon machine, taken off two retail captures rather
than out of a binary, is in `docs/research/player-model-anim-system.md`, "The
weapon channel: what writes `torsoAnim`". Nothing here contradicts it; where
this document sharpens one of its INFERRED readings it says which.

The `EV_*` numbers below are the table in `docs/research/cod11-events-and-fx.md`
section 1, and the bullet-impact temp entity's client-side handling is that
document's `EV_BULLET_HIT_*` section. Playerstate offsets and the `ammo[]` /
`ammoclip[]` indexing are `docs/protocol-1.1.md`, "PlayerState delta".

---

## 0. The weapon-def struct, and where its fields are

Every timer and flag the weapon machine reads is a field of the parsed weapon
file. VERIFIED: the field table is at VA `0x7C9A0` in `game.mp.i386.so` (file
offset `0x7B9A0`), records of 12 bytes `{ char *name; int offset; int type; }`,
and the record at VA `0x7D534` holds `0xFFFFFFFF`, `0xFFFFFFFF`, `0`, which
makes 247 records ahead of it. INFERRED: that record is the terminator, since
its name word is not a `.rodata` offset and every record before it is.
VERIFIED: the offsets are byte offsets into the weapon def, and the type codes
seen in it are 0 string, 4 int, 5 bool, 6 float, 7 time, 8/9/0xa/0xb/0xc/0xd
enum. VERIFIED: `weaponTime` and `weaponDelay` are decremented by `pml.msec`,
and the captured `weaponstate` runs match the weapon file's seconds times 1000
(`player-model-anim-system.md`). INFERRED: a type-7 field therefore holds
milliseconds, converted from the seconds the weapon file spells.

VERIFIED: `weaponDefs`, the array the game indexes by weapon number, is the
pointer at `0x7c91c`; `BG_SetupWeaponInfo` (`0x36674`) allocates it 64 entries
wide at `0x366b1`. VERIFIED: `BG_GetInfoForWeapon` (`0x3ac68`) is the accessor,
and `weaponDef+0x4` is the weapon's name string, which `Scr_PlayerDamage`
passes to script (`0x5ca84`).

The fields the combat path reads, all VERIFIED out of that table:

| offset | field | offset | field |
|---|---|---|---|
| `0x70` | `weaponType` (enum) | `0x1FC` | `reloadEndTime` |
| `0x74` | `weaponClass` (enum) | `0x200` | `dropTime` |
| `0x1A0` | ammo index (derived) | `0x204` | `raiseTime` |
| `0x1A8` | clip index (derived) | `0x208` | `altDropTime` |
| `0x1AC` | `maxAmmo` | `0x20C` | `altRaiseTime` |
| `0x1B0` | `clipSize` | `0x210` | `fuseTime` |
| `0x1C0` | `damage` | `0x23C` | `hipSpreadStandMin` |
| `0x1C4` | `meleeDamage` | `0x240` | `hipSpreadDuckedMin` |
| `0x1CC` | `fireDelay` | `0x244` | `hipSpreadProneMin` |
| `0x1D0` | `meleeDelay` | `0x248` | `hipSpreadMax` |
| `0x1D4` | `fireTime` | `0x2BC` | `twoHanded` |
| `0x1D8` | `rechamberTime` | `0x2C0` | `rifleBullet` |
| `0x1DC` | `rechamberBoltTime` | `0x2C4` | `semiAuto` |
| `0x1E0` | `holdFireTime` | `0x2C8` | `boltAction` |
| `0x1E4` | `meleeTime` | `0x2CC` | `aimDownSight` |
| `0x1E8` | `reloadTime` | `0x2D4` | `clipOnly` |
| `0x1EC` | `reloadEmptyTime` | `0x2DC` | `adsFire` |
| `0x1F0` | `reloadAddTime` | `0x2E8` | `noPartialReload` |
| `0x1F4` | `reloadStartTime` | `0x2EC` | `segmentedReload` |
| `0x1F8` | `reloadStartAddTime` | `0x2F0` | `reloadAmmoAdd` |
| `0x38C` | `adsSpread` | `0x2F8` | `altWeapon` (name) |
| `0x24C` | `hipSpreadDecayRate` | `0x268` | `adsTransInTime` |
| `0x250` | `hipSpreadFireAdd` | `0x26C` | `adsTransOutTime` |
| `0x254` | `hipSpreadTurnAdd` | `0x3D4` | `adsReloadTransTime` |
| `0x258` | `hipSpreadMoveAdd` | `0x25C` | `hipSpreadDuckedDecay` |
| `0x260` | `hipSpreadProneDecay` | | |

VERIFIED, `BG_SetupWeaponInfo` `0x36950`-`0x369a1`: `weapDef+0x414` and
`weapDef+0x418` are not in the field table but derived at load, as
`adsTransInTime > 0 ? 1.0 / adsTransInTime : 0.0033333` (`.rodata 0x724cc`)
and `adsTransOutTime > 0 ? 1.0 / adsTransOutTime : 0.002` (`.rodata 0x724d0`),
with both times read as the type-7 integer milliseconds they parse into. So
the two are **reciprocal milliseconds** and the fallbacks are 300 ms in and
500 ms out. VERIFIED, `0x38b23`: the fire-timer setter computes
`weaponDelay = (int)((1.0 - ps->fWeaponPosFrac) * (1.0 / weapDef[0x414]))`
for an `adsFire` weapon, which is a millisecond count only if `0x414` is
per-millisecond.

VERIFIED: `ammoName` is at 412 and the index at 416 (`0x1A0`), `clipName` at
420 and the index at 424 (`0x1A8`), `altWeapon` at 760 and the resolved alt
weapon number at 764 (`0x2FC`); the three name fields are in the table and the
three resolved integers are not, and each sits one slot past its name. The
first two are the `weapDef+0x1A0` / `weapDef+0x1A8` that
`docs/protocol-1.1.md` already pins as the `ammo[]` and `ammoclip[]` indexes.

VERIFIED, off the shipped `weapons/mp/*` in `pak0.pk3`, the two weapons the
committed combat fixtures used:

| field | `m1carbine_mp` | `mosin_nagant_mp` |
|---|---|---|
| `damage` | 45 | 120 |
| `meleeDamage` | 50 | 150 |
| `semiAuto` | 1 | 1 |
| `boltAction` | 0 | 1 |
| `rifleBullet` | 1 | 0 |
| `clipSize` | 15 | 5 |
| `fireTime` | 0.135 | 0.33 |
| `rechamberTime` | 0.1 | 1 |
| `rechamberBoltTime` | 0 | 0.4 |
| `reloadTime` | 2.65 | 2.4 |
| `reloadEmptyTime` | 3.3 | 2.4 |
| `noPartialReload` | 0 | 0 |
| `segmentedReload` | 0 | 0 |
| `hipSpreadStandMin` | 1.5 | 2 |
| `hipSpreadDuckedMin` | 1.3 | 1.6 |
| `hipSpreadProneMin` | 1.1 | 1.3 |
| `hipSpreadMax` | 5 | 5.5 |
| `adsSpread` | 0.4 | 0.1 |
| `hipSpreadFireAdd` | 0.7 | 1 |
| `hipSpreadDecayRate` | 4 | 3.25 |
| `hipSpreadTurnAdd` | 0 | 0 |
| `hipSpreadMoveAdd` | 8 | 8 |
| `hipSpreadDuckedDecay` | 1 | 1.5 |
| `hipSpreadProneDecay` | 1.1 | 1.2 |
| `aimDownSight` | 1 | 1 |
| `adsTransInTime` | 0.3 | 0.3 |
| `adsTransOutTime` | 0.4 | 0.6 |
| `adsReloadTransTime` | 0.6 | 0.6 |
| `adsFire` | absent | absent |

VERIFIED, over the 80 weapon files in the stock paks: `hipSpreadTurnAdd` is 0
in 48 of them, absent in 27 and 0.8 in five, of which `bar_mp` and
`bar_slow_mp` are MP; `adsFire` is set only on the two panzerfaust files, one
of them `panzerfaust_mp`; `aimDownSight` is 1 in 52, absent in the 27 with no
spread block at all (grenades, mounted MGs) and 0 in one SP file. INFERRED: an
absent key reads as 0, since the parsed struct is zeroed and the field table
only writes keys the file spells.

---

## 1. `PM_Weapon`: the states and what moves between them

`PM_Weapon` is `0x390e0` in `game.mp.i386.so` and `0x30011ab0` in
`cgame_mp_x86.dll`. VERIFIED: it is 0x270 bytes in the `.so` and is a
dispatcher; every state's work is in one of eleven helpers around it. The two
builds inline different helpers, so a helper is named below by both addresses
where both exist.

| what it does | `.so` | dll |
|---|---|---|
| `PM_Weapon` | `0x390e0` | `0x30011ab0` |
| advance `weaponTime` / `weaponDelay` | `0x387c0` | `0x30011210` |
| begin a weapon change if the usercmd asks | `0x38918` | `0x300112e0` |
| reload check (auto and by key) | `0x384d8` | `0x30010eb0` |
| melee check | `0x38f68` | `0x30011930` |
| rechamber check (bolt action) | `0x375d0` | `0x300100a0` |
| reload state machine | `0x3820c` | `0x30010c50` |
| melee finish | `0x38eb4` | `0x300118d0` |
| finish a weapon change | `0x37d84` | `0x300107c0` |
| finish a raise | inlined at `0x392b4` | `0x30010a30` |
| release the trigger | inlined at `0x392f0` | `0x300113d0` |
| fire | `0x38d28` | `0x30011770` |
| start the shot's timers and state | `0x38a44` | `0x30011410` |
| decide the shot happens | `0x38bbc` | `0x300115d0` |
| pick the shot's `weapAnim` | `0x38c80` | `0x300116a0` |
| set `weapAnim` | not separately | `0x3000ffe0` |
| set `weapAnim` only if the index changed | not separately | `0x30010010` |
| back to idle after a rechamber | not separately | `0x30010050` |
| begin a weapon change | not separately | `0x30010570` |
| begin a reload | not separately | `0x30010440` |
| pick the reload's anim, time and event | not separately | `0x30010330` |
| set the reload's `weaponDelay` | not separately | `0x30010250` |
| add the reloaded rounds | not separately | `0x30010b00` |
| can the weapon reload | not separately | `0x30010a80` |
| finish a melee | not separately | `0x30011850` |

### 1.1 `weaponstate`, twelve values with names out of the binary

VERIFIED: the names come from the debug printer at `0x30011d00` in
`cgame_mp_x86.dll`, a `switch` on `ps->weaponstate` whose twelve cases each
call the console printer with a literal string, preceded by the literal
`"WEAP_STATE -- "`.

| value | name | value | name |
|---|---|---|---|
| 0 | `WEAPON_READY` | 6 | `WEAPON_RELOADING_INTERUPT` |
| 1 | `WEAPON_RAISING` | 7 | `WEAPON_RELOAD_START` |
| 2 | `WEAPON_DROPPING` | 8 | `WEAPON_RELOAD_START_INTERUPT` |
| 3 | `WEAPON_FIRING` | 9 | `WEAPON_RELOAD_END` |
| 4 | `WEAPON_RECHAMBERING` | 10 | `WEAPON_MELEE_WINDUP` |
| 5 | `WEAPON_RELOADING` | 11 | `WEAPON_MELEE_RELAX` |

Anything else prints `"UNKNOWN"`; VERIFIED, that is the `default` arm.

VERIFIED: `weaponstate` is `ps+0xB4`, which is the offset `PM_Weapon` reads at
`0x392ae` and every helper writes. This resolves the INFERRED state numbers in
`player-model-anim-system.md` to an enum: 0, 2, 3, 4 and 5 there are
`WEAPON_READY`, `WEAPON_DROPPING`, `WEAPON_FIRING`, `WEAPON_RECHAMBERING` and
`WEAPON_RELOADING`. VERIFIED: nothing in this module writes 1 except the
weapon-change finish, and 6 to 11 are the interrupted reload, the segmented
reload's start and end, and melee.

### 1.2 `weapAnim`, seventeen values, and the 512 toggle

VERIFIED: the names come from the second debug printer, `0x30011e50` in
`cgame_mp_x86.dll`, which masks `ps->weapAnim` with `0xFFFFFDFF` and switches
on the result.

| value | name | value | name |
|---|---|---|---|
| 0 | `WEAP_IDLE` | 9 | `WEAP_DROP` |
| 2 | `WEAP_ATTACK` | 10 | `WEAP_RAISE` |
| 3 | `WEAP_ATTACK_LASTSHOT` | 11 | `WEAP_RELOAD` |
| 4 | `WEAP_RECHAMBER` | 12 | `WEAP_RELOAD_EMPTY` |
| 5 | `WEAP_ADS_ATTACK` | 13 | `WEAP_RELOAD_START` |
| 6 | `WEAP_ADS_ATTACK_LASTSHOT` | 14 | `WEAP_RELOAD_END` |
| 7 | `WEAP_ADS_RECHAMBER` | 15 | `WEAP_ALTSWITCHFROM` |
| 8 | `WEAP_MELEE_ATTACK` | 16 | `WEAP_ALTSWITCHTO` |

VERIFIED: index 1 falls into the `default` arm, so it has no name in the
binary. VERIFIED: the mask `0xFFFFFDFF` clears bit `0x200`, so bit 512 is not
part of the index, which is the same restart toggle the anim channels carry
(`player-model-anim-system.md`, "The restart toggle").

VERIFIED, `0x3000ffe0` in the dll: the setter is
`weapAnim = (~weapAnim & 0x200) | anim`, and the function compares
`ps->pm_type` against 6 and `pm->cmd.weapon` against 0. INFERRED: the write
is on the arm where `pm_type < 6` and `cmd.weapon != 0` both hold. INFERRED:
because the write always inverts bit `0x200`, every call flips the toggle
whether or not the index changed. VERIFIED,
`0x30010010`: a second setter takes the same argument, compares
`weapAnim & 0xFFFFFDFF` against it, and has a return path that writes nothing.
INFERRED: that path is the one an equal comparison takes. INFERRED: that
second form is what holds an index steady across frames without restarting the
clip, and the first form is what a repeated shot goes through, which is why the
captured sustained fire reads 253, 765, 253, 765.

VERIFIED, `.so` `0x38c80` (dll `0x300116a0`): the function compares
`ps->fWeaponPosFrac` (`ps+0xB8`) against the float at `.rodata 0x7254c`, which
is `0.75`, compares `ps->ammoclip[clipIndex]` against 0, and loads the
immediates 2, 3, 5 and 6 into `ebx` on four separate arms. INFERRED: the 5/6
pair is above the threshold and the 2/3 pair at or below it, and the lower of
each pair is the arm a non-zero `ammoclip` takes. INFERRED: so a hip shot writes
`WEAP_ATTACK`, a hip shot that empties the clip writes
`WEAP_ATTACK_LASTSHOT`, and the aimed forms are the same two shifted by three.

### 1.3 The two timers

VERIFIED: `ps->weaponTime` is `ps+0x2C` and `ps->weaponDelay` is `ps+0x30`,
the offsets `docs/protocol-1.1.md` already carries. VERIFIED, `.so` `0x387c0`
(dll `0x30011210`): each is compared against 0, decremented by `pml.msec`,
compared against 1, and has a store of the immediate 0. INFERRED: so a
non-zero timer loses `pml.msec` a frame and is clamped to 0 once the
subtraction takes it below 1.
VERIFIED: its two return paths load the immediates 1 and 0. INFERRED: it
returns 1 when `weaponDelay` reached 0 on this frame and 0 otherwise, read off
the compare that selects them. INFERRED: that return value is the single
boolean `PM_Weapon` threads into the melee check, the rechamber check, the
reload machine, the trigger-release check and the fire function, so "the delay
expired this frame" is the one edge the whole machine is written around.

Which weapon-file time each state runs on, all VERIFIED off the store that
sets `weaponTime` or `weaponDelay` in the dll:

| state | `weaponTime` from | `weaponDelay` from | address |
|---|---|---|---|
| `WEAPON_RAISING` | `raiseTime` `0x204`, or `altRaiseTime` `0x20C` on an alt switch | -- | `0x300107c0` |
| `WEAPON_DROPPING` | `dropTime` `0x200`, or `altDropTime` `0x208` on an alt switch | -- | `0x30010570` |
| `WEAPON_FIRING` | `fireTime` `0x1D4` | `fireDelay` `0x1CC` | `0x30011410` |
| `WEAPON_RECHAMBERING` | `rechamberTime` `0x1D8` | `rechamberBoltTime` `0x1DC`, or 1 | `0x300100a0` |
| `WEAPON_RELOADING` | `reloadTime` `0x1E8` | `reloadAddTime` `0x1F0` | `0x30010330` |
| `WEAPON_RELOADING`, empty clip | `reloadEmptyTime` `0x1EC` | `reloadAddTime` `0x1F0` | `0x30010330` |
| `WEAPON_RELOAD_START` | `reloadStartTime` `0x1F4` | `reloadStartAddTime` `0x1F8` | `0x30010440` |
| `WEAPON_RELOAD_END` | `reloadEndTime` `0x1FC` | -- | `0x30010c50` |
| `WEAPON_MELEE_WINDUP` | `meleeTime` `0x1E4` | `meleeDelay` `0x1D0` | `0x30011930` |

VERIFIED: the rechamber's `weaponDelay` has two sources, `rechamberBoltTime`
and the literal 1, and `rechamberBoltTime` is tested against 0 and against
`rechamberTime`. INFERRED: the literal is taken when that field is 0 or is not
less than `rechamberTime`. VERIFIED, dll `0x30010250`: the reload's
`weaponDelay` is the smaller of the add-time and the state's own `weaponTime`,
and the function also has a store of the literal 1 and a path that leaves the
field at 0. INFERRED: the 1 is taken when the weapon is bolt-action with its
rechamber bit set and `rechamberBoltTime` is 0 or not smaller, and the field is
left at 0 when the add-time is 0. VERIFIED: `player-model-anim-system.md`
matched the mosin's `weaponstate` 5 run to `reloadTime` 2.4; both `reloadTime`
and `reloadEmptyTime` are 2.4 on that weapon. INFERRED: the branch that ran
was the empty one, since the state opened after event 161 and event 152.

### 1.4 The semi-automatic latch is `weaponTime`, not a flag

VERIFIED, `.so` `0x387c0` (dll `0x30011210`): the function compares
`weaponTime` against 1, tests four more things -- `weaponDef->semiAuto`
(`0x2C4`) against 0, `pm->cmd.buttons & 1`, `ps->weapon` against
`pm->cmd.weapon`, and `ps->ammoclip[weaponDef->clipIndex]` against 0 -- and
holds two stores into `weaponTime`, of the immediates 1 and 0. INFERRED: the
four extra tests are reached once `weaponTime` has been decremented below 1,
and the 1 is stored when all four hold and the 0 otherwise, read off the
branches.

INFERRED: that is the whole semi-automatic edge. Nothing latches the button in
`pm_flags` or in a playerstate field; the weapon simply never reaches
`weaponTime == 0` while the trigger stays down, and the fire path is gated on
`weaponTime` having reached 0. INFERRED: releasing the trigger lets the next
frame take the `else` and store 0, after which the weapon can fire again. This
is what `--save-combat` had to work around by tapping the bit rather than
holding it (`AGENTS.md`, "Netcode debugging").

VERIFIED, same block: it compares `weaponstate` against 4, 3, 10 and 11, and
calls the "return to idle" helper (dll `0x30010050`) on one arm and the
index-preserving `weapAnim` setter on another. INFERRED: 4 takes the first and
3, 10 and 11 the second, on the frame `weaponTime` is pinned at 1. INFERRED:
that is what keeps a held semi-automatic showing its idle pose rather than a
stuck attack anim.

VERIFIED: the "return to idle" arm writes `weaponstate` and not `weapAnim`;
pavlov's rechamber ends with `weapAnim` unchanged, and its shot's state 3 ends
on the same frame the rechamber opens (`player-model-anim-system.md`, "What
`weapAnim` is not written by"). INFERRED: so the state 3 exit is on this frame
too and not in the trigger-release check of 1.6, which is what lets the
rechamber open without waiting a frame for a ready weapon.

### 1.5 Firing

The fire path is `.so` `0x38d28` (dll `0x30011770`). VERIFIED: the offsets,
immediates, weapon-def fields, event numbers and call targets named in the
list below, each read out of the instruction at the address given. INFERRED:
the numbering, and every "when", "unless" and "otherwise" in it, which are
branch conditions.

1. dll `0x30011410` sets the timers, the animscript event and `weaponstate`.
   For `weaponType != 1` it stores `fireDelay` into `weaponDelay` and
   `fireTime` into `weaponTime`, overwrites `weaponDelay` from a helper when
   `weaponDef->adsFire` (`0x2DC`) is set, sets the weapon's bit in
   `ps->weaponrechamber` (`ps+0x31C`) when `weaponDef->boltAction` (`0x2C8`)
   is set, then stores 3 into `weaponstate` and ORs `0x400` into `pm_flags`
   when `pm_flags & 1`.
2. dll `0x300115d0` decides whether the shot happens. It returns 1 when
   `ps->ammoclip[clipIndex] >= 1`. Otherwise it calls the reload starter when
   `ps->ammo[ammoIndex] > 0`, raises event `0x95` (`EV_NOAMMO`, 149) when it
   is 0 and the weapon is not a grenade, clears `weapAnim` to `WEAP_IDLE` with
   the toggle flipped, adds the literal 500 to `weaponTime`, and returns 0.
3. The shot is skipped entirely when `ps->weaponDelay != 0`.
4. `ps->ammoclip[clipIndex]` is decremented, unless it holds `-1` or
   `ps->eFlags & 0xC000` is set.
5. `weaponType == 1` reloads `weaponTime` from `fireTime` a second time at
   `0x38da3`.
6. `weapAnim` is set from the pair described in 1.2.
7. The event is `0xA1` (`EV_FIRE_WEAPON_LASTSHOT`, 161) when the decremented
   `ammoclip` is 0 and `0x9F` (`EV_FIRE_WEAPON`, 159) otherwise.
8. At `.so` `0x38dfb`, `aimSpreadScale` (`ps+0x3D8`) grows by
   `weaponDef->hipSpreadFireAdd (0x250) * 255.0` and is clamped to 255.0, and
   the whole step is skipped when `ps->fWeaponPosFrac == 1.0`. The 255.0 is
   `.rodata 0x72550`.
9. When `weaponDef->clipOnly` (`0x2D4`) is set and both the clip and the
   reserve are now 0, `BG_TakePlayerWeapon` runs and event `0x95`
   (`EV_NOAMMO`) is raised.

INFERRED: a dry trigger therefore costs half a second on top of whatever
`weaponTime` already held. INFERRED: step 7's condition is what
`player-model-anim-system.md` observed on the mosin's fifth round.

VERIFIED: `EV_EMPTYCLIP` (150) is never pushed as an event argument anywhere in
`game.mp.i386.so`; the only two `push 0x95` sites are the two `EV_NOAMMO` ones
above, and the module's only `push 0x96` (`0x46e51`) is a buffer size inside
`G_Say`.

VERIFIED: `0x300107c0` stores `0x437F0000` (255.0) into `aimSpreadScale` and
`P_DamageFeedback` clamps it to the same value. INFERRED: it is a 0..255
float.

### 1.6 Releasing the trigger

VERIFIED: the check is dll `0x300113d0`, inlined in the `.so` at `0x392f0`.
VERIFIED: the offsets, immediates, weapon-def fields, event numbers and
call targets named in the list below. INFERRED: the ordering, and every
"when", "unless", "otherwise" and "skipped" in it, which are branch
conditions.

- When the attack bit is clear and `weaponDelay` did not expire this frame,
  `weaponstate` 3 goes through the index-preserving `weapAnim` setter,
  `weaponstate` is set to 0, and the fire function is not called.
- When either the attack bit is set or the delay expired, the fire function
  runs.

### 1.7 Reloading

VERIFIED: the offsets, immediates, weapon-def fields, event numbers and
call targets named in the list below. INFERRED: the ordering, and every
"when", "unless", "otherwise" and "skipped" in it, which are branch
conditions.

- dll `0x30010a80`, the "can this weapon reload" test: `ps->ammo[ammoIndex]`
  must be non-zero and `ps->ammoclip[clipIndex]` must be below the clip size.
  When `weaponDef->noPartialReload` (`0x2E8`) is set, a further test runs
  against `weaponDef->reloadAmmoAdd` (`0x2F0`): with `reloadAmmoAdd` 0 or not
  below the clip size the reload is refused unless the clip is empty, and
  otherwise it is refused unless at least `reloadAmmoAdd` rounds are missing.
- dll `0x30010eb0`, the caller: the whole reload check is skipped for
  `weaponstate` 1, 2, 5, 6, 7, 8, 9, 10 and 11.
- Same function: a reload begins when `pm->cmd.wbuttons & 8` and the test
  above passes.
- Same function: a reload also begins with no key at all when `ammoclip` is 0,
  `ammo` is non-zero, `weaponstate != 3`, and either `pm_flags & 1` is clear
  or both `cmd.forwardmove` and `cmd.rightmove` are 0.
- Same function: when `weaponDef->segmentedReload` (`0x2EC`) is set, the
  attack bit turns `weaponstate` 7 into 8 and 5 into 6.
- dll `0x30010440`, beginning a reload: with `segmentedReload` set and
  `reloadStartTime` non-zero it writes `WEAP_RELOAD_START`, `weaponTime` from
  `reloadStartTime`, `weaponstate` 7, and event `0x99` (`EV_RELOAD_START`,
  153).
- dll `0x30010330`, otherwise: with `ammoclip` 0 and `weaponType` 0 it writes
  `WEAP_RELOAD_EMPTY`, `weaponTime` from `reloadEmptyTime` and event `0x98`
  (`EV_RELOAD_FROM_EMPTY`, 152); otherwise `WEAP_RELOAD`, `weaponTime` from
  `reloadTime` and event `0x97` (`EV_RELOAD`, 151). `weaponstate` becomes 6
  when it was 8 and 5 otherwise.
- dll `0x30010b00`, on the frame the reload's `weaponDelay` expires: with
  `weaponDef->boltAction` set and the weapon's `ps->weaponrechamber` bit set
  it clears that bit and raises event `0xA3` (`EV_EJECT_BRASS`, 163), then
  re-arms `weaponDelay` for the next segment from the same add-time
  arithmetic.
- dll `0x30010c50`, ending one: from state 5 or 6 the weapon's
  `ps->weaponrechamber` bit is cleared, and with `segmentedReload` clear the
  state goes to 0 with `WEAP_IDLE`. With `segmentedReload` set and the state
  not 6 and the reload test still passing, another segment starts; otherwise,
  with `reloadEndTime` non-zero, the state goes to 9 with `WEAP_RELOAD_END`,
  `weaponTime` from `reloadEndTime` and event `0x9A` (`EV_RELOAD_END`, 154).
  State 9 goes to 0 with `WEAP_IDLE`.

INFERRED: the keyless clause is the automatic reload on a dry clip, and it
refuses to run for a prone player who is moving. INFERRED: the run of states
the check is skipped for leaves 0, 3 and 4 as the only ones it acts in.
INFERRED: the `EV_EJECT_BRASS` in the middle of a reload is where a
bolt-action's spent case leaves the gun when the player reloads instead of
working the bolt.

**What the reload key does to a partial clip.** VERIFIED: both fixture weapons
ship `noPartialReload` 0. INFERRED: with that field clear the test passes on
any non-full clip with reserve left, so the key starts an ordinary reload with
event `EV_RELOAD` (151). VERIFIED: that is what the retaken combat captures
read, `weaponstate` 5 with event 151 for `reloadTime` on both maps
(`player-model-anim-system.md`, "The reload key reloads a partly-full clip").
VERIFIED: the earlier pair read `weaponstate` 2 and event 156 at that step
instead, and the one thing that changed between the runs is the probe's
`cmd.weapon`, 0 before and the held weapon after. That is the cause this
section's own reading named as likeliest, now measured.

### 1.8 The weapon-switch path from the usercmd `weapon` byte

VERIFIED: the offsets, immediates, weapon-def fields, event numbers and
call targets named in the list below. INFERRED: the ordering, and every
"when", "unless", "otherwise" and "skipped" in it, which are branch
conditions.

- dll `0x300112e0`, the check: it is skipped while `weaponTime != 0` unless
  `weaponstate` is one of 4, 5, 6, 7, 8, 9; and it returns outright for
  `weaponstate` 3, 10 or 11, or when `weaponDelay != 0`.
- Same function: with `pm_flags & 0x10` set and `ps->weapon` non-zero it
  begins a change to weapon 0.
- Same function: otherwise, when `pm->cmd.weapon` differs from `ps->weapon`,
  it begins a change to `cmd.weapon` provided `cmd.weapon` is 0 or the
  matching bit is set in `ps->weapons` (`ps+0x30C`), and it skips that when
  `pm_flags & 0x4000` is set with a non-zero current weapon.
- Same function: it also begins a change to 0 when the player no longer owns
  `ps->weapon`.
- dll `0x30010570`, `.so` `0x37a9c`, the putaway: the target must be in
  `0..numWeapons` and owned, and `weaponstate` must not already be 2.
- Same function: it clears `weaponDelay`, and when the current weapon is 0 or
  unowned or `ps->grenadeTimeLeft > 0` it clears `weaponTime`, sets
  `weaponstate` 2, clears `grenadeTimeLeft` and ORs `0x400` into `pm_flags`
  when `pm_flags & 1`.
- Same function, the ordinary path: it raises event `0x9D` (`EV_WEAPON_ALT`,
  157) with `WEAP_ALTSWITCHFROM` when the target is the current weapon's
  `altWeapon`, and otherwise event `0x9C` (`EV_PUTAWAY_WEAPON`, 156) with
  `WEAP_DROP`, and then sets `weaponstate` 2 and `weaponTime` from `dropTime`
  (or `altDropTime` on the alt path). The `EV_PUTAWAY_WEAPON` is suppressed
  when `weaponDef->clipOnly` is set and the clip is empty.
- dll `0x300107c0`, the pickup half, which runs only in state 2: the new
  weapon is `cmd.weapon`, forced to 0 when `pm_flags & 0x10` is set, when the
  player does not own it, or when it exceeds the weapon count.
- Same function: it writes `ps->weapon`, refreshes the cached weapon def, and
  when old and new are equal sets `weaponstate` 0 with `WEAP_IDLE`.
- Same function: otherwise it sets `weaponstate` 1, raises event `0x9B`
  (`EV_RAISE_WEAPON`, 155) unless the new weapon is the old one's alt, sets
  `weaponTime` from `raiseTime` (or `altRaiseTime`), sets `weapAnim` to
  `WEAP_RAISE` (or `WEAP_ALTSWITCHTO`), and sets `aimSpreadScale` to 255.0.
- dll `0x30010a30`, inlined in the `.so` at `0x392b4`: `weaponstate` 1 is
  cleared to 0 and `weapAnim` set to `WEAP_IDLE` with the toggle flipped,
  unconditionally, on the frame after.

VERIFIED, off the grenade capture, whose `to_frag` step is the first
uncorrupted putaway anyone has measured: the putaway stores `WEAP_DROP` and
the pickup's two arms write one `weapAnim` each. VERIFIED, off the superseded
combat captures: no `weaponstate` 2 in either of them writes `weapAnim` at
all. INFERRED: those two runs sent `cmd.weapon` 0 every frame, which is the
one input 1.2's setter refuses to write on, so what they measured is the
artifact and not the path. 1.14 has the samples.

INFERRED: a reload or a rechamber can therefore be interrupted by a weapon
switch and a shot or a melee cannot. VERIFIED, off the grenade capture:
`weaponstate` 1 lasts `raiseTime`, and on a `semiAuto` weapon lasts longer
than that under a held trigger -- the capture measured it on the carbine, and
9.5 has retail's frag, which omits the key, raising in `raiseTime` under the
same held bit. INFERRED: the state ends on `weaponTime` reaching 0 like every
other one, and the semi-automatic latch of 1.4 is what holds it open; the earlier
reading here, that the raise does not wait for `weaponTime`, was inferred from
the check order alone and no capture then held a raise. 1.14 has the numbers.

#### Where `weaponstate` 2 comes from, and where it does not

VERIFIED: every write to `ps->weaponstate` in `game.mp.i386.so` stores an
immediate, 27 of them and no register form, and exactly two store 2, at
`0x37b3e` and `0x37c45`. VERIFIED: both sit inside the putaway function
`0x37a9c`, and the module contains exactly two calls to that function, at
`0x389ec` and `0x38a38`, both inside the weapon-change check `0x38918`.

INFERRED: the list above is therefore the complete set of putaway sources
inside this module. No jump path, stance-change path or prone path writes
`weaponstate` 2, and the only inputs the two call sites act on are
`pm->cmd.weapon` against `ps->weapon`, `ps->weapons` and `pm_flags & 0x10`.

VERIFIED: what produced the `weaponstate` 2 and event 156 that
`player-model-anim-system.md` used to record at a jump and at a stance change
was the capture itself: the probe sent `cmd.weapon` 0 in every usercmd, and
retaken with the probe sending the weapon it holds, neither a jump nor a
stance change raises anything. INFERRED: the byte reaches the server only in
the full usercmd branch, which a `wbuttons`, `upmove` or `weapon` change
forces (`docs/protocol-1.1.md`, "Usercmd delta"), so those two steps are
exactly the ones that put a 0 on the wire. INFERRED: so the first clause of
the list above is what fired, a change begun to weapon 0 because the cmd asked
for it. Neither a jump nor a stance change
puts a weapon away on its own, and `crates/common/src/pmove/weapon.rs` no
longer pretends they do.

### 1.9 Rechamber, and the bolt-action bitfield

VERIFIED: the check is dll `0x300100a0`. VERIFIED: the offsets, immediates,
weapon-def fields, event numbers and call targets named in the list below.
INFERRED: the ordering, and every "when", "unless", "otherwise" and "skipped"
in it, which are branch conditions.

- The whole check needs `weaponDef->boltAction` (`0x2C8`) non-zero and the
  weapon's bit set in `ps->weaponrechamber` (`ps+0x31C`, two dwords, indexed
  `weapon >> 5` and `1 << (weapon & 31)`).
- From `weaponstate` 4 with the delay expired it clears the bit and raises
  event `0xA3` (`EV_EJECT_BRASS`, 163).
- From `weaponstate` 0 it sets `weapAnim` to `WEAP_RECHAMBER` or
  `WEAP_ADS_RECHAMBER` on the same `fWeaponPosFrac > 0.75` test, sets
  `weaponstate` 4, `weaponTime` from `rechamberTime`, `weaponDelay` from
  `rechamberBoltTime`, and raises event `0xA2` (`EV_RECHAMBER_WEAPON`, 162).

VERIFIED: the fire path sets the bit (1.5, step 1) and both the rechamber and
the reload clear it. INFERRED: that is the whole "this weapon has a spent case
in it" state, and it is per weapon rather than per player, so a bolt-action put
away mid-cycle still needs its bolt worked when it comes back. INFERRED: it
also accounts for the mosin's three events per shot in
`player-model-anim-system.md`, 159 then 162 then 163, and for the carbine's
one, since with `boltAction` 0 the bit is never set.

### 1.10 Melee

VERIFIED: the offsets, immediates, weapon-def fields, event numbers and
call targets named in the list below. INFERRED: the ordering, and every
"when", "unless", "otherwise" and "skipped" in it, which are branch
conditions.

- dll `0x30011930`, the check: it needs `weaponDef->meleeDamage` (`0x1C4`)
  non-zero and the delay-expired flag clear, and it needs `weaponDelay` to be
  0 or `weaponstate` to be one of 5, 6, 7, 8, 9.
- Same function: when `pm->cmd.buttons & 0x20` is clear it clears
  `pm_flags & 0x1000`, and when the bit is set and `pm_flags & 0x1000` is
  clear it sets that flag and proceeds.
- Same function: proceeding skips `weaponstate` 1, 2, 10 and 11; otherwise it
  sets `weapAnim` to `WEAP_MELEE_ATTACK`, raises event `0xA4`
  (`EV_MELEE_SWIPE`, 164), and with `meleeDelay` non-zero sets `weaponTime`
  from `meleeTime`, `weaponDelay` from `meleeDelay` and `weaponstate` 10.
- dll `0x30011850`, the finish: it raises `weaponTime` to at least
  `meleeTime - meleeDelay`, raises event `0xA5` (`EV_FIRE_MELEE`, 165), and
  sets `weaponstate` 11.
- dll `0x300118d0`: state 11 clears to 0 with `WEAP_IDLE`.

INFERRED: `pm_flags 0x1000` is the melee button's edge latch, and it is the
only edge latch in the weapon machine.

What the retail melee capture read off this path is in 1.14.

### 1.11 Grenades in the same machine

VERIFIED: the offsets, immediates, weapon-def fields, event numbers and
call targets named in the list below. INFERRED: the ordering, and every
"when", "unless", "otherwise" and "skipped" in it, which are branch
conditions.

- `.so` `0x39158`: with `weaponDef->weaponType == 1` and `ps->grenadeTimeLeft`
  (`ps+0x34`) positive, `PM_Weapon` decrements it by `pml.msec`, and when it
  falls to 50 or below it pins it at 50, raises event `0x9F`
  (`EV_FIRE_WEAPON`, 159) and sets `weaponTime` to 1600, then returns.
- dll `0x30011410`, the grenade's own fire path: it sets `grenadeTimeLeft`
  from `weaponDef->fuseTime` (`0x210`), sets `weapAnim` to 17, raises event
  `0x9E` (`EV_PULLBACK_WEAPON`, 158), sets `weaponDelay` from
  `weaponDef->holdFireTime` (`0x1E0`) and clears `weaponTime`.

The first of those two never runs on 1.1 MP; 1.14 has the capture that says
so and the path a throw takes instead.

### 1.12 What stops `PM_Weapon` outright

VERIFIED: the three tests and the store in the list below, all in `.so`
`0x390e0`. INFERRED: the order they are tested in.

- `ps->pm_flags & 0x800` set: return with nothing done.
- `ps->pm_type > 5`: `ps->weapon = 0`, return.
- `ps->eFlags & 0xC000` set: return with nothing done.

VERIFIED: the second test is `0x390f8: cmp DWORD PTR [eax+0x4],0x5`, the next
instruction is `0x390fc: jle 39110`, and `0x390fe` stores 0 into `ps->weapon`.
INFERRED: the `> 5` case is therefore the fallthrough. VERIFIED: the head of
`player_die` is `0x49a5d: cmp DWORD PTR [eax+0x4],0x5` with a `jg` to its
epilogue, and
`P_DamageFeedback` is `0x3f512: cmp DWORD PTR [ebx+0x4],0x5` with a `jg`,
which is the module's only `ebx` form of that compare. VERIFIED: the binary
carries no name for the `pm_type` values above 5. INFERRED: 6 and 7 are the
two dead ones, from CoDExtended's `shared.h` naming them `PM_DEAD` and
`PM_DEAD_LINKED`.

INFERRED: `eFlags 0xC000` marks a player on a mounted MG, since `FireWeapon`
tests the same mask before taking its turret branch and the fire path skips the
clip decrement when it is set.

UNVERIFIED: what sets `pm_flags 0x800`, and what `pm_flags 0x400` and `0x4000`
mean. VERIFIED: `BG_GetMinSpreadForWeapon` tests `pm_flags & 1` and
`pm_flags & 2` and loads `hipSpreadProneMin`, `hipSpreadDuckedMin` and
`hipSpreadStandMin` off those tests, and the weapon-change check tests
`pm_flags & 0x10` and holds a store of weapon 0. INFERRED: the prone minimum
is the `& 1` arm, the ducked one the `& 2` arm, and the weapon-0 store is the
`& 0x10` arm. INFERRED: so `0x1` is prone, `0x2` is crouch and `0x10` is the
ladder.

### 1.13 `ps.fWeaponPosFrac`, and the flag between the button and it

VERIFIED: `game.mp.i386.so` holds exactly four stores to `ps+0xB8`. Three are
in `PM_UpdateAimDownSightLerp` (`0x372FC`), at `0x3731E` (the immediate 0),
`0x37446` (the ramp) and `0x37477` (the clamp); the fourth is `0x46B6E`, a
`movl $0x0` on the spawn path. VERIFIED: the lerp is called once, from
`PM_Weapon` (`0x390E0`) at `0x39143`, early in the function. VERIFIED: the
client carries the identical function at `cgame_mp_x86.dll` `0x3000FC80` --
same weapon-def offsets `0x2CC`, `0x2EC`, `0x3D4`, `0x414`, `0x418`, same
`weaponstate` values, same `pm_flags & 0x20`, same clamp -- so this is shared
bg code and the client predicts it off every snapshot.

VERIFIED for every offset, immediate, weapon-def field and compare below;
INFERRED for the ordering and every "when" and "unless", which are branch
conditions.

- `weapDef->aimDownSight` (`0x2CC`) 0: the fraction is stored 0 and nothing
  else runs (`0x3730e`).
- With `segmentedReload` (`0x2EC`) clear, the sight is refused while
  `weaponstate` is 5 and `weaponTime - weapDef->adsReloadTransTime (0x3D4)`
  is above 0 (`0x37342`-`0x3735a`). With it set, `weaponstate` 5, 6, 7 and 8
  refuse outright and 9 refuses on the same time test (`0x37368`-`0x3738a`).
- Otherwise the target is `ps->pm_flags & 0x20` (`0x37392`).
- `weapDef->adsFire` (`0x2DC`) with `ps->weaponDelay` non-zero and
  `weaponstate` 3 forces the target up whatever the flag says
  (`0x3739d`-`0x373b7`).
- The step is `+pml.msec * weapDef[0x414]` toward the sight (`0x37415`) and
  `-pml.msec * weapDef[0x418]` back (`0x3743a`), clamped to 0..1
  (`0x3744c`-`0x37477`). Both factors are the reciprocal milliseconds of
  section 0, so it is a linear ramp over `adsTransInTime` / `adsTransOutTime`.

The ramp reads no usercmd. `PM_UpdateAimDownSightFlag` (`0x37230`) writes the
`pm_flags` bit it does read. VERIFIED: `PmoveSingle` calls it at `0x341C8`,
`0x341E6`, `0x34200`, `0x3423D`, `0x3429A` and `0x342D3`, one per `pm_type`
arm, each immediately before `PM_UpdatePlayerWalkingFlag`. Same labels as
above:

- `ps->pm_type > 5` clears (`0x37242`), and so does `cmd.buttons & 0x10`
  being clear (`0x37247`) or `aimDownSight` being 0 (`0x37252`).
- `weaponstate` 1, 2, 10 or 11 -- raise, holster, melee windup, melee relax --
  clears (`0x3725b`-`0x3726e`).
- `pml[0x30] == 0 && ps->pm_type != 1` clears (`0x37275`). `pml+0x30` is
  INFERRED to be `groundPlane` from the Q3 `pml_t` layout, with `frametime`
  at `+0x24` and `msec` at `+0x28` pinned by their use in the ramp; the offset
  and the compare are VERIFIED. INFERRED: an airborne player cannot hold the
  sight, and the fraction ramps down for the whole jump.
- Otherwise, for a prone player (`pm_flags & 1`, `0x37283`) the flag is left
  as it is when the *previous* cmd held the sight bit and either movement axis
  is non-zero (`0x3728a`-`0x37295`), and is otherwise set together with
  `pm_flags 0x400` (`0x3729e`). A player who is not prone just takes the flag
  (`0x372a4`).
- `BG_UpdateConditionValue(ps->clientNum, 7, (pm_flags & 0x20) ? 1 : 0, 1)`
  at `0x372d8` / `0x372ef` is fed the flag, not the raw button. INFERRED:
  animscript condition 7 is `ads`, so the animscript sees the gated form.

VERIFIED: `PM_ClearAimDownSightFlag` (`0x3ABD4`) is the same
`and byte [ps+0xc], 0xdf`, and `PmoveSingle` calls it once, at `0x33EF9`, in
the early dead arm. VERIFIED: `PM_UpdatePlayerWalkingFlag` (`0x33694`) sets
`pm_flags 0x80` only when `0x20` is set, the player is not prone, and
`weaponstate` is not one of 5, 6, 7, 8, 9. INFERRED: `0x80` is the ADS walk
slow-down, and it is off for the whole of a reload while `0x20` stays on.

INFERRED, from the two rules together: the sight can be *asked for* during a
reload -- the flag update excludes only `weaponstate` 1, 2, 10 and 11 -- but
the fraction's target is 0 until the reload has `adsReloadTransTime` left
to run, which for `m1carbine_mp` is the last 600 ms of 2650 and for
`mosin_nagant_mp` the last 600 of 2400. So the sight comes up at the tail of a
reload and not at the key.

**What the `--save-ads` captures measured**, 2026-09-05, one per gate map
(`crates/server/tests/fixtures/playerstate/<map>-dm-ads.txt`, `!trace` per
snapshot with `fWeaponPosFrac` and `aimSpreadScale` as decimals), the probe
standing still and holding each input in turn. VERIFIED, every number;
INFERRED, that they are the file's fields, which is arithmetic.

- The ramp is linear and the rates are the reciprocal times: up, the
  carbine and the mosin both cross 0.0533 at 16 ms and 0.8867 at 258 ms and
  read 1.0 at 323, which is `adsTransInTime` 0.3 s; down, the carbine steps
  0.125 per 48 ms and reaches 0 at 403 ms (`adsTransOutTime` 0.4) and the
  mosin 0.0833 per 48 ms, 0 at 612 (0.6).
- Through a reload with the sight held: the fraction ramps *down* from the
  reload's first frame at the out rate (carbine 1.0 to 0 over 467 ms, mosin
  over 661), sits at 0, and ramps back up at the in rate from about 2080 ms
  on the carbine (`reloadTime` 2650 less 600) and 1840 on the mosin (2400
  less 600), reaching 1.0 some 300 ms before `weaponstate` leaves 5. A
  reload asked for on a full clip does nothing at all, which the first
  capture found by having nothing to show for its reload step.
- The sight released mid-ramp turns straight round: carentan's
  `ads_release_2` reads 0.92 at 32 ms off a fraction that had just reached
  1.0, and `idle_after` the same.

The trace fields in a snapshot are the state after the last of that
frame's pmove steps, and the probe sends a cmd every 16 ms, so a 48 ms gap
holds three steps. That is why the counter below can read a decay on the
same snapshot the fraction first reads 1.0.

**As implemented.** `crates/common/src/pmove/weapon.rs` carries all of the
above: `update_ads_flag` the flag, `ads_pm_flags` the `0x20`/`0x80` pair for
the wire, `advance_ads` the ramp, and `fire` the `adsFire` `weaponDelay`
override. Three divergences, each deliberate. `pm_flags 0x400` is not
modelled, since it is unnamed (section 10). The dead clear lives in
`pmove::dead_move` rather than in the flag update, because no pmove step runs
for a dead player at all, which leaves the fraction frozen where the death
found it rather than ramping it down. And `pml.groundPlane` is read as
`ps.on_ground`, the nearest thing vcod's mover carries.

---

### 1.14 What the grenade and melee capture measured

`crates/server/tests/fixtures/playerstate/mp_carentan-tdm-grenade.txt`, a lone
`--save-grenade` run against the retail 1.1d server: one melee swing, a switch
to the frag, a cooked throw, a cook held past the fuse, a cook cancelled by
switching back, and a throw at the ground. The step names below are the
fixture's own.

**The fuse does not run down.** VERIFIED: `grenadeTimeLeft` reads a flat 4000
(`fuseTime` 4) from the pullback frame to the throw frame and 0 on every other
sample, under a 200 ms, a 1000 ms and a 5000 ms hold alike. VERIFIED:
`pin_out` holds the trigger 5000 ms and the throw's event 159 lands on the
release, at +5032 ms, with the explode 3954 ms after that. INFERRED: 1.11's
decrement is unreachable on this build, so there is no pin at 50, no
auto-throw and no shortening of the fuse by cooking.

**The pin, and what the release is.** VERIFIED: `weaponDelay` counts down from
`holdFireTime` (600) while the bit is held and then reads 1 on every sample
until the release. INFERRED: that pin is what stops the delay edge repeating,
the same shape the semi-automatic latch of 1.4 gives `weaponTime`.
UNVERIFIED: which store writes the 1.

**The throw is the ordinary fire path.** VERIFIED: all three throw frames read
`weaponDelay` 0, `weapAnim` `WEAP_ATTACK` (`WEAP_ATTACK_LASTSHOT` on the last
frag), event 159 (161 on the last), and `weaponstate` 3 for exactly 1000 ms,
which is the frag's `fireTime`. INFERRED: the throw runs 1.5 with its two
`weaponType == 1` special cases, steps 1 and 5, which is why `fireDelay` never
reaches `weaponDelay`. VERIFIED: `throw_down`'s 161 is followed by 149 in the
same frame and the step after it holds `ps.weapon` 0. INFERRED: that is 1.5
step 9, the `clipOnly` weapon being taken away.

**A weapon change cancels the pullback without a putaway.** VERIFIED: `cancel`
switches 300 ms into a hold and reads `weaponstate` 3 then 1 with event 155
and `weapAnim` 522, no event 156, no `weaponstate` 2 sample at all, and the
clip unspent -- three throws still empty the loadout at `idle_after`.
INFERRED: the switch takes the short putaway branch of 1.8, the one that
clears `weaponTime` and `grenadeTimeLeft` rather than raising the drop, and
the pickup half runs later in the same `PM_Weapon` call.

**The putaway does write `weapAnim`.** VERIFIED: `to_frag` reads `weapAnim`
521 -- `WEAP_DROP` with the toggle -- through the whole of `weaponstate` 2,
then 10 through the raise and 512 after it. INFERRED: the putaway stores the
index and the pickup's two arms are exclusive, one write each; the superseded
combat captures read no write at all because they sent `cmd.weapon` 0 every
frame, which is the one input 1.2's setter refuses to write on.

**The raise ends when `raiseTime` does.** VERIFIED: `weaponstate` 1 lasts
295 ms on `to_frag` and 263 ms on `to_frag_2`, against the frag's `raiseTime`
of 250, and 2.7 s on `cancel`, whose trigger is held through the raise.
INFERRED: the state ends on `weaponTime` reaching 0 and the semi-automatic
latch of 1.4 pins it at 1 for as long as the bit is down, so a held trigger
keeps a weapon coming up.

**The swing.** VERIFIED: `melee_tap` reads `weaponstate` 10 with event 164 and
`weapAnim` 520, then 11 with event 165 once `meleeDelay` (0.15 on the carbine)
runs out, then 0 with `weapAnim` 0 at `meleeTime` (0.65), and `torsoAnim` 732
across both states. VERIFIED: `aimSpreadScale` reads 255.00 at every sample of
states 10 and 11 and starts decaying on the first sample of state 0.
UNVERIFIED: which store holds the counter there; 1.8's raise is the only 255.0
store this document has located.

**A `both` event clause is a legs anim.** VERIFIED: each of the three throw
frames reads `legsAnim` 575, index 63 with the toggle set, and a `torsoAnim`
that differs from the sample before it by the toggle alone -- 512 on
`cook_release` and `throw_down`, 0 on `pin_out`, index 0 either way. VERIFIED:
`mp/playeranim.script`'s standing grenade clause is
`both pb_stand_grenade_throw`. INFERRED: a `both` clause puts the anim on the
legs and restarts the torso on no anim at all, which is the same index 0 every
settled retail pose reads (player-model-anim-system.md, "The weapon channel").

**A `both` land clause is not.** VERIFIED: the sample after the one where
`throw_down`'s frag knocks the thrower off his feet reads `legsAnim` 100,
index 100 with the toggle clear, between two samples of 634, and `torsoAnim`
512 across all three. VERIFIED: the landing clause the stance and class
select is `weaponclass pistol AND grenade`, whose body is
`both pb_standjump_land_pistol duration 5`. INFERRED: the land
event puts its anim on the legs and leaves the torso alone, so the rule above
is the throw's and not every `both` clause's; the single sample is the
`duration 5`.

INFERRED, and unmeasured: vcod applies the throw's rule to the rest of the
event clauses, so it also reaches `fireweapon`'s pistol-ADS clause and
`jump`'s two run clauses. No capture covers either.

**As implemented.** `pmove::weapon`'s `pullback`, `grenade_hold`,
`melee_check` and `melee_finish`, with the two-arm `pickup` and the short
`putaway` branch beside them, and `spectate.rs::play_event` for the two
paragraphs above it, whose landing branch clears the torso of the selection
before it plays it. `crates/server/tests/playerstate_combat_ab.rs`'s `grenade`
gate replays the whole capture, from the spot the capture's own header
records: where a blast lands, and so who it hurts, is the map's business.

## 2. `Bullet_Fire_Extended`: spread, the trace, and what a bullet does

### 2.1 Where the spread number comes from

VERIFIED, `FireWeapon` `0x68d68`: the shot's view angles are
`ps->viewangles` with components 0 and 1 replaced from `client+0x220C` and
`client+0x2210`, and `AngleVectors` turns them into a forward/right/up axis
triple. VERIFIED: the muzzle is the entity's origin with
`ps->viewHeightCurrent` (`ps+0xD0`) added to z, and the function calls
`G_AddLean` on it and rounds each component to an integer with an explicit
`fldcw`. INFERRED: the lean is applied before the rounding.

VERIFIED, `ClientEndFrame` `0x410f1`: `client+0x2240` is
`ps->aimSpreadScale / 255.0`. INFERRED: it is computed once a frame, since
that is when `ClientEndFrame` runs. VERIFIED, `FireWeapon`
`0x68eb9`: it compares `ps->fWeaponPosFrac` against 1.0 and has two arms, one
computing `adsSpread + (hipSpreadMax - adsSpread) * client->0x2240` and the
other `min + (hipSpreadMax - min) * client->0x2240` where `min` is
`BG_GetMinSpreadForWeapon(ps, weapon, level.time)`. INFERRED: the `adsSpread`
arm is the one an equal comparison takes.

VERIFIED, `BG_GetMinSpreadForWeapon` `0x37114`: it compares
`ps->viewHeightCurrent` against `ps->viewHeightTarget` and
`ps->viewHeightLerpTime` against 0, tests `pm_flags & 1` and `pm_flags & 2`,
and loads `hipSpreadProneMin`, `hipSpreadDuckedMin` and `hipSpreadStandMin`
off those tests. VERIFIED: a second path blends two of the three by a fraction
clamped to 0..1. INFERRED: the settled stance picks one of the three outright
and the blend is what a stance still lerping takes.

`PM_AdjustAimSpreadScale` `0x385e8` (dll `0x30011050`) is what moves
`aimSpreadScale` between shots. VERIFIED: `PmoveSingle` calls it once, at
`0x340E0`, right after `BG_GetInfoForWeapon` (`0x340D6`) and before
`PM_UpdateViewAngles` (`0x340FC`), so it runs on the raw usercmd angles, and
before `PM_UpdateAimDownSightFlag` and `PM_Weapon`, so it reads the previous
frame's `fWeaponPosFrac`. VERIFIED, `.rodata`: `0x72538` = 0.01,
`0x7253C` = 0.5, `0x72540` = 0.0054931640625 (`SHORT2ANGLE`, 360/65536),
`0x72544` = 1.28, `0x72548` = 255.0. VERIFIED: the offsets, immediates,
weapon-def fields, constants and the `AngleSubtract` call target named in the
list below. INFERRED: the ordering, and every "when", "otherwise" and
"skipped" in it, which are branch conditions.

- The decay starts at `hipSpreadDecayRate` (`0x24C`, at `0x385f8`). A decay of
  0.0 short-circuits the whole function to `add = 0, decay = 1.0` with no
  frame-time scaling (`0x3860a` / `0x38754`), which is -255 a frame: a weapon
  with no decay rate slams the counter shut and holds it there.
- `airborne` is `ps->groundEntityNum == 1023 && ps->pm_type != 1`
  (`0x38619`). Airborne halves the decay (`0x38628`); otherwise `eFlags & 0x40`
  multiplies it by `hipSpreadProneDecay` (`0x260`, at `0x3863c`) and
  `eFlags & 0x20` by `hipSpreadDuckedDecay` (`0x25C`, at `0x38648`). The three
  arms are exclusive and airborne wins. The decay is then scaled by
  `pml.frametime` (`0x3864e`).
- The additions are skipped whole when `ps->fWeaponPosFrac == 1.0`
  (`0x38657`), an exact float compare, and in cgame an exact integer one
  against `0x3F800000`. The decay is not skipped, so a settled sight decays
  to 0 whatever the player does.
- With `hipSpreadTurnAdd` (`0x254`) non-zero (`0x38676`), axes 0 and 1 each
  add `fabs(AngleSubtract(SHORT2ANGLE(cmd.angles[i]),
  SHORT2ANGLE(oldcmd.angles[i]))) * 0.01 * hipSpreadTurnAdd / pml.frametime`
  (`0x3868e`-`0x386ed`). The input is a usercmd angle delta across one pmove
  step, not `ps->viewangles` and not a delta across a server frame.
- `hipSpreadMoveAdd` (`0x258`) non-zero (`0x386fe`) with either of
  `cmd.forwardmove` / `cmd.rightmove` non-zero (`0x38712`) adds it flat
  (`0x38719`); airborne adds 1.28 twice (`0x3873c`-`0x3874a`). The whole add
  is then scaled by `pml.frametime` (`0x38750`).
- The result is `aimSpreadScale += (add - decay) * 255.0`
  (`0x3876d`-`0x3877a`), clamped to 0..255 (`0x38780`-`0x387b5`).

INFERRED: the turn term divides by the frame time inside the loop and the
whole add multiplies by it again, so the turn term is frame-rate independent
(`|dAng| * 0.01 * turnAdd * 255` per frame) while the move and airborne terms
are not. The `fdiv` at `0x386d6` and the `fmul` at `0x38750` are both in the
`.so`, so this is not a decompiler artifact.

The shot's own add is `PM_Weapon`'s, not `FireWeapon`'s: section 1.5, step 8,
`hipSpreadFireAdd * 255.0` with the same `fWeaponPosFrac == 1.0` skip. Both
the per-frame delta and the per-shot add carry the same `* 255.0`.

With the stock numbers of section 0 this makes the move term decisive and the
turn term invisible: 8 against the carbine's decay of 4 saturates the counter
in five 50 ms frames and holds it at 255 for as long as an axis is held, while
both fixture weapons spell `hipSpreadTurnAdd 0`. Both retail motion captures
agree -- 255.0 at every pose with a movement axis, standing, crouched or
prone, and 0.0 at every pose without one.

The `--save-ads` captures (1.13) put numbers on the rest. VERIFIED: a hip
shot from the carbine reads 162.18 on the first snapshot after it, 16 ms
into a decay from `hipSpreadFireAdd` 0.7 times 255, and then 111.18, 43.86,
9.18, 0 at roughly 50 ms steps, which is the decay of 4 times 255 a second;
the mosin's reads 255 and comes down 41.4 a step. A walk climbs 51 a step
on the carbine (the add of 8 less the decay of 4, times 255, times the
frame) to 255 in 274 ms, and stopping brings it down 51 a step to 0 in 258.
A shot down the settled sight adds nothing, and a walk begun with the sight
coming up adds until the snapshot the fraction reads 1.0 and decays from the
next step on: `ads_walk` on carentan climbs to 255 at 276 ms with the
fraction at 0.94 and reads 222.36 at 324 with it at 1.0. INFERRED, that the
skip keys on the previous step's fraction, as 2.1's call order says: the
step that reaches 1.0 still adds, and the one after it does not. The BAR
is the one stock weapon with a turn term (`hipSpreadTurnAdd` 0.8) and it
is not captured.

**As implemented.** `pmove::weapon::adjust_aim_spread_scale`, called first of
all in `pmove` so it keeps retail's position ahead of the view update and the
flags. The turn term is written as the product the divide and the multiply
reduce to, which is the same number and does not divide by a zero-length
frame. A weapon index with no file behind it takes the `decay == 0` arm.

### 2.2 The cone

VERIFIED, `gunrandom` `0x3d4ac`: it calls `rand()` at `0x3d4bb` and at
`0x3d4d5`; the `0x3d4bb` result is scaled by `-1/2^31` (`.rodata 0x72a44`) and
by 360.0 and converted to radians, the `0x3d4d5` result by `-1/2^31` alone;
the two outputs are the `0x3d4d5` value times the cosine and the sine of the
angle. INFERRED: the pair is a point in the unit disc with
the radius drawn uniformly rather than by area, so the distribution is denser
at the centre.

VERIFIED, `FireWeapon` `0x68f1e` and `Bullet_Fire` `0x690dc`: the endpoint is
`muzzle + forward * 8192 + right * (x * R) + up * (y * R)` where
`R = tan(spread * PI / 180) * 8192` and `(x, y)` is the `gunrandom` pair. The
8192.0 is `.rodata 0x79cc0` in `FireWeapon` and `0x79cd8` in `Bullet_Fire`.
INFERRED: `spread` is therefore the cone's half-angle in degrees and the trace
runs 8192 units.

VERIFIED, `FireWeapon` `0x68f2c`: the `damage` argument the shot carries into
`Bullet_Fire_Extended` is `weaponDef->damage` (`0x1C0`), loaded straight from
the weapon def with no scaling.

### 2.3 The trace

VERIFIED, `Bullet_Fire_Extended` `0x68890`, signature read off its call sites
in `FireWeapon` (`0x68ff9`) and `Bullet_Fire` (`0x691b4`) and off its two
recursions: `(passEnt, attacker, start, end, damage, depth, params, shooter)`.
VERIFIED: `params` is the caller's stack frame holding `axis[3][3]` at `+0`,
the origin at `+0x24` and the weapon def pointer at `+0x3C`.

VERIFIED: it compares `depth` against 12 and holds the string
`"Bullet_Fire_Extended: Too many resursions, bullet aborted\n"`. INFERRED: the
print and the return are on the arm where `depth > 12`.

VERIFIED: the trace is `trap_LocationalTrace(&trace, start, end,
passEnt->s.number, 0x02802031, priorityMap)`. VERIFIED: the two candidates
for `priorityMap` are `riflePriorityMap` and `bulletPriorityMap`, selected off
a test of `weaponDef->rifleBullet` (`0x2C0`) at `0x688ef`. INFERRED:
`riflePriorityMap` is the one a `rifleBullet` weapon takes. VERIFIED: the
trace is per-bone rather than against the link box; what walks the bones and
what the priority map does there is section 3.

VERIFIED: the trace result at `[ebp-0x30]` is read at these offsets --
`+0` fraction, `+4` endpos, `+16` normal, `+28` surface flags, `+32` the hit
entity's contents mask, `+40` a 16-bit entity number, `+44` a 16-bit hit
location. The full 48-byte layout, including the two fields this caller does
not read, is in section 3. VERIFIED: bit `0x4` of `+28` suppresses the impact
effect, and bits 20 to 24 of the same word are shifted down into the temp
entity's `surfType` (`entityState+136`).

VERIFIED: `bulletPriorityMap` is 19 bytes at `0x7DD6C` and `riflePriorityMap`
19 bytes at `0x7DD7F`, one byte per hit location in the index order of
section 3:

| i | hit location | bullet | rifle |
|---|---|---|---|
| 0 | `none` | 1 | 1 |
| 1 | `helmet` | 3 | 9 |
| 2 | `head` | 3 | 9 |
| 3 | `neck` | 3 | 9 |
| 4 | `torso_upper` | 3 | 8 |
| 5 | `torso_lower` | 3 | 7 |
| 6 | `right_arm_upper` | 3 | 6 |
| 7 | `left_arm_upper` | 3 | 6 |
| 8 | `right_arm_lower` | 3 | 6 |
| 9 | `left_arm_lower` | 3 | 6 |
| 10 | `right_hand` | 3 | 5 |
| 11 | `left_hand` | 3 | 5 |
| 12 | `right_leg_upper` | 3 | 4 |
| 13 | `left_leg_upper` | 3 | 4 |
| 14 | `right_leg_lower` | 3 | 4 |
| 15 | `left_leg_lower` | 3 | 4 |
| 16 | `right_foot` | 3 | 3 |
| 17 | `left_foot` | 3 | 3 |
| 18 | `gun` | 0 | 0 |

VERIFIED, out of the bone loop that reads the table (section 3): the map is a
per-location weight the engine trace ranks candidates by, and it beats
distance -- a rifle bullet crossing both a leg and the head is scored as the
head however far behind the leg the head is, a pistol bullet, whose map is
flat, scores whichever of the two it reaches first, and `gun` at 0 is never
scored at all.

### 2.4 What the impact does

VERIFIED: the offsets, immediates, event numbers and call targets named in the
list below, each read out of the instruction it sits in. INFERRED: the
numbering, and every "when", "unless" and "otherwise" in it.

1. `G_CheckHitTriggerDamage(attacker, start, endpos, damage, mod)`.
2. With `trace.surfaceFlags & 4` clear and the hit entity having no client, a
   temp entity at `trace.endpos` carrying event `0xAD`
   (`EV_BULLET_HIT_SMALL`, 173), or `0xAE` (`EV_BULLET_HIT_LARGE`, 174) when
   `rifleBullet` is set. Its `eventParm` (`entityState+160`) is
   `DirToByte(trace.normal)`, `entityState+216` is `DirToByte` of the incoming
   direction mirrored in the plane, `surfType` (`entityState+136`) is bits 20
   to 24 of the trace's surface flags, and `otherEntityNum` (`entityState+116`)
   is the
   shooter's entity number. The client side of that temp entity is
   `docs/research/cod11-events-and-fx.md`, `EV_BULLET_HIT_*`.
3. With bit `0x10` of the trace's contents mask (`+32`) set, the bullet
   continues:
   `start` is moved to `trace.endpos` nudged along the ray by `0.25 / d` where
   `d` is minus the dot of the normal with the ray and the nudge is skipped
   when `d <= 0.125`, and the function recurses at the same damage with
   `depth + 1`. The 0.125 and 0.25 are `.rodata 0x79c60` and `0x79c64`.
4. Otherwise, when the hit entity's `takedamage` byte (`gentity+0x171`) is
   set, `G_Damage(hitEnt, attacker, attacker, params, trace.endpos, damage,
   dflags, mod, trace.hitLoc)`. INFERRED: `params` is passed where `G_Damage`
   expects a direction vector, and `params+0` is the axis triple's forward
   vector, so the damage direction is the shot's forward and not the ray from
   the muzzle to the impact.
5. When the hit entity has a client and `dflags` is non-zero, the function
   recurses with `passEnt` set to that entity, `start` at `trace.endpos`,
   `damage` halved by C integer division, and `depth + 1`, and it skips the
   recursion when the halved damage is not positive.

VERIFIED: the function tests `weaponDef->rifleBullet` at `0x688c6` and the
two arms load the immediate pairs (`dflags` `0x20`, `mod` 2) and (`dflags` 0,
`mod` 1). INFERRED: the first pair is the one a `rifleBullet` weapon takes.
VERIFIED: 2 and 1 are `MOD_RIFLE_BULLET` and `MOD_PISTOL_BULLET`
(section 4.1). INFERRED: since `dflags` is non-zero
only for a rifle bullet, step 5 means a rifle round passes through the player
it hits and carries half its damage to whatever is behind, and a pistol round
stops on the first player.

**Damage does not fall off with distance.** VERIFIED: nothing in
`Bullet_Fire_Extended`, `Bullet_Fire` or `FireWeapon` reads the trace fraction
or the distance from the muzzle; the `damage` argument reaches `G_Damage`
unchanged except for the halving in step 5. VERIFIED: the only weapon-file
field that scales it is `g_fHitLocDamageMult[hitLoc]`, applied inside
`G_Damage` (section 4.2).

### 2.5 Melee uses the same trace

VERIFIED, `Weapon_Melee` `0x68720`: it traces from the frame's origin to
`origin + forward * 64.0` (`.rodata 0x79c00`) with the same mask `0x02802031`
and always with `bulletPriorityMap`. VERIFIED: it spawns a temp entity whose
event is either `0xA6` (`EV_MELEE_HIT`, 166) or `0xA7` (`EV_MELEE_MISS`, 167),
filling `otherEntityNum` with the traced entity number, `eventParm` with
`DirToByte(trace.normal)` and `entityState+200` with the attacker's weapon.
INFERRED: the first event is the arm taken when the hit entity has a client,
read off the test of `gentity+0x158`. VERIFIED: it tests the traced entity
number against 1022 and the entity's `takedamage`, and calls `G_Damage` with
`dflags` 0, `mod` 7 (`MOD_MELEE`) and a damage of `weaponDef->meleeDamage`
plus `rand() % 5`. INFERRED: the call is on the arm where the number differs
from 1022 and `takedamage` is set.

**As implemented.** `melee_fire` (`crates/server/src/game/combat.rs`) shares
`trace_attack` with `bullet_fire`, so the two cannot disagree about a
player's box or its bones. Two divergences, both deliberate. The hit event's
`eventParm` is `DirToByte(-forward)`: the bone trace answers no surface
normal where retail's returns the bone's own, and 1.14's capture reads 115
for a hit against a forward of +y, which is that swing reversed and tilted.
And both events go to every client rather than to the PVS, since `TempEntity`
carries no scope between the two and a 64-unit reach puts every witness in
the PVS anyway.

### 2.6 `Bullet_Endpos`

VERIFIED: `Bullet_Endpos` is `0x69624`, 0xcf bytes, and nothing in the
combat path above calls it. UNVERIFIED: what does, and what it is for.

---

## 3. Hit locations

VERIFIED, `G_ParseHitLocDmgTable` (`0x4981c`, and `0x498b0` below is
`Base+0x94`, the head of its initialising loop): the name array is the 19
pointers at `0x7DD20`, in this index order.

| index | name | index | name |
|---|---|---|---|
| 0 | `none` | 10 | `right_hand` |
| 1 | `helmet` | 11 | `left_hand` |
| 2 | `head` | 12 | `right_leg_upper` |
| 3 | `neck` | 13 | `left_leg_upper` |
| 4 | `torso_upper` | 14 | `right_leg_lower` |
| 5 | `torso_lower` | 15 | `left_leg_lower` |
| 6 | `right_arm_upper` | 16 | `right_foot` |
| 7 | `left_arm_upper` | 17 | `left_foot` |
| 8 | `right_arm_lower` | 18 | `gun` |
| 9 | `left_arm_lower` | | |

VERIFIED: `G_GetHitLocationString(i)` (`0x4a8b0`) returns the 16-bit word at
`0xAA140 + i*2`. VERIFIED: `G_GetHitLocationIndexFromString(s)` (`0x4a8c4`)
compares the argument against each of the same 19 words up to index 0x12 and
has two return paths, the loop counter and a zeroed `eax`. INFERRED: the zero
is what a walk with no match returns. VERIFIED: `G_ParseHitLocDmgTable` fills
that table with `Scr_AllocString(name, 1)` per index. INFERRED: the table
therefore holds interned script strings, not configstring numbers.

**No box partition, no angle test.** VERIFIED: the index does not come from
any code in `game.mp.i386.so`; it arrives as the 16-bit field at offset 44 of
the `trap_LocationalTrace` result and goes straight into
`g_fHitLocDamageMult[]` and into script. VERIFIED: the partition lives in the
engine binary, which is what the rest of this section reads.

### 3.1 Where the index comes from: a ray against per-bone boxes

VERIFIED: the index is produced by a ray against per-bone oriented boxes on
the entity's posed skeleton, with a per-bone bounding sphere as the broad
phase and the `priorityMap` argument of 2.3 as the ranking. VERIFIED: both the
box and the hit-location byte ship per bone in the `xmodelparts` file
(`docs/research/xmodel-v14-format.md`), so nothing is hardcoded and nothing is
keyed on a bone name.

**The dispatch chain.** VERIFIED, `game.mp.i386.so`: `trap_LocationalTrace`
is at `0x63948`, and its body pushes the immediate `0x2b` before the indirect
call through the syscall pointer at `0x7ec10`, so the trap number is 43.
(CoDExtended's `shared.h:1145` says 46; its enum carries three traps 1.1 does
not have and is off by +3 across this range.) VERIFIED, `cod_lnxded`: the
game syscall dispatcher, my name `SV_GameSystemCalls`, is at `0x8087dcc`; it
reads `args[0]`, rejects anything above `0xd3` and jumps through the
212-entry table at `0x80d4eac`, whose default arm at `0x8089100` prints
`"Bad game system trap: %i"` (`0x80d4e8c`). VERIFIED: table entry 43 is
`0x8088316`, and that case pushes eleven arguments and calls `0x80916f4`.
INFERRED, mapping the pushes back to the trap's arguments: the call is
`SV_Trace(results, start, mins=NULL, maxs=NULL, end, passEntityNum,
contentmask, 0, locational=1, priorityMap, staticmodels=1)`. VERIFIED:
`0x80916f4` is the same address CoDExtended calls directly
(`private/reference/CoDExtended/src/sv_world.c:20`), and the prototype
published there names the `locational` flag and the `char *priorityMap`,
which confirms the tail of that signature independently.

**Per entity: pose first, then the bone trace.** VERIFIED, the operands,
offsets, immediates and call targets in this list are read out of the
instructions of `SV_ClipMoveToEntity` (my name, `0x809105c`); INFERRED, its
ordering and its conditions, read off the branches.

- A null `mins`/`maxs` is replaced with `0x80cdfc8`, three zero floats, in the
  caller; the world trace runs first and the entity pass is driven from a clip
  struct handed to `0x805a708`, which recurses through the area-node tree at
  `0x8059e94` and calls this function once per entity in a leaf.
- The clip struct built at `0x80917de` holds `start` at `+0x00`, `end` at
  `+0x0c`, the `trace_t` at `+0x18`, `passEntityNum` at `+0x48`, the pass
  entity's owner at `+0x4c`, the contentmask at `+0x50`, the `locational` flag
  at `+0x54` and `priorityMap` at `+0x58`.
- `0x80910c5`: the per-bone path is taken only when `locational` is non-zero,
  the entity resolves to a model record, and `gentity+0xf4 & 6` is non-zero.
  Bit `0x4` selects a brush trace (`0x80c52c0`); bit `0x2` without `0x4` is the
  animated-model arm, and it is the only one handed the `priorityMap`.
- `0x8091236`: a ray/AABB reject against the entity's bounds runs before any of
  the expensive work. The link box is therefore a broad phase and nothing more:
  a ray can clip it and score no bone, and that is a miss.
- `0x809124c` calls `vmMain` with export id 13. VERIFIED, `game.mp.i386.so`
  `vmMain` `0x50dd4`, jump table at `.rodata 0x75728`, 21 entries: entry 13 is
  `0x50ed0`, which computes `&g_entities[arg]` and calls `G_DObjCalcPose`
  (`0x67314`). The engine asks the game module to pose exactly the entity it is
  about to trace, immediately before tracing it.
- `0x8066520` transforms the clip's `start` and `end` into the entity's local
  frame, so the bone trace works entirely in entity space, and `0x80c4564` (my
  name `SV_LocationalTraceModel`) is called with `(model, localStart, localEnd,
  priorityMap, &localResult)`, its fraction seeded with the clip's current best.

**What poses the entity.** VERIFIED, `G_DObjCalcPose` (`0x67314`), the whole
function: it memsets a 16-byte bone mask to `0xff`, calls
`trap_DObjCreateSkelForBones(ent, mask)` and returns early when that is
non-zero, then `trap_DObjCalcAnim(ent, mask)`, then the per-entity hook at
`ent+0x220` when it is non-null, then `trap_DObjCalcSkel(ent, mask)`.
INFERRED: the skeletal math is the engine's and the game module owns only
which animations are playing. VERIFIED: the only other caller of the pose
path is `ClientEndFrame`, and there only when the `g_debugLocDamage` cvar is
set, followed by `trap_XModelDebugBoxes`. INFERRED: on a normal frame the
server updates the DObj's model list and animation weights but does not run
the skeleton, and the matrices are computed lazily by the trace that needs
them.

VERIFIED, `BG_UpdatePlayerDObj` (`0x2bd80`): it frees and rebuilds the DObj
when the model set changed, walking six attachment slots (`+0x7c`, stride
`0x40`, each a model name resolved by `trap_XModelGet`) plus the base model,
and hands the list to `trap_DObjCreate`. INFERRED: the head and the helmet are
therefore DObj parts, and the skeleton the trace walks is the grafted
body-plus-attachments one. VERIFIED, `BG_PlayerAnimation` (`0x2c1f4`): it
calls the anim-condition updaters and then works through `Scr_GetAnimsIndex`
and `trap_XAnimGetWeight` on the DObj's anim tree. INFERRED: the pose is the
blended animtree state the animscript machine drives, so stance, lean and aim
pitch are all in it, and `legsAnim`/`torsoAnim` are the wire projection of the
same state rather than its source.

**The bone loop.** VERIFIED, `0x80c4564`, the same two-label rule as above --
operands and targets out of the instructions, ordering and conditions off the
branches:

- `dobj+0x16` (u8) is the part count and `dobj+0x18[]` the array of part
  pointers; each part chain lands on a skeleton record whose `+0x0` points at
  the bone-name array, `+0x4` (s16) is the root-bone count, `+0x8` the per-bone
  box array with a stride of `0x28`, and `+0x14` the per-bone hit-location byte
  array.
- `0x80c4db0` is the bone-matrix accessor, `dobj->4 + dobj->0x50[part] * 64 +
  0x30`: `dobj->0x50` is a per-part byte giving that part's first bone in the
  combined skeleton, and each bone matrix is 64 bytes, three 16-byte rows plus
  the translation at `+0x30`.
- Per bone, `hl` is the hit-location byte and `prio` is `priorityMap[hl]`.
  VERIFIED: `hl` indexes the 19-byte map directly, so it is the hit-location
  index of the table above.
- INFERRED, branches at `0x80c46e7` and `0x80c4716`: a bone whose `prio` is 1 --
  which for both shipped maps means `hl == 0`, `none` -- inherits, from the
  DObj's remap list when this bone is the next one it lists, otherwise from its
  parent bone within the part, and for a part's root bones from `dobj->0x48[i]`
  where `0xff` means `none`. The effective code is written into a local array so
  later bones inherit through it. INFERRED: this is what gives an attached
  model's roots the hit location of the body bone they hang off.
- `0x80c479a`: the bone is skipped when its sphere radius (box record `+0x24`)
  is zero, and when `bestPriority > prio`. `bestPriority` starts at 2, so `none`
  (1) and `gun` (0) bones are never traced.
- Broad phase: the bone's sphere centre (box record `+0x18`) is transformed to
  entity space by the bone matrix and the segment's closest approach is compared
  against the radius; when `prio` equals the current best, a second early-out
  compares that approach against the fraction already found.
- Narrow phase: `start` and `end` are transformed into the bone's own frame by
  the bone matrix and a six-plane slab test runs against `boxRecord+0x00`
  (mins) and `boxRecord+0x0c` (maxs).
- On a hit at `0x80c4c21` onward: an equal `prio` is discarded unless it is
  nearer than the fraction already stored, and a differing one -- necessarily
  greater, by the skip above -- raises `bestPriority` and takes the hit
  regardless of distance. The result gets the fraction, the bone's name id, the
  effective hit-location code, and a normal that is a signed row of the bone
  matrix.
- A segment that starts inside a box sets the two flag bytes at `+0x18`/`+0x19`
  of the local result, zeroes the fraction and returns at once.

So the engine's box record is 40 bytes, `{ vec3 mins; vec3 maxs; vec3
sphereCentre; float radius }`. The file supplies the first 24; INFERRED that
the engine derives the sphere from them at load, since the file carries nothing
else.

### 3.2 The 48-byte trace result

Pinned field by field from the writes in `SV_ClipMoveToEntity` and
`SV_LocationalTraceModel`.

| off | size | field | evidence |
|---|---|---|---|
| 0 | f32 | `fraction` | VERIFIED, seeded from the clip and compared throughout |
| 4 | 3xf32 | `endpos` | VERIFIED, `start + fraction * (end - start)` at `0x8091352` |
| 16 | 3xf32 | `normal` | VERIFIED, the local normal rotated to world at `0x809134a` |
| 28 | i32 | `surfaceFlags` | VERIFIED; zero for a bone hit, the model trace zeroes it |
| 32 | i32 | contents of the hit entity | VERIFIED, `gentity+0x118` copied at `0x8091472` |
| 36 | ptr | texture name | VERIFIED, written 0 for an entity hit at `0x809146b` |
| 40 | u16 | `entityNum` | VERIFIED, `*(u16*)gentity` at `0x8091464` |
| 42 | u16 | bone name id | VERIFIED, the winning bone's name id |
| 44 | u16 | `hitLoc` | VERIFIED, the winning bone's effective hit-location code |
| 46 | u8 | all-solid | VERIFIED written and OR-merged; INFERRED name |
| 47 | u8 | start-solid | VERIFIED written and OR-merged; INFERRED name |

Two corrections this forces on what used to be here. `+32` is the hit entity's
contents mask, not a second flag word, which is why a bit in it (`0x10`) is
what makes a rifle bullet continue (2.4, step 3). And CoDExtended's
`shared.h:371` labels offset 44 `surfaceFlags` with an in-source admission that
it is a guess: it is the hit location, and the `allsolid` / `startsolid` its
comment goes looking for are the two bytes at 46 and 47.

VERIFIED: a world or brush hit leaves `hitLoc` 0 (`none`), because only the
bone trace ever writes it.

### 3.3 Where the boxes live

VERIFIED, parsed out of the stock paks: every `xmodelparts` entry carries a
bone-space AABB in the 24 bytes after each bone name and a hit-location byte
per bone after the name block, and all 725 non-empty entries in `pak0-6` parse
to exact EOF with the trailing byte only ever taking values 0..18. The layout
is in `docs/research/xmodel-v14-format.md`.

VERIFIED, `xmodelparts/USAirborne3`, the LOD0 of all 17 `playerbody_*` models:
`tag_origin` is 0; the pelvis and lower spine chain to `torso_lower`; `bip01
spine2`, `back_up` and the breast-pocket tags to `torso_upper`; `bip01 neck` to
`neck`; `bip01 head` to `head`; `tag_helmet` and `tag_helmetside` to `helmet`;
each clavicle and upperarm to the matching `arm_upper`, forearm to `arm_lower`,
hand and its fifteen finger bones to `hand`; thigh to `leg_upper`, calf to
`leg_lower`, foot and toe to `foot`; and `tag_weapon_left` /
`tag_weapon_right` to 18, `gun`. VERIFIED: on the playerbody the head and neck
boxes are all-zero, as is every `tag_*`; the head boxes live in the attached
head model, where `xmodelparts/basehead21` gives `bip01 head` code 2 with a
real box, `bip01 neck` code 3 and every facial bone code 2. INFERRED, from
the radius skip above: an all-zero box is how the artist turns a bone off.

VERIFIED: `xmodel/playerbody_american_airborne` parses to exact EOF with
`collision_lod = -1` and no collision surfaces, so a player model carries no
collision LOD at all and the per-bone box is the only hit geometry there is.

VERIFIED: there is no `hitloc` csv, txt or shader anywhere in the paks. The
only files naming the 19 locations are `info/mp_lochit_dmgtable`,
`info/ai_lochit_dmgtable`, the gametype scripts, `animscripts/death.gsc`,
`animscripts/pain.gsc` and the game module's string pool.

INFERRED, worth keeping for verification later: `g_debugLocDamage 1` on the
retail server is a per-frame `G_DObjCalcPose` plus `trap_XModelDebugBoxes`,
the engine's own visualisation of exactly these boxes. It draws client-side,
so a headless probe cannot see it.

### 3.4 As implemented

vcod runs the same trace. `vcod_common::bonetrace::bone_trace` walks the posed
skeleton's per-bone boxes in each bone's own frame and ranks candidates by the
weapon's priority map, with `bestPriority` starting at 2, ties going to the
nearer hit and a start-solid returning at once;
`crates/server/src/game/combat.rs` picks `riflePriorityMap` or
`bulletPriorityMap` off `rifleBullet`, keeps the link box as the broad phase
only, and takes the first candidate whose bones the segment actually scores.
The height-fraction partition that used to stand here is gone.

The victim is posed per shot, not per frame, out of the same grafted
body-plus-head-plus-helmet skeleton the client draws
(`crates/server/src/game/hitrig.rs`): the legs and torso clips the animscript
picked, each at the phase its channel started at, then the spine aim layer.
Three deliberate gaps, none of them measured against retail:

- No cross-fade between an outgoing and an incoming clip. The client's draw
  path blends over 200 ms; a shot poses one instant with the incoming clip at
  full weight.
- The aim layer is the client's `apply_aim`, whose per-bone weights are a
  stand-in: `BG_Player_DoControllers`' own constants are not decoded
  (`docs/research/player-model-anim-system.md`). It is fed the victim's view
  pitch as the torso pitch and zero as the waist pitch, since this server
  sends neither `fTorsoPitch` nor `fWaistPitch` and there is nothing better to
  read.
- A client whose models will not load has no rig, and its hits carry `none`,
  which the shipped multiplier table reads as full damage. Retail has no such
  state.

#### What the sweep measured

`--probe-sweep` (`crates/client/src/probe.rs`) taps once per entry of a
table of pitch offsets around the aim at the target's eye and echoes the
offset on its `!trace` line; the retail server logs `sHitLoc` per hit in its
`games_mp.log`, a `D;` record for a wound and a `K;` record for the kill, and
each hit the target's own trace registers pairs with the shooter's snapshot
at or before it. Run 2026-09-05 against retail: mp_carentan tdm,
`scr_friendlyfire 1`, both probes allies, `m1carbine_mp` from the hip,
shooter and target standing on the same floor, 24 taps, 16 hits over six
lives. The height each bullet crossed the target's origin plane at is
`shooter eye - range * tan(pitch)`, the eye 60 up, taken from the
playerstate's `viewangles`, which the server echoes a snapshot late at
times, so a height is good to one table step (1 to 4 units at these ranges)
plus the 1.5 degree hip cone. VERIFIED, the labels and the numbers;
INFERRED, the pairing of each hit with a tap.

The first hit of each life, the victim idle in `legsAnim` 634 and not yet
knocked back:

| range | pitch | units above the victim's origin | retail |
|---|---|---|---|
| 112 | -3.4 | 65.8 | `head` |
| 36 | -2.4 | 60.6 | `head` |
| 37 | 0.6 | 58.7 | `head` |
| 36 | 1.6 | 58.1 | `torso_upper` |
| 37 | 10.6 | 52.2 | `torso_upper` |
| 37 | 22.6 | 43.7 | `torso_upper` |

Five taps above that, 67.8 to 83.6 units at 111 range, all missed: the head
box tops out between 65.8 and 67.8. The idle `head` to `torso_upper` line
sits between 58.1 and 58.7, and **no tap in the run read `neck`**, at any
height, so a standing idle victim yields no neck from this bearing. Which
way the target faced is not in the capture: the target probe stands with
its spawn yaw, which the shooter walks up to from wherever tdm put it.

Every later hit in a life lands on a victim the first hit's knockback has
set moving: the target's own trace reads `legsAnim` 94 with `torsoAnim` 0 and
a velocity of 80 units a second on the frame after each hit, and 94 is
`pb_combatrun_forward_loop`, the animscript reading the knockback as a run.
No pain animation plays on either channel. The labels move down the body
with the running pose:

| range | pitch | units above the victim's origin | retail |
|---|---|---|---|
| 118 | -2.4 | 64.0 | `head` |
| 43 | -0.2 | 59.3 | `head` |
| 43 | 0.8 | 58.5 | `head` |
| 42 | 4.7 | 55.6 | `head` |
| 47 | 6.9 | 53.4 | `head` |
| 42 | 13.7 | 48.9 | `head` |
| 48 | 17.0 | 44.4 | `head` |
| 41 | 26.7 | 38.5 | `torso_upper` |
| 45 | 30.8 | 32.3 | `right_leg_upper` |
| 47 | 34.9 | 26.3 | `torso_lower` |

`head` at 44 units up, chest height on the idle model, and `torso_lower`
below a `right_leg_upper`: the ray is against the posed skeleton, and the
running pose carries the head lower and forward, where a descending ray
clips it before the torso and the head's priority takes the hit (3.1).
INFERRED from the two tables together; nothing here was read out of the
binary. vcod's own grafted rig says the same thing in numbers: posed with
`pb_stand_alert` the head box spans 55.8 to 70.3 units up and `back_up`
(`torso_upper`) 41.9 to 62.9, and posed with `pb_combatrun_forward_loop` at
200 ms the head box drops to 48.1 to 62.5. It is what a fixed table of
heights cannot reproduce, and it means a sweep on vcod must be read life by
life the same way.

The same sweep against vcod, same map, gametype, weapon and probes, the
shooter stopped at 111 units (the sweep walks to within 120 before it taps,
`SWEEP_RANGE`), the victim idle at every first hit and `legsAnim` 634 again
within two frames of each knockback:

| offset | units above the victim's origin | vcod | retail at the nearest height |
|---|---|---|---|
| -6 and above | 71.7 and up | miss | miss |
| -4 | 67.8 | `head` | miss at 67.8, `head` at 65.8 |
| -2 | 64.0 | `head` | `head` |
| -1 | 61.9 | `head` | `head` |
| 0 | 60.0 | `head` | `head` |
| 1 | 58.1 | `torso_upper` | `torso_upper` at 58.1, `head` at 58.7 |
| 2 | 56.1 | `torso_upper` | |
| 3 | 54.0 | `torso_upper` | |
| 4 | 52.2 | `torso_upper` | `torso_upper` |
| 6 | 48.2 | `torso_upper` | |
| 8 | 43.9 | `torso_upper` | `torso_upper` |

The idle `head` to `torso_upper` line lands in the same step on both (58.1
to 60.0 here, 58.1 to 58.7 on retail), neither side reads `neck` anywhere,
and the one difference is the top of the head: vcod's box reaches 67.8 and
retail's does not. The rig's own numbers put that box at 55.8 to 70.3 units
up in `pb_stand_alert`, so the top is 2 to 4 units higher than retail's, a
step and the hip cone. Not a finding yet; a finer sweep at a fixed short
range would make it one either way.

Two earlier runs against vcod read nothing like this, and what they found
is in 9.2: the server was playing the stock `pain` clause,
`pb_crouch_pain_holdStomach`, on every surviving hit and holding it for the
clip's 1.35 s, so every tap after the first met a doubled-over victim and
missed. Retail plays no pain animation on the server: both captures keep
`legsAnim` 634 through a surviving hit, bar the one frame the knockback
reads as a run.

What the retail run still holds that vcod does not reproduce: the `head`
reads at 44 to 49 units up on a victim hit 200 to 400 ms earlier (second
table above), where vcod reads `torso_upper` on an idle victim at 48.2 and
52.2. The retail victim was back in 634 by then, so a posed idle box does not
explain it; whether the server's pose cross-fades out of the knockback's run
frame over those 200 ms, or the echoed `viewangles` lag by more than a step
there, is open (section 10).

Retail's first head hit in each life killed on the second (67 + 67), so the
run holds two idle points per life at most; a target with more health
(`scr_*_maxhealth` is not a 1.1 cvar; a gametype script that sets
`self.maxhealth` would do) is what a finer idle table needs.

### 3.5 The multiplier table and what a missing file does

VERIFIED, `G_ParseHitLocDmgTable` `0x498b0`: a loop over indices 0 to 0x12
writes `1.0f` into `g_fHitLocDamageMult` (`0x16F080`, 0x4c bytes, one float
per hit location), and `0x49909` writes `0.0f` into the entry at `+0x48`.
INFERRED: the second store runs after the loop, so with no table loaded every
location does full damage except `gun` at index 18, which does none.

VERIFIED: it contains a `trap_FS_FOpenFile` on
`"info/mp_lochit_dmgtable"`, a `strncmp` against `"LOCDMGTABLE"`, a compare of
the length against `0x1FFF`, and a call to `Info_Validate`. VERIFIED: it holds
four error strings, `"Could not load hitloc damage table %s"`,
`"\"%s\" does not appear to be a hitloc damage table"`,
`"\"%s\" Is too long of a hitloc damage table to parse"` and
`"\"%s\" is not a valid hitloc damage table"`, all four passed to
`Com_Error` with the literal 1, plus `"Error parsing hitloc damage table %s"`
passed to `G_Error`. INFERRED: each string is on the failing arm of the check
it names, so a bad or missing file reaches `Com_Error` and none of the four is
a soft fallback. INFERRED: a server whose paks lack the file therefore does not
start, and the all-ones default is only ever the state the parser overwrites.

VERIFIED, the shipped `info/mp_lochit_dmgtable` in `pak5.pk3`: `none 1,
helmet 1.5, head 1.5, neck 1.5, torso_upper 0.9, torso_lower 0.8,
right_arm_upper 0.6, right_arm_lower 0.5, right_hand 0.4, left_arm_upper 0.6,
left_arm_lower 0.5, left_hand 0.4, right_leg_upper 0.6, right_leg_lower 0.5,
right_foot 0.4, left_leg_upper 0.6, left_leg_lower 0.5, left_foot 0.4,
gun 0`, behind the `LOCDMGTABLE` header, as an Info string. `45 * 1.5`
truncated is the 67 of 8.4. As implemented: `HitLocTable::load` reads it at
map load and keeps the all-ones default on a missing or malformed file,
where retail stops.

VERIFIED: the parse spec it builds is 19 records of
`{ name, offset = i*4, type = 6 }` against `g_fHitLocDamageMult`, and type 6
is `float` in the same type vocabulary the weapon-def table uses.

---

## 4. `G_Damage`, `G_DamageClient` and the script callback

### 4.1 Means of death

VERIFIED: the name table is the 25 pointers at `0x7DDA0`, and
`Scr_PlayerDamage` (`0x5ca90`) compares the `mod` argument against 0x18,
indexes the table with it, and holds the literal `"badMOD"` (`.rodata
0x78a80`) as a second string. INFERRED: `"badMOD"` is what a `mod` above 0x18
selects.

| index | name | index | name |
|---|---|---|---|
| 0 | `MOD_UNKNOWN` | 13 | `MOD_DYNAMITE` |
| 1 | `MOD_PISTOL_BULLET` | 14 | `MOD_DYNAMITE_SPLASH` |
| 2 | `MOD_RIFLE_BULLET` | 15 | `MOD_AIRSTRIKE` |
| 3 | `MOD_GRENADE` | 16 | `MOD_WATER` |
| 4 | `MOD_GRENADE_SPLASH` | 17 | `MOD_SLIME` |
| 5 | `MOD_PROJECTILE` | 18 | `MOD_LAVA` |
| 6 | `MOD_PROJECTILE_SPLASH` | 19 | `MOD_CRUSH` |
| 7 | `MOD_MELEE` | 20 | `MOD_TELEFRAG` |
| 8 | `MOD_HEAD_SHOT` | 21 | `MOD_FALLING` |
| 9 | `MOD_MORTAR` | 22 | `MOD_SUICIDE` |
| 10 | `MOD_MORTAR_SPLASH` | 23 | `MOD_TRIGGER_HURT` |
| 11 | `MOD_KICKED` | 24 | `MOD_EXPLOSIVE` |
| 12 | `MOD_GRABBER` | | |

VERIFIED: `MOD_MELEE` at 7 is the literal `push 0x7` `Weapon_Melee` passes to
`G_Damage` (`0x6884d`), which is the independent check that the index order
above is the enum order.

### 4.2 `G_Damage`

VERIFIED, `G_Damage` `0x49dac`, argument slots read off its own uses and off
the call in `Bullet_Fire_Extended`:
`(targ, inflictor, attacker, dir, point, damage, dflags, mod, hitLoc)` at
`+8`, `+0xC`, `+0x10`, `+0x14`, `+0x18`, `+0x1C`, `+0x20`, `+0x24`, `+0x28`.

**When `targ` has a client** (`gentity+0x158` non-null). VERIFIED: the
offsets, immediates and call targets named in the list below. INFERRED: the
numbering and every condition in it.

1. Return unless `targ->takedamage` (`gentity+0x171`) is non-zero.
2. Return when `client+0x21D8` or `client+0x21DC` is non-zero. INFERRED: those
   are `noclip` and `ufo`, from `shared.h`'s `gclient_s`, whose two
   `qboolean`s land exactly there once `sess.maxHealth` is pinned at
   `client+0x2150` by `cod11-gsc-object-model.md`.
3. Return unless `client+0x20EC` equals 2. INFERRED: that is
   `sess.connected == CON_CONNECTED`, on the same `shared.h` reading, which
   puts `sess.connected` seven dwords past `sess.sessionState` at
   `client+0x20D0`.
4. `damage = (int)(damage * g_fHitLocDamageMult[hitLoc])`, truncated toward
   zero by an explicit `fldcw` of `0xC00`.
5. The weapon passed on is `inflictor->s.weapon` (`entityState+200`), or
   `attacker->s.weapon` when there is no inflictor, or 0 when there is
   neither.
6. `Scr_PlayerDamage(targ, inflictor, attacker, damage, dflags, mod, weapon,
   point, dir, hitLoc)`, and then `G_Damage` returns.

**So for a player the engine does nothing else.** VERIFIED: the client branch
contains no store to health, no store to velocity and no call to a pain or die
function, and `0x49e67` is a `jmp` to the function's epilogue at `0x4a08b`.
INFERRED:
health, knockback, pain and death for players are all the script's, through
`finishPlayerDamage` (4.4).

**When `targ` has no client.** VERIFIED: the offsets, immediates, strings and
call targets named in the list below. INFERRED: the ordering, and every "when"
and "otherwise" in it.

- A null inflictor or attacker is replaced by a fixed entity.
- `s.eType == 5` takes a separate branch that notifies script and calls
  `targ+0x210` with `(targ, inflictor, attacker)`.
- Otherwise the direction is normalised, `targ->flags & 1` returns, a
  non-positive damage is raised to 1, `g_debugDamage` prints
  `"target:%i health:%i damage:%i\n"`, `targ->health` (`gentity+0x230`, the
  same offset `cod11-gsc-object-model.md` gives the `health` script field) is
  decremented, and script is notified with two arguments.
- With health still positive it copies the direction into
  `targ+0x280..0x288` and the point into `targ+0x1BC..0x1C4` and calls
  `targ+0x214` as `pain(targ, attacker, damage, point, mod, dir, hitLoc)`.
- With health at or below zero it clamps health to -999, notifies script,
  stores the attacker in `targ+0x258` and calls `targ+0x218` as
  `die(targ, inflictor, attacker, damage, mod, weapon, dir, hitLoc)`.

### 4.3 `G_DamageClient`

VERIFIED, `G_DamageClient` `0x4aa38`: it is the client branch of `G_Damage`
with the argument order shifted -- `(targ, inflictor, attacker, dir, point,
damage, dflags, mod, hitLoc)` at `+8`, `+0xC`, `+0x10`, `+0x14`, `+0x18`,
`+0x1C`, `+0x20`, `+0x24`, `+0x28` -- and the same five guards, the same
multiplier and the same `Scr_PlayerDamage` call. INFERRED: it exists so a
caller that already knows the target is a player can skip the branch, and it
changes nothing.

### 4.4 `Scr_PlayerDamage` and the callback signature

VERIFIED, `Scr_PlayerDamage` `0x5ca18`: its nine pushes run in the instruction
order `G_GetHitLocationString(hitLoc)` as a const string, `dir` as a vector or
undefined, `point` as a vector or undefined,
`BG_GetInfoForWeapon(weapon)->name` as a string, the means-of-death name as a
string, `dflags` as an int, `damage` as an int, `attacker` as an entity or
undefined, and `inflictor` as an entity or undefined. VERIFIED: `0x5cb15`
calls `Scr_ExecEntThread(self, g_scr_data+0x18, 9)`.

INFERRED: gsc reads pushed arguments in reverse, so the callback is
`CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags,
sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)`, which is the signature the
shipped `maps/mp/gametypes/_callbacksetup.gsc` declares.

### 4.5 `finishPlayerDamage`, which is where the damage lands

VERIFIED: the script method `finishplayerdamage` is `0x4376c`, entry 25 of the
player method table (`python3 tools/re/dump_builtins.py game.mp.i386.so all`).
VERIFIED: it takes the same nine arguments, resolving the means of death with
`G_IndexForMeansOfDeath`, the weapon with `BG_GetWeaponIndexForName` truncated
to a byte, and the hit location with `G_GetHitLocationIndexFromString`.
VERIFIED: it compares `iDamage` against 0 and jumps to its epilogue.
INFERRED: it returns without doing anything when `iDamage <= 0`.

**Knockback**, `0x43945` onward. VERIFIED: the offsets, immediates, constants
and cvar named in the list below. INFERRED: the ordering, and every "when",
"otherwise" and "skipped" in it.

- It is skipped when `self->flags & 8` (`gentity+0x17C`) or when
  `iDFlags & 4`.
- The stance scale is `0.02` for `pm_flags & 1`, `0.15` for `pm_flags & 2` and
  `0.3` otherwise (`.rodata 0x731ec`, `0x731f0`, `0x731e8`).
- `knockback = (int)(iDamage * scale)` truncated toward zero, clamped down to
  60, and the rest is skipped when it is 0.
- The velocity added is `normalize(vDir) * (knockback * g_knockback / 250.0)`,
  with 250.0 at `.rodata 0x731f4` and `g_knockback` defaulting to `"1000"`.
- When `ps->pm_time` (`ps+0x10`) is 0 it is set to `knockback * 2` clamped to
  `[50, 200]` and `pm_flags` gets `0x200` ORed in.

INFERRED: with the stock `g_knockback` the push is four units per second per
point of `knockback`, so a 45-damage carbine hit on a standing player adds
`(int)(45 * 0.3) = 13`, that is 52 units per second along the shot direction,
and a prone player takes `(int)(45 * 0.02) = 0` and is not moved at all.

**Impact feedback**, `0x43a5b` onward. VERIFIED: the offsets, immediates,
event numbers and call targets named in the list below. INFERRED: the
ordering, and every "with", "when" and "otherwise" in it, which are branch
conditions.

- With `self->flags & 1` set the function returns.
- With a non-zero weapon whose `weaponDef->weaponType` is 0 it spawns two temp
  entities at `vPoint`.
- The first carries event `0xAD` or `0xAE` (`EV_BULLET_HIT_SMALL` /
  `EV_BULLET_HIT_LARGE`), chosen on a test of `rifleBullet`, with `eventParm`
  and `entityState+216` both `DirToByte(normalize(vDir))`, `surfType` set to
  the literal 7, `otherEntityNum` set to the attacker's entity number,
  `gentity+0xF5` ORed with `0x20` and `gentity+0xF8` set to the victim's
  `ps->clientNum`.
- The second carries `0xAF` or `0xB0` (`EV_BULLET_HIT_CLIENT_SMALL` /
  `EV_BULLET_HIT_CLIENT_LARGE`), with the same `surfType` 7 and
  `otherEntityNum`, `entityState+144` set to the victim's `ps->clientNum`, and
  `gentity+0xF4` set to `0x800`.

INFERRED: the two `gentity+0xF4` writes are the entity-shared visibility flags
and the pair sends the plain impact to everybody but the victim and the
client-flavoured one to the victim alone.

**Health, and the order**, `0x43b7e` onward. VERIFIED: the offsets,
immediates, event numbers and call targets named in the list below.
INFERRED: the ordering, and every "with", "when" and "otherwise" in it, which
are branch conditions.

- `client+0x2214 += iDamage`.
- `client+0x2218..0x2220` takes `normalize(vDir)` with `client+0x2224` set to
  0, and takes `self->r.currentOrigin` with `client+0x2224` set to 1 on the
  other arm of the test of the `vDir` argument.
- `self->health (gentity+0x230) -= iDamage`.
- Script is notified with the attacker and the damage.
- With health still positive it calls `self+0x214` as
  `pain(self, attacker, iDamage, vPoint, mod, dirNorm, hitLoc)`.
- With health at or below zero it clamps to -999, stores the attacker in
  `self+0x258`, and calls `self+0x218` as
  `die(self, inflictor, attacker, iDamage, mod, weapon, dirNorm, hitLoc)`.
- It copies `self->health` into `ps->stats[0]` (`ps+0xF4`).

INFERRED: the `ps->stats[0]` copy is the last of these, and the two arms of the
`vDir` test are as listed.

INFERRED: so the answer to "does the engine subtract health or does
`finishPlayerDamage`" is that `G_Damage` hands the whole decision to script
for a player and `finishPlayerDamage` is where the subtraction, the knockback,
the impact events and the call into `player_die` all happen, in that order,
inside the callback.

VERIFIED: `client+0x2214`, `client+0x2218` and `client+0x2224` are read by
nothing else but `P_DamageFeedback` (section 6). INFERRED: they are the
per-frame damage accumulator, the direction it came from, and a flag saying
the direction is really an origin.

### 4.6 `CanDamage`

VERIFIED: `CanDamage` is `0x4a098` and the only call to it in the module is at
`0x4a601`, inside `G_RadiusDamage`. INFERRED: nothing on the bullet path
consults it.

---

## 5. `player_die` and the corpse clone

### 5.1 `player_die`

VERIFIED: `player_die` is `0x49a48` and reads stack slots `+8`, `+0xC`,
`+0x10`, `+0x14`, `+0x18`, `+0x1C`, `+0x20` and `+0x24`. INFERRED: those are
`(self, inflictor, attacker, damage, mod, weapon, dir, hitLoc)`, read off the
`die` call `finishPlayerDamage` makes through `self+0x218`.

VERIFIED: the offsets, immediates, constants and call targets named in the
list below. INFERRED: the numbering and every condition in it.

1. Return when `client->ps.pm_type > 5`.
2. Notify script with the attacker.
3. Replace `weapon` with `g_entities[attacker->s.otherEntityNum]->s.weapon`
   when `weapon` is non-zero, the attacker has a client whose
   `ps->eFlags & 0xC000` is set, and that entity's `s.eType` is 11.
   INFERRED: that credits a mounted MG's own weapon rather than the carried
   one.
4. `self+0x258 = attacker`.
5. When `client->ps.grenadeTimeLeft` is non-zero, `fire_grenade` from
   `self->r.currentOrigin` with z raised by 40.0 (`.rodata 0x743ec`), with a
   velocity built from three `rand()` calls and 160.0 (`.rodata 0x743e8`).
   Section 11.3 reads that arithmetic out in full; it is not the isotropic
   direction this step used to call it.
6. `BG_AnimScriptEvent(client, 1, 0, 1)`.
7. `G_AddEvent(self, 0xBD, 0)`. VERIFIED: `0xBD` is 189, `EV_DEATH`, and its
   event parm is the literal 0.
8. `Scr_PlayerKilled(self, inflictor, attacker, damage, mod, weapon, dir,
   hitLoc)`.
9. A walk over `level.maxclients` calling `Cmd_Score_f` for every connected
   client whose `sess.sessionState` is 2 and whose `spectatorClient`
   (`client+0x21D4`) is this entity's number. VERIFIED: the walk's stride over
   the client array is `0x22C4`, which is `sizeof(gclient_t)`.
10. `self->angles[2] = 0`; `self->takedamage = 1`; `self+0x118 = 0x4000000`.
    INFERRED: `self+0x118` is the entity's contents mask and `0x4000000` is
    the corpse contents, on the strength of `player_die` writing it between an
    unlink and a link and of Quake III Arena's `CONTENTS_CORPSE` having the
    same value.
11. `client->ps.stats[1]` (`ps+0xF8`) takes `(int)vectoyaw(attacker->origin -
    self->origin)`, or `(int)vectoyaw(inflictor->origin - self->origin)` when
    there is no usable attacker, or `(int)self->angles[1]` when there is
    neither. VERIFIED: the three stores. INFERRED: which arm runs, and
    reading `stats[1]` as the dead yaw, which is the label
    `docs/protocol-1.1.md` carries for it; the store is the strongest
    evidence there is for that reading and is still a reading.
12. `client->ps.viewangles` takes `self->angles`.
13. `self->s.loopSound = 0`, `trap_UnlinkEntity`, `self+0x114 = 30.0`
    (`.rodata 0x743f0`), `trap_LinkEntity`. INFERRED: `self+0x114` is the
    bounding box's `maxs[2]`, since it sits three floats past `self+0x108` and
    the clone copies `self+0x100..0x114` as two vectors.
14. `self->health = 0`; `self+0x218 = 0` (the die function pointer);
    `trap_LinkEntity` again.

**What `player_die` does not do.** VERIFIED: it contains no store to
`ps->pm_type`, no store to `ps->eFlags`, no store to `ps->deadViewHeight`
(`ps+0x348`) and no call to `G_SpawnPlayerClone`. INFERRED: the dead `pm_type`
and the corpse are the gametype script's job, through `self.sessionstate` and
the `clonePlayer` method.

### 5.2 `G_SpawnPlayerClone` and the body queue

VERIFIED: `G_SpawnPlayerClone` is `0x67888`. VERIFIED: the offsets,
immediates, arithmetic and call targets named in the list below. INFERRED:
the ordering, and every "when" and "before" in it.

- The slot is `&g_entities[64 + level->bodyQueIndex]`, computed as
  `level->gentities + index * 0x314 + 0xC500`, and `0xC500` is `64 * 0x314`.
- `level->bodyQueIndex` (`level+0x1DE8`) is advanced as `(index + 1) & 7`,
  written as `(index+1) - ((index+1) & ~7)` with the sign correction for a
  negative operand.
- The slot's `s.eFlags & 8` is read and inverted before anything else, and
  written back at the end.
- When the slot is in use, `G_FreeEntity` runs on it.
- The slot is marked in use, given a classname through `Scr_SetString`, given
  its own entity number, `+0x14C = 0x3FF`, and `+0x180` and `+0x184` cleared.

VERIFIED: **the body queue holds 8 entries**, from the `& 7` on the index.
INFERRED: the queue starts at entity 64, immediately past the 64 client slots.
INFERRED: the eFlags toggle guarantees the word differs from whatever the
previous occupant of this slot sent, which is the same restart trick the anim
channels use. INFERRED: the `G_FreeEntity` above is the only call that frees a
clone, since it is the only one on a path that reaches an entity in this
range.

The `cloneplayer` script method `0x4450C` is what fills it. VERIFIED: every
field and source in the table below, read off the store that writes it.

| clone field | source |
|---|---|
| `s.clientNum` (`entityState+144`) | `client->ps.clientNum` |
| `s.eFlags` (`entityState+8`) | `ps->eFlags & ~8`, plus the slot's toggled bit 8, plus `0x800` |
| origin | `G_SetOrigin(body, self->r.currentOrigin)` |
| angles | `G_SetAngle(body, self->r.currentAngles)` |
| `s.pos.trType` (`entityState+12`) | the literal 5 |
| `s.pos.trTime` (`entityState+16`) | `level.time` |
| `s.pos.trDelta` (`entityState+36`) | `client->ps.velocity` |
| `s.eType` (`entityState+4`) | the literal 2 |
| `s.groundEntityNum` (`entityState+124`) | the literal 1023 |
| `s.legsAnim` (`entityState+204`) | `client->ps.legsAnim` |
| `s.torsoAnim` (`entityState+208`) | `client->ps.torsoAnim` |
| bounds | `self+0x100..0x114` and `self+0x11C..0x130`, copied float by float |
| `r.svFlags` (`+0xF4`) | the literal `0x200`, `SVF_CAPSULE` |
| `physicsObject` (`+0x161`) | the literal 1 |
| `clipmask` (`+0x190`) | the literal `0x10001` |

Everything in that table is a **spawn-time** value. `pos.trType` 5,
`pos.trDelta` and `groundEntityNum` 1023 last one or two frames; what a client
is sent for the rest of the body's life is the settled trajectory 5.3
describes.

The three named offsets, anchored on CoDExtended's `struct gentity_s`
(`src/shared.h`) with `svFlags` at `+0xF4` putting `sizeof(entityState_t)` at
`0xE4`, and confirmed at their use sites:

- `+0xF4 = 0x200` is `r.svFlags` = `SVF_CAPSULE` (`CoDExtended/src/server.h:40`
  defines `SVF_CAPSULE 0x00000200`). VERIFIED: `G_RunEntity` 0x5034C/0x50355
  mirrors `s.eFlags & 0x10` into `+0xF5` bit `0x2` every frame, `G_RunItem`
  0x4EB8A and `G_BounceItem` 0x4E956 branch on the same `s.eFlags & 0x10` to
  pick `trap_TraceCapsule`, and a live client takes the identical
  `+0xF4 = 0x200` at 0x4258F.
- `+0x161 = 1` is `physicsObject` (`shared.h:942 char physicsObject;`, one
  byte past `inuse` at `+0x160`). VERIFIED: it has exactly one writer in the
  whole `.text`, this store, and three readers: `G_RunEntity` 0x503E8,
  `G_TryPushingEntity` 0x553F3 and a script-command check at 0x5D621.
- `+0x190 = 0x10001` is `clipmask`. VERIFIED: `G_RunItem` 0x4EB76 reads it as
  the trace mask and falls back to `0x491` when it is 0. The other writers are
  `0x2810011` (0x42732, client spawn), `0x81` (0x4DCA1) and `0x2802091`
  (missiles).

`+0x118`, the word between the two copied runs, is `r.contents`. VERIFIED:
`G_FreeEntity` ends in `bzero(ent, 0x314)` (0x66B97) and nothing on the clone
path writes the word, so a retail corpse has `r.contents == 0` and nothing
traces against it; the `+0x118 = 0x4000000` `player_die` writes at 0x49C28
goes on the dying player's own entity.

VERIFIED: `G_SetOrigin` 0x67D38 writes `pos.trBase` and then zeroes `trDelta`,
`trType`, `trTime` and `trDuration`, which is why the clone's own trajectory
stores come after it, and do. VERIFIED: `player_die` 0x49A48 has no store to
`client->ps.velocity` -- its only float stores through a client pointer are
`ps.viewangles` at 0x49D30/0x49D42/0x49D54 -- so the velocity the clone copies
at 0x445CA is the live one the dying player carried.

VERIFIED: `+0x118`, the word between the two copied runs, has no store.
VERIFIED: the function also calls `trap_LinkEntity` and `GScr_AddEntity` and
stores `level.time + 250` into `+0x1FC` and the address `0x456DC` into
`+0x200`. INFERRED: those four come after the table's stores, and skipping
`+0x118` is deliberate, since every float around it is copied one by one.

VERIFIED, the think at `0x456DC`: its entire body is
`ent->s.eFlags &= ~0x800`. INFERRED: the `0x800` bit set at spawn is the "this
body just died" marker, cleared 250 ms later so the client plays the death
animation once.

INFERRED: nothing frees a clone on a timer. It lives until the eighth
subsequent death reuses its slot, which is the eight-body behaviour the
project already observed live (`AGENTS.md`, "Netcode debugging").

**As implemented** (`crates/server/src/game/bodies.rs` and the `cloneplayer`
builtin in `crates/server/src/game/builtins/client.rs`). The clone is written
onto an empty entity state rather than a copy of the player's, and it carries
the table's `eType`, `clientNum`, `legsAnim`, `torsoAnim`, `eFlags` with the
slot's toggled bit 8 and the `0x800` marker, `groundEntityNum`, `pos.trType`,
`pos.trTime`, `pos.trBase`, `pos.trDelta` and `apos.trBase`, and is then
settled in place the way 5.3 describes, since there is no per-frame item
physics to settle it a frame later. The 250 ms think that clears `0x800` runs
in `ScriptRuntime::run_frame`, beside the object table's own thinks. What is
left out of the settle is the ground-plane re-orientation and the NODROP
free.

Where the state comes from is a two-step arrangement retail does not need.
A builtin cannot reach a client's sim, so `Server::replay_moves` mirrors each
player's entity state onto `GameHost::client_entity_states` before the script
frame, and `cloneplayer` copies the slot's entry. That copy is one step stale
by construction: the gametype clones the player before the death animation is
raised. So the queue records which client each body was born from, and
`Server::send_snapshots` re-reads a body born this frame from that client's
sim once, at the snapshot build, after the frame's sim ops have been applied.
A body born on an earlier frame is never re-read.

**As implemented**, `dropItem` (same file). VERIFIED that retail's
`PlayerCmd_dropItem` (`.so` 0x43684) resolves the name through
`BG_GetWeaponIndexForName` and calls `Drop_Weapon` (0x4dd40), which takes the
weapon off the player with `BG_TakePlayerWeapon` and hands the entity to
`LaunchItem` (0x4db98). VERIFIED that `LaunchItem` stores `+0x4 = 3`
(`ET_ITEM`), `+0x17c = 0x10` (the `eFlags` 16 the placed-weapon traces
carry), `pos.trType` 5, `pos.trTime = level.time`, and the entity's think as
`DroppedItemClearOwner` at `level.time + 1000`; VERIFIED that
`DroppedItemClearOwner` (0x4efb4) does nothing but write `0x3fe` into the
owner field, and that neither function arms a free. VERIFIED that the two
`0x7530` (30000) immediates in the module's `.text` are in `Cmd_CallVote_f`
and `fire_rocket`, so no dropped item is freed on a timer anywhere in it: a
retail drop lives until somebody picks it up.

vcod diverges on two of those, both deliberately. It does not take the weapon
off the player, because the stock death path re-gives the loadout on respawn
and nothing else calls the builtin yet. And it arms `ThinkFn::Free` 30 000 ms
out (`DROPPED_ITEM_MS`), because pickup on touch does not exist: with retail's
lifetime every death would leave an item that never goes away. The timer is a
placeholder for the touch path, not a claim about retail. The drop runs the
same `drop_item_to_floor` a placed weapon does, which stands in for the
`LaunchItem` launch and the `G_RunItem` settle behind it: the wire then reads
`pos.trType` 0 with `groundEntityNum` 1022, which is what all 133 item samples
in `crates/server/tests/fixtures/entities/` carry.

### 5.3 The corpse runs item physics, and settles

The clone is a `physicsObject` (`+0x161 = 1`), and that is what puts it
through the item physics every frame it exists.

VERIFIED, `G_RunFrame` 0x50478: the loop at 0x50930 walks
`i < level.num_entities` and calls `G_RunEntity` (0x502BC) for every `inuse`
entity (0x50955); 64..71 are inside `num_entities`. VERIFIED, `G_RunEntity`
0x502BC dispatch order: the framenum guard (0x502CB); `s.eFlags & 0x10`
mirrored into svFlags (0x5034C); `eType == 4` to `G_RunMissile` and `== 3` to
the item path; then 0x503E8 `cmp BYTE PTR [ebx+0x161], 0` with non-zero going
to `G_RunItem` (0x503F1); only then `eType` 5/8 to `G_RunMover`,
`ent->client` to `G_RunClient`, and last the bare think. So a clone, `eType`
2, reaches `G_RunItem` on its `physicsObject` byte alone.

VERIFIED, `G_RunItem` 0x4EB18:

- 0x4EB24: when `groundEntityNum == 0x3FF` and `pos.trType != 5`, `trType`
  becomes 5 and `trTime` the level clock -- airborne implies gravity.
- 0x4EB42: when `trType` is 0 or 8, run the think and return. A settled body
  costs one think a frame and nothing else.
- 0x4EB71: `BG_EvaluateTrajectory(&s.pos, level.time, newOrigin)`.
- 0x4EB76: `mask = ent->clipmask ? : 0x491`, which is where the clone's
  `0x10001` is used.
- 0x4EB8A: `trap_TraceCapsule` (or `trap_Trace`) from `r.currentOrigin` to
  `newOrigin` with the entity's own box, `passEntityNum = r.ownerNum`.
- 0x4EBED: `r.currentOrigin = trace.endpos`; 0x4EC19/0x4EC22
  `trap_LinkEntity` and `G_RunThink`.
- 0x4EC33: on `trace.fraction != 1`, a non-zero `trap_PointContents` frees the
  entity -- a body landing in a NODROP brush is deleted -- and otherwise
  `G_BounceItem` 0x4E858 runs.

VERIFIED, `G_BounceItem` 0x4E858, the settle:

- the reflection `trDelta = v + n * (v.n * -2.0)` (`.rodata` 0x74E44), then
  `trDelta *= ent->physicsBounce` (`+0x18C`) at 0x4E90B.
- 0x4E916: on startsolid, zero `trDelta` and re-trace 128.0 units straight
  down.
- 0x4E9BB `trace.plane.normal[2] > 0` and 0x4E9D1 `40.0 > trDelta[2]`
  (`.rodata` 0x74E4C) gate the settle itself:
  - 0x4EA02: `trace.endpos[2] += rand() * -2^-31 * 0.5 + 0.5`, a random lift
    in (0, 0.5].
  - 0x4EA13: `G_SetOrigin(ent, trace.endpos)`, so `pos.trType = 0`,
    `pos.trDelta = 0`, `pos.trTime = 0` and `pos.trBase` at the endpoint.
  - 0x4EA1F: `s.groundEntityNum = (u16)trace.entityNum`.
  - 0x4EA44-0x4EAA7: `AngleVectors`, two `CrossProduct` and `AxisToAngles`
    against the plane normal, then `G_SetAngle` -- the body is re-oriented
    flat to the ground.
  - 0x4EAB3: `trap_LinkEntity`.
- otherwise, still airborne (0x4EAC0): `r.currentOrigin += plane.normal`,
  `pos.trBase = r.currentOrigin`, `pos.trTime = level.time`.

INFERRED, from the multiply at 0x4E90B: a clone never bounces. It never writes
`physicsBounce` and `G_FreeEntity` bzeroed the slot, so the reflected velocity
is multiplied to zero and the first contact settles it. INFERRED, from the two
`G_RunItem` guards: once `trType` is 0 and `groundEntityNum` is not 0x3FF the
body never moves again.

VERIFIED, corroborating that `groundEntityNum` is a live physics field rather
than a static marker: `G_FreeEntity` 0x66B0E scans every entity and resets
`+0x7C` to `0x3FF` wherever it pointed at the freed one.

**The client does not settle anything.** VERIFIED, `cgame_mp_x86.dll`:
`CG_CalcEntityLerpPositions` (0x3001d210) sends only `trType == 1`, and
`trType == 3` with `number < 64`, to snapshot interpolation (0x3001d090);
everything else, a corpse included, is `BG_EvaluateTrajectory` on `pos` and
`apos` straight into `lerpOrigin` and `lerpAngles`. VERIFIED (negative): there
is no trace and no collision syscall anywhere in the `eType` 2 add path
(`CG_AddCEntity` 0x3001d5f0 case 2 is `FUN_30028400`; every call reachable one
level from it was enumerated and they are the DObj fetch, the anim update, an
`AnglesToAxis` and the refent submit). So a body left under `TR_GRAVITY` falls
on the client's parabola without bound -- 4 units after 100 ms, 400 after a
second -- and only the server's settle stops it.

**The corpse's animation is its own `legsAnim` and `torsoAnim`.** VERIFIED:
`FUN_30004e40` (0x30004e40), shared by the player and the corpse add, reads
`entityState+0xcc` and `entityState+0xd0` and pushes each into the legs
(record+0x37c) and torso (record+0x3ac) channels. There is no client-side
death-anim pick. VERIFIED: `FUN_30003be0` (0x30003c3e) derives `CG_SetAnim`'s
third argument as `entityState.eFlags >> 11 & 1`, and `CG_SetAnim`
(0x300036b0), for an anim whose table flags carry 0x40 -- the death flag,
OR'd in by the animscript parser at 0x30001f68 under the `DEATH` token --
branches on it: with the argument 0 it sets the animation and then calls trap
0x91 with 1.0, and with it non-zero it starts the animation over the entry's
blend time. VERIFIED: trap 0x91 is `FUN_004891f0` in CoDMP.exe and writes the
passed float into two dwords of the animation slot; INFERRED that the float is
the normalized playback position, so 1.0 is the clip's last frame. VERIFIED:
the same bit gates the DObj
anim-slot clone (trap 0xb8) in `CG_TransitionEntity`'s `eType` 2 arm
(0x3002f986), which copies the live player's animation slots onto the body.
INFERRED, the two arms: with `0x800` set, a fresh kill inside the server's
250 ms window, the death animation plays from the top; with it clear, the
animation is snapped to its last frame and held. A body sent without the
marker is therefore drawn at the last frame of its death animation from the
first frame it exists.

### 5.4 The dead player's own entity leaves the wire

Live, 2026-09-05 (mp_carentan tdm, one probe shooting a teammate with
`scr_friendlyfire 1`): the victim's `ET_PLAYER` entity is in the shooter's
snapshot on every frame up to the one that carries the kill, absent from
the one after it and from every frame of the dead wait, and back on the
respawn frame, six lives in a row. VERIFIED.

Where it goes, in `ClientEndFrame` (`game.mp.i386.so` 0x40e98; the
`sessionstate` branch structure is in `cod11-gsc-object-model.md`, "What
`ClientEndFrame` writes for a live client's own view"). VERIFIED: the stores
and compares named below. INFERRED: which arm a dead player takes, read off
the compares.

- `cmp [client+0x20d0], 1` at 0x41057, on the live arm after the own-body
  block. 1 is `STATE_DEAD` in CoDExtended's `sessionState_t`
  (`src/shared.h`: playing 0, dead 1, spectator 2, intermission 3), and the
  2 and 3 arms above it are the spectator and intermission ones the
  object-model doc already names.
- The equal arm, 0x41060..0x41093: `ps.pm_type` (`client+0x4`) takes 6, or 7
  when `gentity+0x2e4` is non-zero; `gentity+0x171` (`takedamage`) takes 0;
  and `r.svFlags` (`gentity+0xf4`) takes `| 0x1` then `& ~0x2`. `0x1` is
  `SVF_NOCLIENT` (`CoDExtended/src/server.h:38`), the flag the snapshot
  builder skips an entity on.
- Just above it, 0x40ff0..0x41012: the same `sessionstate == 1` compare
  writes `r.contents` (`gentity+0x118`) as 0, where a playing client takes
  `0x2000000`. A dead player therefore stops a trace no more than a corpse
  does (5.2), which is why 8.4's third round after the kill found nobody.
- The intermission arm (0x40ef0) sets the same `SVF_NOCLIENT` and clears
  contents; the own-body arm (0x40f9a) does the inverse, `| 0x2` and
  `& ~0x1`. So the flag is rewritten every frame from the session state, and
  a respawn clears it by setting the state back to playing, not by any store
  of its own.

`player_die` relinks the entity (5.1, step 13), so the death frame itself
still carries it; it is the end-of-frame pass that hides it, which is why the
capture reads one more frame with the entity, the kill's, and then none.

As implemented: `Server::send_snapshots` builds no entity for a client whose
sim is dead, the same filter that already drops spectators. The corpse's
newborn re-read (5.2) takes its state straight off the sim, since the dead
client has no entity in that list any more. Before this, a retail client was
sent a dead player standing in its death pose under the corpse until the
respawn, and `--probe-sweep`'s live-target gate saw a target to shoot for the
whole dead wait.

---

## 6. `P_DamageFeedback`

`P_DamageFeedback` is `0x3f500`. VERIFIED: the offsets, immediates, constants
and call targets named in the list below. INFERRED: the numbering and every
condition in it.

1. Return when `client->ps.pm_type > 5`.
2. Return when `client+0x2214` (the accumulated damage) is not positive.
3. Return when `client->sess.maxHealth` (`client+0x2150`) is not positive.
4. `count = damage * 100 / maxHealth`, integer, clamped down to 127.
5. `ps->aimSpreadScale += count`, clamped up to 255.0 (`.rodata 0x72c18`).
6. `kick = clamp(aimSpreadScale * 0.2, 5.0, 90.0)`, the three constants at
   `.rodata 0x72c1c`, `0x72c20` and `0x72c24`.
7. When `client+0x2224` is set: `client+0x2270 = 0`, `client+0x2274 = -kick`,
   `ps->damagePitch` (`ps+0xEC`) and `ps->damageYaw` (`ps+0xE8`) both take the
   literal 255, and `client+0x2224` is cleared.
8. Otherwise: `vectoangles(client+0x2218)` gives the world angles of the
   direction; `AnglesToAxis(ps->viewangles)` gives the victim's view axis;
   `client+0x2270 = -kick * dot(dir, axis[1])` and
   `client+0x2274 = kick * dot(dir, axis[0])`;
   `ps->damagePitch = (int)(angles[0] / 360.0 * 256.0)` and
   `ps->damageYaw = (int)(angles[1] / 360.0 * 256.0)`, both truncated toward
   zero, with 360.0 at `.rodata 0x72c28` and 256.0 at `0x72c2c`.
9. When `level.time > self+0x224` and `self->flags & 1` is clear:
   `G_AddEvent(self, 0xBB, clamp((int)(ps->stats[0] * 100.0 / ps->stats[2]),
   0, 100))` and `self+0x224 = level.time + 700`. VERIFIED: `0xBB` is 187,
   `EV_PAIN`, and its parm is the victim's health as a percentage of max.
10. `ps->damageEvent` (`ps+0xE4`) is **incremented**, not assigned.
11. `client+0x226C = level.time - 20`.
12. `ps->damageCount` (`ps+0xF0`) `= count`.
13. `client+0x2214 = 0`.

INFERRED: `client+0x2270` and `client+0x2274` are the view kick the server
applies itself, since nothing on the wire carries them.

**What `P_DamageFeedback` itself resets, and what it leaves standing.**
VERIFIED: the only value this function stores that resets anything is the 0
into `client+0x2214`; `damageEvent` takes an increment, `damageCount` takes
`count`, and `damageYaw` and `damagePitch` take either a scaled angle or the
literal 255. UNVERIFIED: whether anything else in the module writes the four
fields. VERIFIED: `objdump` finds three unresolved writes to `ps+0xE8` at
`0x31778`, `0x31826` and `0x31fc6` whose base register I did not identify, and
the writes to `+0xE4`, `+0xE8` and `+0xEC` in `BG_PlayerStateToEntityState`
(`0x2ceb0`) and `BG_PlayerStateToEntityStateExtrapolate` (`0x2d400`) are
`entityState` offsets rather than playerstate ones. INFERRED: the
client detects a new hit by `damageEvent` changing, so the fields keep their
last values between hits and a reader that waits for them to return to zero
waits forever.

VERIFIED: step 7's arm is the one that stores the literal 255 into both
fields, and its test is of `client+0x2224`, the field `finishPlayerDamage`
also stores 1 and 0 into alongside `client+0x2218`. INFERRED: the 1 goes with
the victim's own origin and the 0 with a real direction, so the 255/255 pair is
the sentinel for "the damage carried no direction".

**As implemented** (`ClientSim::take_damage` and `end_frame`,
`crates/server/src/spectate.rs`): steps 2 to 5 and 7 to 13 as listed, with
step 1 read as the sim's `dead` flag, which the killing hit sets before the
end-frame runs. Step 6's view kick is not carried; nothing on the wire reads
it. `EV_PAIN` is raised whatever the stance: nothing above names
`EV_CROUCH_PAIN` (188), so no crouch rule is modelled. Step 5's
`aimSpreadScale` add lands on the same playerstate field 2.1 moves and the
wire carries.

---

## 7. `G_AddEvent` and `G_TempEntity`

VERIFIED, `G_AddEvent` `0x67ca4`: it tests `gentity+0x158` and has two arms.
One writes `ps->events[ps->eventSequence & 3] = event` and
`ps->eventParms[ps->eventSequence & 3] = parm` and increments
`ps->eventSequence`; the other does the same against `entityState+168`,
`entityState+184` and `entityState+164`. VERIFIED: it holds two stores of
`level.time`, into `ent+0x180` and `ent+0x150`. INFERRED: both arms reach
them. INFERRED: the playerstate arm is the one a client entity takes.
INFERRED: this is the ring `player-model-anim-system.md` measured from the
outside, four slots, masked, with the counter incremented after the write.

VERIFIED: `G_TempEntity` is `0x67938`, and every call site above writes
`entityState` fields into its return value. INFERRED: it returns an entity the
callers fill in by hand.

---

## 8. What the hit capture measured

Two probes on one retail server, `--net-probe --probe-target` and
`--net-probe --save-hit`, on `mp_carentan`. The target stands still, sends the
`kill` client command every 45 s, presses use 3 s after each death and traces
its own playerstate; the shooter walks toward it and shoots. The fixtures are
in `crates/server/tests/fixtures/playerstate/`.

It takes two runs, because one gametype cannot give both halves:

- `mp_carentan-dm-hit-target.txt` and `-hit-shooter.txt` hold the **death
  half**. VERIFIED: in every deathmatch run the two probes stood 2000 to 4500
  units apart, the shooter's walk never closed it inside its 150 s limit, its
  fixture carries `# BROKEN no line of sight`, and the target's `damageEvent`
  never moves. INFERRED: the distance is deathmatch's spawn picker placing a
  respawning client away from the other one. VERIFIED: that pair was taken
  before the probe's aim was corrected, so its shooter `viewangles` column does
  not point at the target; nothing in the death half reads that column.
- `mp_carentan-tdm-hit-target.txt` and `-hit-shooter.txt` hold the **hit
  half**, captured with `+set g_gametype tdm +set scr_friendlyfire 1` and both
  probes on `allies`. VERIFIED: in team deathmatch the two spawned 143 to 563
  units apart across five runs. VERIFIED, read out of `tdm.gsc` in `pak5`: the
  damage callback returns without applying anything when
  `getCvarInt("scr_friendlyfire")` is `<= 0` and reflects the damage when it is
  `2`, so `1` is the value that lets a teammate's bullet through.

VERIFIED: the `!event` lines in all four committed fixtures carry only the
event entities (`event=201`). The playerstate and entity rings were read one
slot high when these were taken (`crates/common/src/net/events.rs`, fixed
since), so no `EV_DEATH` or `EV_PAIN` reached the `!event` list. The `events=`
columns on the `!trace` lines are the source for both, and are what everything
below quotes. VERIFIED, from a run taken after the fix and not committed as a
fixture: the same death now drains as `ev 189 parm 0` on the playerstate ring.

### 8.1 The death

VERIFIED: the `kill` command lands in one snapshot, ~50 ms after it goes out,
and not every one lands. The dm run sent five and three took, at 10 s, 100 s
and 190 s, with the two in between doing nothing at all. UNVERIFIED: what
refuses them.

VERIFIED, the frame one lands in:

| field | before | at the death |
|---|---|---|
| `stats[0]` (health) | 100 | 0 |
| `pm_type` | 0 | **6** |
| `eventSequence` | 0 | 2 |
| `events[0]`, `events[1]` | 0, 0 | **189** (`EV_DEATH`), **155** (`EV_RAISE_WEAPON`) |
| `eventParms[0]`, `eventParms[1]` | 0, 0 | 0, 0 |
| `legsAnim` | 634 | one of 18, 19, 22 in this capture, 16, 17, 20, 21 in others |
| `torsoAnim` | 0 | 512 |
| `weaponstate` | 0 | 0 or 1 |
| `eFlags` | 16 | 16 |
| `velocity` | 0, 0, 0 | 0, 0, 0 |

VERIFIED: `EV_DEATH` arrives with parm 0, which is the literal 5.1 reads out of
`player_die`. INFERRED: the `EV_RAISE_WEAPON` beside it is the same frame's
weapon work rather than part of the death, since nothing in `player_die`
writes the weapon channel.

VERIFIED: the dead `pm_type` is 6. INFERRED: it is the gametype script's, since
5.1 shows `player_die` writes no `pm_type` at all; either way it is the value a
server has to reproduce.

**`deadViewHeight` never moves.** VERIFIED: it reads 8 before the death,
through it and after the respawn, which is 5.1's "no store to
`ps->deadViewHeight`" seen from outside. VERIFIED: what moves is
`viewHeightCurrent`, 60 at the death frame, then 51, 42, 33, 24, 15, 8 over the
next six snapshots, reaching `deadViewHeight` ~300 ms later. INFERRED: that is
the ordinary view-height lerp running to the dead height rather than a
death-specific path, since the step per 50 ms snapshot is the same ~9 units the
stance lerp uses.

VERIFIED: `stats[1]` reads 0 on every death by `kill`. INFERRED: that is 5.1
item 11's `vectoyaw(attacker->origin - self->origin)` on a zero vector, since
the attacker is the victim; 8.4 has the case where it is not.

VERIFIED: the four damage fields stay 0 across every death by `kill`.

VERIFIED: the magazine goes with the death. `ammoclip` reads `3:7, 6:3, 10:15`
alive and `3:7, 6:3` dead, `ammo` reads `3:56, 10:400` alive and `3:56` dead,
and index 10 comes back at the respawn. UNVERIFIED: which weapon index 10 is.

### 8.2 The corpse

VERIFIED: every death puts a corpse in the body queue in the same snapshot as
`EV_DEATH`, and the queue is used in order from entity **64**. The dm run's
three deaths land in 64, 65 and 66; the tdm run's five deaths show four
distinct slots, 64 through 67, in that order. UNVERIFIED: why the fifth death
leaves no corpse edge at all. It is the 39527 ms one, whose whole playerstate
frame reads blank (8.3), so the capture shows neither what killed it nor what
became of its body.

VERIFIED: the first slot is 64, measured. INFERRED: the range is the eight
entities 64..71, which is 5.2's `& 7` on the index rather than anything this
capture reached; only 64 through 67 were ever occupied here.

VERIFIED: corpses do not expire on a timer here. The dm run's first corpse is
still in the body queue at the end of the run, 190 s after it died. VERIFIED:
in the tdm run one body vanishes and comes back with the same entity number and
clientNum, 65 at 30885 ms and 37535 ms. INFERRED: those two edges are PVS,
since the target had respawned elsewhere in between. UNVERIFIED: what frees a
clone, since neither capture shows one being freed.

### 8.3 The obituary and the respawn

VERIFIED: each death broadcasts one `EV_OBITUARY` (201) in the same snapshot.
A `kill` gives `otherEntityNum` 0 (the victim), `attackerEntityNum` 0 and
`eventParm` **150**, `0x96`, `MOD_SUICIDE`. VERIFIED: the tdm capture's one
measured bullet death gives the victim 0, the attacker 1 and `eventParm`
**136**, `0x88`, `MOD_HEAD_SHOT`. Both are rows of `cod11-hud-protocol.md` section 2's
table.

VERIFIED: the tdm run holds five deaths. Three are at the `kill` times with
`MOD_SUICIDE`, one is the measured bullet death at 28891 ms with
`MOD_HEAD_SHOT`, and one is at 39527 ms. UNVERIFIED: what killed that one. No
`kill` went out between 10004 ms and 55012 ms, and its frame reads
`damageEvent` 0, `damageCount` 0, `damageYaw` 0, `damagePitch` 0, `stats[1]` 0,
zero velocity, a cleared event ring, `legsAnim` and `torsoAnim` 0 and both ammo
arrays empty, so it carries neither the damage signature 8.4 measures nor a
`kill`'s.

VERIFIED: the fixture carries two `136` obituaries, at 28891 ms and 37535 ms,
both from event entity 182, and no death sits beside the second. INFERRED: it
is that temp entity re-firing rather than a second kill, since `EventTracker`
fires an event entity again when its slot leaves the snapshot and comes back
with the same contents. VERIFIED: the 39527 ms death has no obituary of its own
in the trace, so `!observed obituaries=5` is four deaths with one each, one
duplicate, and one death with none.

VERIFIED: the dm shooter, 2000 to 4500 units away with no line of sight,
receives every obituary. That is the `SVF_BROADCAST` `cod11-hud-protocol.md`
section 1 reads out of the builtin.

**The respawn.** The probe presses use 3 s after each death and again once a
second. VERIFIED, dm, three deaths: 4.04 s, 3.05 s and 3.04 s dead. VERIFIED,
tdm, five deaths: 3.06 s, **1.99 s**, 4.05 s, 4.05 s and 3.06 s dead. VERIFIED:
the 1.99 s one is shorter than the 3 s the probe waits before its first press,
so nothing the probe sent respawned it. INFERRED: a press that is taken is
taken within ~50 ms, so the ~3 s cases are the body being held for about that
long, and the two 4 s cases are a first press that did nothing followed by a
second one a second later that did. UNVERIFIED: what respawns a body with no
press, and what refuses the first one.

VERIFIED, the frame the player is alive again: `stats[0]` 100, `pm_type` 0,
`eventSequence` 0 with a cleared ring, `legsAnim` 634, `torsoAnim` 0,
`viewHeightCurrent` 60, and `eFlags` **24** where it read 16 before. INFERRED:
24 is 16 with bit `0x8` set, the anim-restart toggle 5.2 names on the clone, so
a respawn flips it the same way.

### 8.4 What a bullet does

From the tdm capture: `m1carbine_mp` (`damage` 45 in `weapons/mp/m1carbine_mp`)
aimed at a standing teammate's eye from 586 units. VERIFIED: the shooter spent
16 rounds across two magazines, its clip index 10 running 15 down to 1, back
to 15 and on to 13. VERIFIED: the 2nd and the 5th landed, the 5th killing, and
the obituary's `eventParm` is 136, `0x88`, `MOD_HEAD_SHOT`.

VERIFIED, **the hit frame** against the settled frame before it:

| field | before | on the hit |
|---|---|---|
| `stats[0]` (health) | 100 | 33 |
| `damageEvent` | 0 | 1 |
| `damageCount` | 0 | 67 |
| `damageYaw` | 0 | 64 |
| `damagePitch` | 0 | 255 |
| `eventSequence` | 0 | 1 |
| `events[0]`, `eventParms[0]` | 0, 0 | **187** (`EV_PAIN`), **33** |
| `velocity` | 0, 0, 0 | -1.4, 80.0, 0.1 |

**67 damage from a 45-damage weapon on a head hit.** VERIFIED: the shipped
`info/mp_lochit_dmgtable` in `pak5.pk3` gives the head 1.5 (3.5). INFERRED:
the product is truncated, since 45 * 1.5 is 67.5, 6.4's `damage * multiplier`
is the only scaling on this path, and nothing else in the capture is a factor
of 1.489.

VERIFIED: `damageCount` reads 67, the same number as the damage. INFERRED: this
capture cannot separate it from 6's `damage * 100 / maxHealth`, since
`maxHealth` is 100 here; a server with another max health would.

**`damageYaw` and `damagePitch` are the world angles of the direction the
damage came along, from the attacker toward the victim.** VERIFIED: the shooter
stood at `(1810.0, 2109.5)` and the target at `(1800.0, 2696.0)`, a bearing of
91.0 degrees, and `(int)(91.0 / 360 * 256)` is 64, which is what `damageYaw`
reads. VERIFIED: both eyes sat at the same height and `damagePitch` reads 255,
one short of the 256 a level shot wraps to. INFERRED: that is 6's step 8, since
the numbers match its `angles / 360.0 * 256.0` on that bearing.

**The knockback is one frame of velocity along that direction.** VERIFIED: the
target stood still, and the hit frame reads `-1.4, 80.0, 0.1`, 80 u/s along the
bearing the bullet travelled, decaying to 64, 46, 28 and 0 over the next four
snapshots. INFERRED: the decay is ordinary ground friction rather than a
knockback timer, since the target held no movement input.

VERIFIED: `EV_PAIN`'s parm is 33, the victim's health as a percentage of max
after the damage, which is the number 6's step 9 computes.

**The fatal hit leaves `damageEvent` alone.** VERIFIED: the second bullet took
health 33 to 0 and `damageEvent` stayed 1, with `damageCount`, `damageYaw` and
`damagePitch` keeping the first hit's values. INFERRED: that is 6's step 1, the
`pm_type > 5` return, so the feedback for the killing hit never runs.

VERIFIED: a death by a bullet carries the same `pm_type` **6** as a death by
`kill`, with `legsAnim` 17, `torsoAnim` 512 and events `[187, 189, 155]` in one
frame.

VERIFIED: `stats[1]` reads **270** on the bullet death, where every other death
in the two captures reads 0, and the shooter sat at bearing 270.98 degrees from
the target.
INFERRED: it is 5.1 item 11's `vectoyaw` truncated toward zero, and it is the
opposite of `damageYaw`'s direction, which the two numbers agree on: 64 is 90
degrees and 270 is 90 + 180.

VERIFIED: the four damage fields read 0 again after the respawn.

---

## 9. What vcod's own server does, measured the same way

The retail-client hand check that this stage's plan called for -- a 1.1
client joining `vcod-server` through the menus and shooting and being shot --
ran twice. On 2026-09-03 it found six defects (deaths column, ADS during a
reload, ADS and crosshair twitch, head shots reading as body hits, an inert
grenade twitching, and a death cluster: no death animation, a sinking suicide
corpse, no respawn text). On 2026-09-04, with the fixes in, a retail client
under Wine against `vcod-server mp_carentan --gametype tdm --set
scr_friendlyfire=1` and a `--save-hit` probe as the teammate: the suicide
corpse played its death animation and lay on the ground, the respawn text
appeared and the use key respawned, and the client's shot on the probe logged
`D;...;head` and a `MOD_HEAD_SHOT` obituary; the person at the keyboard
confirmed the ADS and reload behaviour by eye. What follows is the headless
measurement: the same two probes that took section 8's retail captures,
pointed at `cargo run -p vcod-server` instead. Every claim below is evidence
about vcod, not about retail; the retail column is section 8's committed
fixtures.

Three runs, 2026-09-03. `mp_carentan` `dm` for 200 s took the settled state
and the weapon channel. `mp_ship` `dm` for 155 s took the kill by a bullet:
carentan is too large for the shooter's walk to cross, exactly as it was for
the retail `dm` capture, and mp_ship is small enough that the approach closed
to 572 units and found a line of sight. A third run on `mp_carentan` `dm`
retook the death half after 9.2's divergences were closed, which is the
capture 9.1 quotes. None is committed; every fixture pair was moved out of the
tree after its run, because the fixture names are the retail evidence's.

### 9.1 What matches

VERIFIED, `mp_carentan` `dm`, the settled standing pose, field for field
against section 8's `mp_carentan-dm-hit-target.txt`: `health` 100, `pm_type` 0,
`eFlags` 16, `deadViewHeight` 8, `viewHeightCurrent` 60.0, `legsAnim` 634,
`torsoAnim` 0, `weaponstate` 0, `stats[1]` 0, `damageEvent`/`damageCount`/
`damageYaw`/`damagePitch` all 0, `eventSequence` 0 with an empty ring, and both
array blocks: `clip=3:7,6:3,10:15 ammo=3:56,10:400`.

VERIFIED, the same map, the `kill` client command's death frame against
retail's, field for field: `health` 0, `damageEvent`/`damageCount`/`damageYaw`/
`damagePitch` 0, `pm_type` 6, `eFlags` 16, `deadViewHeight` 8,
`viewHeightCurrent` 60.0, `legsAnim` 18, `torsoAnim` 512, `weaponstate` 0,
`stats[1]` 0, `clip=3:7,6:3` and `ammo=3:56` -- the held carbine's index 10
gone from both arrays -- and zero velocity. The one field that differs is
`eventSequence`, 1 against retail's 2, for the reason 9.2's last entry gives.

VERIFIED, the respawn frame against retail's, field for field: `health` 100,
`pm_type` 0, `eFlags` **24**, `eventSequence` 0 with an empty ring, and
`clip=3:7,6:3,10:15 ammo=3:56,10:400`. The one field of that frame that
differs is `legsAnim`, in 9.2.

VERIFIED: over a 130 s run of three deaths and three respawns, `eFlags`
alternates 16 and 24 the way retail's captures do (38 samples against 51 here,
115 against 101 in retail's `dm` capture).

VERIFIED: the three corpses take entities **64**, **65** and **66** in order,
each on its death frame with `trType` 5, and none vanishes -- section 5.2's
rule, retail's `dm` capture's 64/65/66/67, and no lifetime timer.

VERIFIED: the obituary reaches the probe on all three deaths as
`victim=0 attacker=0 parm=150`, which is retail's own `dm` line
(`!obituary ms=10037 victim=0 attacker=0 parm=150`): `MOD_SUICIDE` is index 22
and one of the seven the `0x80` flag covers.

VERIFIED, the shooter's single shot on `mp_carentan`: `weaponstate` 3,
`weapAnim` 514, one `EV_FIRE_WEAPON` (159) in the ring, and the clip index 10
counting 15 down to 14 -- the same four the retail shooter capture reads.
`torsoAnim` reads 765 where the retail `dm` shooter fixture reads 0; that
fixture predates the `cmd.weapon` fix and its torso channel is the holstered
artifact (`player-model-anim-system.md`, "The `jump_takeoff` torso index is
gone"), so 765 = 512 | 253 is the number the retaken `combat` fixtures measure
and the 0 is not evidence.

VERIFIED, `mp_ship` `dm`, the kill by a bullet: `health` 0, `pm_type` 6,
`torsoAnim` 512, and the dead view height lerping 60 -> 50.8 -> 42 -> 32.8 ->
24 -> 12.1 -> 8 against retail's 60 -> 51 -> 42 -> 33 -> 24 -> 15 -> 8.
VERIFIED: `legsAnim` reads 22 where the two retail deaths read 17 and 18, and
all three are indices of the standing `death` clause, whose several anims
`player-model-anim-system.md` reads as a random draw. VERIFIED: `stats[1]`
reads 294 and the shooter stood at bearing 294.42 degrees from the target when
it fired, which is 5.1 item 11's `vectoyaw` truncated toward zero.
VERIFIED: `damageEvent` stays 0 through the fatal hit. INFERRED: that is 6's
step 1, the `pm_type > 5` return.

VERIFIED: a bullet kill's obituary reaches both probes as
`victim=0 attacker=1 parm=5`. INFERRED: 5 is the killer's configstring 7
weapon index rather than `0x80 | mod`, which is section 4.1's rule for a
`MOD_RIFLE_BULLET`, one of the means of death the `0x80` flag does not cover.

VERIFIED: the victim respawns on the use key with the full spawn loadout, and
the killer's scoreboard row goes from score 0 to score 1 on the frame after
the kill.

### 9.2 What differs

VERIFIED: **the death frame is missing `EV_RAISE_WEAPON`.** Retail's death
raises two events, 189 then 155, and reads `eventSequence` 2; vcod raises 189
alone and reads 1. It is the one field of the death frame that still differs,
and it is in 9.4's list.

VERIFIED: **the respawn frame carries `legsAnim` 0 where retail carries 634.**
It is one frame: the next one reads 634 and every frame after it. INFERRED:
the animscript picks nothing until a move has run, so the spawn frame goes out
before the standing idle is chosen, where retail's already carries it.

Open, both found on 2026-09-06 by the `--save-ads` capture on
`kar98k_sniper_mp` (`mp_carentan-tdm-ads-sniper`, the one that answered the
"the scope twitches like crazy" hand-check report). Each is gapped in
`playerstate_combat_ab`'s `KNOWN_GAPS`, which asserts the gap still applies,
so both fail the run the moment they are fixed:

- **A scoped shot leaves a rechamber retail does not run.** VERIFIED: the
  `ads_release` step that follows the shot reads `weaponstate` 0 and 9 with
  `weaponDelay` 0 throughout on retail, and on vcod holds `weaponstate` 5 for
  12 of the step's samples with a 175 ms `weaponDelay`, writing the
  rechamber's `weapAnim` 11 and 13 with it; `ads_shot` itself carries a
  `torsoAnim` retail's does not. INFERRED: the bolt-action rechamber the
  `kar98k_sniper_mp` file asks for is being run on a path retail's sight does
  not take it down. The gap is `RECHAMBER_GAP`; the fix is in
  `vcod_common::pmove::weapon`.
- **The ground trace drops a walking player for a frame where retail never
  does, and the sight ramp reverses with it.** VERIFIED: replaying the
  capture from its own spawn, retail reads `groundEntityNum` 1022 on all 31
  samples of `ads_walk` and vcod reads 1023 on one. VERIFIED: across the
  whole capture taken against vcod at *its* own spawn there is no such
  sample in 351, so the defect is position-dependent, not a constant.
  INFERRED: `PM_UpdateAimDownSightFlag` clears the sight for an airborne
  frame (1.13), so `advance_ads` ramps down by `msec / adsTransOutTime` for
  that cmd and up for the next -- -0.0625 then +0.083 at the 25 ms cmds the
  replay sends -- which is why the fraction moves 0.333 to 0.354 across a
  frame it should have moved 0.167. INFERRED: on a weapon with `adsZoomFov`
  16 a client re-basing its zoom prediction off that reads as the scope
  twitching, which `m1carbine_mp`'s 65 would hide. The gap is
  `ADS_WALK_GAP`; the suspect is `pmove::ground_trace`, a bare 0.25-unit box
  trace with no hysteresis (`crates/common/src/pmove.rs`).

Closed on 2026-09-05, off the `--probe-sweep` runs (3.4):

- **A dead player's entity stayed on the wire.** Retail drops it on the
  frame after the kill and brings it back on the respawn (5.4); vcod sent it
  through the dead wait, standing in the death pose under its own corpse.
  Fixed: `send_snapshots` builds no entity for a dead sim.
- **A surviving hit played a pain animation.** vcod raised the animscript's
  `pain` event, whose only live clause for a standing player is the default
  `both pb_crouch_pain_holdStomach`, and held it 1.35 s; retail's server
  raises no pain anim at all, and both captures keep `legsAnim` 634 through
  a hit. The held clip doubled the victim over under the bone trace, which
  is why the first two vcod sweeps missed seven and eight taps in a row at
  head height. Fixed: no pain anim on the server; what a retail client draws
  for another player's `EV_PAIN` is its own business.

Everything else section 9 found on 2026-09-03 has since been closed, each
against the fixture line that decided it: the frag's reserve entry (its file
reads `clipOnly 1`, so a give writes the clip and no reserve), the held
weapon's clip and reserve surviving the death (the drop takes the rounds with
it), the `eFlags` `0x8` toggle, the event ring surviving a respawn, the
missing `kill` client command, and the `dm` scoreboard's two team totals,
which retail sends as `0 0` rather than the `-9999` sentinel a gametype has to
write for itself.

### 9.3 What this run could not reach

The one-shot kill on mp_ship means **no non-fatal hit was observed live**, so
`damageEvent`'s increment, `damageCount`, `damageYaw`/`damagePitch` against a
bearing, the `EV_PAIN` parm, the knockback velocity and the hit-location
multiplier are unmeasured against a running server. All of them are pinned
in-process instead by `crates/server/tests/combat.rs`, which reproduces
section 8.4's 33 health, 67 `damageCount`, `EV_PAIN` parm 33 and the
`MOD_HEAD_SHOT` obituary from two clients on one server.

Of what a person has to look at, the 2026-09-04 hand check (section 9's
opening) covered the death anim, the corpse, the respawn text and key, the
ammo counter, the reload key and the ADS and crosshair behaviour under
prediction. Still open: whether the shooter sees its own muzzle flash and
hears its fire sound, whether the damage direction indicator points the right
way, and a kill by another player seen from the victim's side on a retail
client (the check's teammate probe was shot before it could shoot; the
in-process test and the headless run cover that path). PENDING.

### 9.4 What is not modelled at all

Named here so a reader of sections 1 to 7 does not assume the code follows
them: rifle rounds passing through a player at half damage (2.3); the
`pm_time` stun (4.5); the view kick of 6's step 6; events 175 and 176;
`EV_CROUCH_PAIN` (188);
the `EV_RAISE_WEAPON` (155) retail raises on the death frame beside `EV_DEATH`;
the direct-hit `MOD_GRENADE` arm (13.1), which a stock frag cannot reach
because its file spells `damage` 0; the pitch rate `G_MissileLandAngles`
redraws at a bounce (11.2); the splash event 173 and the water mask of 12.1;
and item pickup.

Melee (1.10, 2.5) and grenades (1.11, 11 to 14) were absent from the run 9.1
to 9.3 measured, and the radius-damage falloff vcod carried then was RTCW's
curve. Both are modelled now, 14.1's falloff with them, and 9.5 is what the
probes measured of the two. `CanDamage`'s line-of-sight check (4.6, 14.3) and
`setPlayerIgnoreRadiusDamage` (14.2) moved off this list with them.

The ADS fraction and the spread scale used to be here. Both are retail's now:
1.13 and 2.1 carry the rules with their addresses and each an "As
implemented" note, and `spread_deg`
(`crates/server/src/game/combat.rs`) and the `fWeaponPosFrac > 0.75` anim
tests of 1.2 and 1.9 read the fraction rather than the usercmd's sight bit.
What is still not modelled of the two: `pm_flags 0x400`, and the `pm_time`
and `eFlags 0xC000` arms the fraction shares with the rest of `PM_Weapon`.

The ramp rates, the reload window, the fire add and the decay are pinned by
the `--save-ads` captures (1.13 and 2.1), and
`crates/server/tests/playerstate_combat_ab.rs` replays both against ours to
half a frame's tolerance and passes. The turn term is not: both fixture
weapons spell `hipSpreadTurnAdd 0`, and the BAR, the one stock weapon with
one, needs a probe that can answer the weapon menu with it.

### 9.5 Melee and grenades, measured the same way

Four runs on 2026-09-05, `mp_carentan` `tdm` with `scr_friendlyfire 1`, against
`cargo run -p vcod-server -- mp_carentan --gametype tdm --set
scr_friendlyfire=1`: the lone `--save-grenade` script, and the target-plus-
shooter pair under `--probe-melee`, `--probe-grenade` and
`--probe-grenade-death`. The retail column is the seven committed fixtures of
section 8's family. None of the vcod captures is committed; each pair was moved
out of the tree after its run, because the fixture names are the retail
evidence's.

**What matches.**

VERIFIED: the melee triple, event for event and entity for entity. Both
engines raise `EV_MELEE_SWIPE` (164) on the attacker's own ring, then
`EV_MELEE_HIT` (166) or `EV_MELEE_MISS` (167) on a broadcast temp entity, then
`EV_FIRE_MELEE` (165) back on the attacker's ring, the last two on the same
frame and about `meleeTime` after the swipe (retail 146 to 165 ms over ten
swings, vcod 98 to 166 over ten, the spread being when the probe's snapshot
arrived rather than when the server raised it). Eleven swings on each side:
retail two hits and nine misses, vcod three and eight.

VERIFIED: melee damage. Retail's target drops 100 to 44; vcod's drops 100 to
53 to 5. Both sit in `meleeDamage + rand(0..5)` times a hit-location
multiplier (3.5): 56 is the band at `torso_upper`'s 1.1, 47 and 48 at a leg's
0.9. The two runs strike from different approaches, so the locations are not
the same and the numbers are not directly comparable.

VERIFIED: the grenade event chain and the explode frame's shape.
`EV_PULLBACK_WEAPON` (158) on the thrower's ring, one `EV_GRENADE_BOUNCE`
(177) per bounce on the missile's own ring, and `EV_GRENADE_EXPLODE` (178)
with parm 5 on the same ring, on a frame that reads `eType` 0, `eFlags` 256,
`pos` and `apos` both `TR_STATIONARY` with zero delta, and the origin
truncated to whole units. Both engines write `events[eventSequence & 3]` and
then increment, so the explode lands in the slot below the sequence.

VERIFIED: `grenadeTimeLeft` takes 0 or 4000 and nothing between, on both
sides. The two `-grenade-shooter.txt` captures alone carry 499 vcod traces and
233 retail ones with no third value. There is no cook countdown in 1.1 MP
(1.14).

VERIFIED: the fuse. Pullback to explode is 4568 ms on vcod's one paired throw
against retail's 4759 and 4549, on the same 1000 ms cook the script holds and
the same `fuseTime` 4.

VERIFIED: the weapon switch to the frag, frame for frame against the lone
capture's `to_frag` step: putaway 14 frames against retail's 13, raise 5
against 6, then ready. The cook holds `weaponstate` 3 for 41 frames against
retail's 40.

**What differs.**

VERIFIED: **every vcod grenade launches with the same tumble.** Retail's
`apos` `trDelta` is drawn per throw. Across the committed throws the roll rate
holds one value for a throw's whole life and takes 316.3, 318.0, 344.8, 354.4,
367.4 and 387.1 deg/s, all inside `360 +/- 45`; the launch pitch rate reads
692.7, 704.1 and 726.9, all inside `720 +/- 45`, and is redrawn at each bounce
(11.2's `G_MissileLandAngles`, unmodelled). Every vcod throw in the four runs
reads exactly `675.0, 0.0, 315.0`, the low end of both bands.
`Missiles` carries its own `flrand` state and `#[derive(Default)]` left it
zero, which is `xorshift`'s fixed point, so the draw was always 0.0. vcod was
wrong; retail is right. Fixed: the pool seeds its state off the host's
`RNG_SEED` (`crates/server/src/game/missile.rs`), and two throws now draw
different tumbles. Still unmodelled is the redraw at each bounce.

VERIFIED: **a held trigger latches a grenade's raise on vcod and not on
retail.** `mp_carentan-tdm-grenade-death-shooter.txt` has retail raise the
frag under `buttons=1` for five frames (ms 10734 to 10930, `raiseTime` 0.25),
then `weaponstate` 3 with `EV_PULLBACK_WEAPON` and `grenadeTimeLeft` 4000 at
ms 10995. vcod holds `weaponstate` 1 for the fourteen frames the capture got
before the scripted `kill` cut it off, and never pulls back. INFERRED: the
cause is the `semiAuto` default. The semi-automatic latch (1.4) pins
`weaponTime` at 1 while the trigger is held, and vcod's weapon parser defaulted
an absent `semiAuto` to true; of the 32 stock `weapons/mp` files, the eight
that omit the key are the four grenades, the three bipod mg42s and the
PTRS41, and retail latches none of them. Retail's own carbine, which spells
the key, does latch: the lone capture's `cancel` step reads `weaponstate` 1
for 54 frames under a held bit, and vcod reproduces that. vcod was wrong;
retail is right. Fixed: the default is off (`crates/common/src/weapon.rs`).

The next three came out of the hand check the end of this section asks for, a
retail 1.1 client on `vcod-server` on 2026-09-06, rather than out of the four
probe runs above.

VERIFIED: **a bounce on any tagged surface was silent on vcod.** The client
reads the bounce alias out of `grenade_bounce_<es.surfType>`
(`cod11-sound-system.md` section 7a) and `iw_sound.csv` carries the
`grenade_bounce_default` row and no other, while retail's bounce arm writes
the material into the event parm alone and leaves `s.surfType` to the explode
(13.1 step 5). vcod wrote `sound_material(tr.surfaceFlags)` into `s.surfType`
on every bounce as well. INFERRED: a bounce on grass, gravel, dirt or concrete
therefore asked for an alias no row has, which is a null handle and a no-op,
where an untagged surface fell back to the `default` suffix and sounded.
vcod was wrong; retail is right. Fixed: the bounce writes the parm only
(`crates/server/src/game/missile.rs`).

VERIFIED: **every throw logged a projectile model nothing had precached.**
`RegisterItem` precaches the weapon file's `projectileModel` verbatim, prefix
and all -- retail's own table reads `394 xmodel/projectile_GermanGrenade` and
`386 xmodel/projectile_USGrenade`
(`crates/server/tests/fixtures/configstrings/mp_carentan-dm.txt`) and
`configstrings_ab` pins ours slot for slot -- while `WeaponDef` strips the
`xmodel/` prefix, so the lookup at the throw compared a stripped name against a
prefixed table and missed. VERIFIED: the index does not travel (11.1), so
nothing on the wire moved; the warning was the whole symptom. Fixed: both
lookups put the prefix back (`crates/server/src/configstrings.rs`).

VERIFIED: **an empty frag stayed in vcod's weapon list.** The lone capture's
`throw_down` step ends on `weapons[0]` 4112, `weaponslots[4]` 0 and `weapon`
0; the step before it reads 4368, 8 and 8, and that is where vcod's replay of
`throw_down` ended too. INFERRED: the drop is 1.5 step 9's `BG_TakePlayerWeapon` and
the 0 in `ps.weapon` is 1.8's switch path finding the player no longer owns
what he holds. vcod's pmove raised `EV_NOAMMO` and nothing consumed it, and
the per-tick mirror put the held bit straight back. vcod was wrong; retail is
right. Fixed: the last shot of a `clipOnly` weapon with no reserve queues a
take beside the weapon change (`crates/server/src/server.rs`).

VERIFIED: **the blast on a player is unmeasured against vcod.** The pair runs
never closed the range: retail spawns two same-team clients 41 to 367 units
apart (the four committed fixtures' first `origin` and `target_origin`),
vcod's `tdm` picker put them 280 units apart in xy and on two floors, `z`
-143.9 against -23.9, so the shooter's walk spent 150 s wedging through the
map and threw from 296 units with no line of sight. Its target took no blast
damage in any run. Retail's target drops 100 to 26 at 137 units, which 14.1's
falloff reproduces to the unit; what is unconfirmed live is only that vcod
puts that number on a real wire. The in-process pin is
`crates/server/tests/combat.rs`. **Open**, and the spawn picker, not the
blast, is what to change.

VERIFIED: **the lone `--save-grenade` script cannot finish against vcod.** Its
`throw_down` and `pin_out` steps blew up their own thrower on both attempts, at
57 s and at 23 s, and the lone capture mode has no respawn, so the run stalls
on the step it died in. Retail's thrower survived the same script from a spawn
144 units up. INFERRED: the spawn geometry, not the damage, since the falloff
at the ranges involved is 14.1's on both sides. The step-by-step comparison
above is off the traces the log carries up to the death.

Not a divergence, recorded so a reader of two captures does not read it as
one: vcod numbers a temp entity out of a fixed 64-slot ring at 958 to 1021
(`crates/server/src/game/temp_entity.rs`) where retail takes the next free
entity, so the melee hit and miss entities read 960 to 971 against retail's
245 to 307. A client keys a fired event on the entity number only to tell one
event from the next, and both sides give every event a fresh one.

The `EV_GRENADE_BOUNCE` parm is the surface type, and vcod's three bounces in
the paired run all read 0 where retail's seven read 5, 6, 10, 17 and 21. That
is 13.4's prop-material gap and nothing new: vcod's other runs read 2, 5, 10
and 21 off world brush, and the gate already excludes a prop bounce's parm.

**What the hand check still owes.** A 1.1 client on `vcod-server`, and eight
things only a person can look at: the arc of a throw; a bounce off a wall and
one off a floor; the explosion's decal and sound; the damage at three ranges
against the server's own `D;` records; a kill by grenade and its killfeed
line; a melee kill and its icon; a death while cooking and the grenade the
body drops; and a wall between the eye and the blast. PENDING; none of it is
claimed here.

---

## 10. Open cells

- UNRESOLVED: retail labels `head` at 44 to 49 units above the origin on a
  victim hit 200 to 400 ms earlier, on a victim whose `legsAnim` is back at
  634 (3.4, second table), where vcod reads `torso_upper` on an idle
  victim. Candidates: a server-side cross-fade out of the knockback's
  one-frame run (`G_DObjCalcPose` blending the way the client does), or the
  echoed `viewangles` lagging the tap by more than one snapshot there. A
  sweep against a target with more than two hits of health at a fixed short
  range, with the pitch read off the cmd rather than the echo, would settle
  it.
- UNVERIFIED: what sets `pm_flags 0x800`, which stops `PM_Weapon` outright,
  and what `pm_flags 0x400` and `0x4000` mean. INFERRED for `0x400`: it is set
  whenever a fire, a melee finish or a weapon change happens while
  `pm_flags & 1`, and the ADS flag update ORs it in on the same prone arm
  (1.13), so it is prone-specific.
- UNRESOLVED: `cod11-mantle.md`, "Jumps", reads the ground jump as writing
  255 into `ps.aimSpreadScale` and the ladder push-off as adding 64. Neither
  is modelled here, because the first does not square with the captures: both
  motion fixtures sample the first airborne frame of a jump and read 66.28
  (carentan) and 67.81 (pavlov), where a 255 written on the takeoff frame
  would still read above 229 one 50 ms frame of halved decay later. 2.1's
  airborne terms alone produce a small climb from 0, which is the shape the
  two samples have. Settling it needs a per-snapshot trace through a jump,
  not another read.
- UNVERIFIED: the meaning of bit `0x10` of the trace word at offset 32, which
  is what makes a bullet continue through a surface at full damage.
- UNVERIFIED: what `Bullet_Endpos` (`0x69624`) is for; nothing on this path
  calls it.
- UNVERIFIED: the hit-location partition itself. It is decided inside
  `trap_LocationalTrace`, which lives in the engine binary and not in the game
  module, so nothing above pins how a point on a player maps to one of the 19
  names.
- Closed, both of them, and by the same measurement. VERIFIED: the captures
  that read a putaway at the reload key, at a stance change and at a jump were
  taken by a probe that sent `cmd.weapon` 0 in every usercmd; retaken with the
  probe sending the weapon it holds, none of the three raises anything and the
  reload key reloads with event 151 (`player-model-anim-system.md`, "The
  weapon channel"). INFERRED: the weapon-change check of 1.8 read the zero as
  a request to holster, which is exactly the source 1.8 lists and no other,
  and the byte only reaches the server in the full usercmd branch, which a
  `wbuttons`, `upmove` or `weapon` change forces -- the three inputs those
  three steps used.
- UNVERIFIED: the exact meaning of `client+0x220C` and `client+0x2210`, the
  two floats `FireWeapon` substitutes for the view pitch and yaw.

---

## 11. `fire_grenade`: what a throw spawns

Sections 11 to 14 cite a call or a stored function pointer by the address of
its **relocation slot**, which is the instruction's operand and one byte past
the `call` or `mov` opcode. `readelf -r game.mp.i386.so` lists them at exactly
those addresses; a disassembly listing shows the instruction one byte lower.

Everything a thrown grenade is comes from one function. VERIFIED:
`fire_grenade` is `0x543AC` in `game.mp.i386.so`, `0x268` bytes, and the
module's relocation table holds exactly three calls to it, at `0x49B70`
(inside `player_die`), `0x6904E` (inside `FireWeapon`) and `0x69396` (inside
`weapon_grenadelauncher_fire`).

VERIFIED: it reads stack slots `+8`, `+0xC`, `+0x10` and `+0x14`. INFERRED:
those are `(self, origin, velocity, weapon)`, read off the three call sites,
each of which hands a gentity, a vec3 it filled with a spawn point, a vec3 it
filled with a velocity, and a weapon number.

### 11.1 The entity it builds

VERIFIED: the offsets, immediates, weapon-def fields and call targets named in
the list below, each read out of the instruction it sits in. INFERRED: the
ordering, and every "when" and "otherwise" in it, which are branch conditions.

- `G_Spawn()` supplies the entity.
- `nextthink` (`+0x1FC`) takes `level.time + client->ps.grenadeTimeLeft`
  (`client+0x34`) when the thrower has a client and that field is non-zero,
  and `level.time + 2500` otherwise. `think` (`+0x200`) takes
  `G_ExplodeMissile` unconditionally.
- `client->ps.grenadeTimeLeft` is cleared when the thrower has a client, so
  the cook time transfers from the playerstate to the missile's fuse and
  nothing keeps counting on the player.
- `s.eType` (`+0x4`) takes 4. VERIFIED: 4 is `ET_MISSILE` in
  `private/reference/CoDExtended/src/shared.h:449`, and the entity runner that
  precedes `G_RunFrame` (`0x50478`) branches on `s.eType == 4` into
  `G_RunMissile` at `0x50376`.
- `s.eFlags` (`+0x8`) takes `0x03000000`. VERIFIED: this is the module's only
  write of either bit, and the only reads of them are the `& 3` byte tests on
  `entityState+0xB` in `G_RunMissile` (`0x54219`) and `G_MissileImpact`
  (`0x53AE5`) and the two `test` immediates in `G_BounceMissile` (12.4).
  INFERRED: they are the two bounce flags, since bouncing is all any of the
  three does with them.
- `r.svFlags` (`+0xF4`) takes `0x88`. VERIFIED: `0x8` is the bit `0x5A7EC`
  sets as `SVF_BROADCAST`. UNVERIFIED: what `0x80` means.
- `s.weapon` (`+0xC8`) takes the `weapon` argument, `r.ownerNum` (`+0x14C`)
  the thrower's `s.number`, and `parent` (`+0x198`) the thrower itself.
- `classname` (`+0x176`) takes `Scr_SetString` of `scr_const+0x3E`. VERIFIED:
  `GScr_LoadConsts` (`0x58550`) fills that slot with
  `Scr_AllocString("grenade")`.
- `BG_GetInfoForWeapon(weapon)` supplies the weapon def, and four fields are
  copied out of it into the entity: `damage` (`0x1C0`) into `+0x238`,
  `explosionInnerDamage` (`0x30C`) into `+0x23C`, `explosionOuterDamage`
  (`0x310`) into `+0x240` and `explosionRadius` (`0x308`) into `+0x244`.
  `+0x248` takes the literal 3 and `+0x24C` the literal 4. VERIFIED: 3 is
  `MOD_GRENADE` and 4 `MOD_GRENADE_SPLASH` in 4.1's table. INFERRED: the six
  slots are `damage`, `splashDamage`, a second splash number, `splashRadius`,
  `methodOfDeath` and `splashMethodOfDeath`, read off how `G_MissileImpact`
  and `G_ExplodeMissile` hand them on (13).
- `clipmask` (`+0x190`) takes `0x02802091`. VERIFIED: the bullet's mask (2.3)
  is `0x02802031`, so the two differ in exactly two bits: the grenade drops
  `0x20` and adds `0x80`. VERIFIED: `0x20` is `CONTENTS_WATER`
  (`bsp-ibsp59-format.md`, "Content flags"), and `G_RunMissile` ORs it back in
  conditionally (12.1). UNVERIFIED: what `0x80` is.
- VERIFIED: `G_SetClientContents` (`0x41530`) writes `0x02000000` into a live
  player's contents (`gentity+0x118`), and `0x02000000` is set in the grenade
  clipmask. INFERRED: a live player therefore stops a grenade in flight, which
  is what 12.3 and 13.1 do with the contact.
- VERIFIED: `fire_grenade` writes nothing to `mins` (`+0x100`), `maxs`
  (`+0x10C`), `contents` (`+0x118`), `takedamage` (`+0x171`) or `die`
  (`+0x218`). INFERRED: the missile is a zero-sized point that nothing can
  target, since `G_Spawn` hands back a zeroed record.

### 11.2 The trajectory

VERIFIED: the offsets, immediates and call targets named in the list below.
INFERRED: the ordering in it.

- `s.pos.trType` (`+0xC`) takes 5, `s.pos.trTime` (`+0x10`) `level.time`, and
  `s.pos.trBase` (`+0x18`) the `origin` argument verbatim. VERIFIED: 5 is
  `TR_GRAVITY` in CoD 1.1's own `trType_t`, read out of `BG_EvaluateTrajectory`
  at `.so 0x2C600` and tabulated in `docs/protocol-1.1.md`, divergence 8.
  CoDExtended's `shared.h` puts `TR_SINE` at 5 and is an unverified RTCW paste;
  the protocol doc records reading it that way as a past error of this repo's.
- `s.pos.trDelta` (`+0x24`) takes the `velocity` argument with each component
  separately truncated toward zero to a whole number and converted back, the
  x87 round-to-zero control word being set for each of the three.
- `s.apos.trType` (`+0x30`) takes 2 (`TR_LINEAR`), `s.apos.trTime` (`+0x34`)
  `level.time`.
- `s.apos.trBase` (`+0x3C`) takes `vectoangles(velocity)`, after which
  `trBase[0]` is replaced by `AngleNormalize360(trBase[0] - 120.0)`
  (`.rodata 0x75B60`).
- `s.apos.trDelta[0]` (`+0x48`) takes `flrand(-45.0, 45.0)
  (.rodata 0x75B68, 0x75B64) + 720.0` (`.rodata 0x75B6C`),
  `trDelta[1]` (`+0x4C`) the literal 0, and `trDelta[2]` (`+0x50`) a second
  `flrand(-45.0, 45.0)` plus `360.0` (`.rodata 0x75B70`). INFERRED: those are
  degrees per second, so a thrown grenade tumbles at 675 to 765 deg/s in pitch
  and 315 to 405 deg/s in roll and never yaws.
- `r.currentOrigin` (`+0x134`) takes the `origin` argument and
  `r.currentAngles` (`+0x140`) the finished `apos.trBase`.

### 11.3 Where the throw's origin and velocity come from

VERIFIED, `FireWeapon` `0x68D68`: it takes the client's view angles from
`client+0xC0`, then overwrites pitch with `client+0x220C` and yaw with
`client+0x2210`, calls `AngleVectors` for a forward/right/up basis, sets the
start point to `self->r.currentOrigin` with `client+0xD0` added to z, calls
`G_AddLean` on it, and truncates each of its three components toward zero.
INFERRED: `client+0xD0` is the view height, since `CanDamage` adds the same
field to the same origin for the same purpose (14.3). INFERRED: that start
point is the muzzle, since the same local is what `FireWeapon` hands
`Bullet_Fire_Extended` as the trace start (2.3).

VERIFIED, `FireWeapon` `0x69003`-`0x6909F`: on the `weaponType == 1` arm it
builds `velocity = forward * (float)weapDef->projectileSpeed (0x314)` with
`(float)weapDef->projectileSpeedUp (0x318)` added to z, calls `fire_grenade`
with that start point and that velocity and `self->s.weapon`, then normalizes
the velocity in place and adds `n * dot(client->ps.velocity, n)` to the
returned missile's `s.pos.trDelta`, where `n` is the normalized velocity.
INFERRED: the thrower's own motion along the throw direction is therefore
added to the grenade after `fire_grenade` has truncated the delta, so the
final `trDelta` is not whole-numbered.

VERIFIED: `weapon_grenadelauncher_fire` (`0x69348`) is the same arithmetic
against the `params` frame of 2.3, taking the forward axis from `params+0`,
the origin from `params+0x24` and the weapon def from `params+0x3C`.
UNVERIFIED: what calls it, since nothing on the paths read here does.

VERIFIED, the `player_die` call at `0x49B70`, which is 5.1's step 5 read out
in full: the origin is `self->r.currentOrigin` with `40.0`
(`.rodata 0x743EC`) added to z, the weapon is `self->s.weapon` (`+0xC8`), and
the velocity is built from three `rand()` calls and two constants, `160.0`
(`.rodata 0x743E8`) and `-4.656612873e-10` (`.rodata 0x743E4`, the bit pattern
`0xB0000000`, which is `-2^-31`). VERIFIED: components 0 and 1 are each
`160.0 * (2 * (rand() * -2^-31) - 1.0)` and component 2 is
`160.0 * (rand() * -2^-31)`, the subtraction order taken from the `DE E1`
encoding with `2m` in `st(0)` and `1.0` in `st(1)`. INFERRED: with glibc's
`rand()` in `[0, 2^31)` that puts x and y in `(-480, -160]` and z in
`(-160, 0]`, so the death drop is not isotropic at all and the negative
constant looks like a sign slip in the shipped code. This corrects 5.1's
step 5, which called it "a random direction ... and a speed of 160.0".

---

## 12. `G_RunMissile`: flight, bounce, rest

VERIFIED: `G_RunMissile` is `0x53FCC`, `0x3DE` bytes, and the module's
relocation table holds one call to it, at `0x50376`, inside the static entity
runner that ends just before `G_RunFrame` (`0x50478`) and reaches it on
`s.eType == 4`.

### 12.1 The move trace

VERIFIED: the offsets, immediates and call targets named in the list below.
INFERRED: the ordering, and every "when", "unless" and "otherwise" in it,
which are branch conditions.

- `BG_EvaluateTrajectory(&ent->s.pos, level.time, origin)` gives this frame's
  wanted position, and the travel vector is `origin - ent->r.currentOrigin`.
- When `VectorNormalize` of that vector returns less than `0.001`
  (`.rodata 0x75B4C`), the function calls `G_RunThink` and returns without
  tracing.
- The trace mask starts as `ent->clipmask` and gets `0x20`
  (`CONTENTS_WATER`) OR-ed in when `fabs(s.pos.trDelta[2])` exceeds `30.0`
  (`.rodata 0x75B50`, a double) and `trap_PointContents(r.currentOrigin, -1,
  0x20)` returns 0.
- The trace is `trap_LocationalTrace(&tr, r.currentOrigin, origin,
  r.ownerNum, mask, bulletPriorityMap)`. VERIFIED: there are no mins/maxs
  arguments and the priority map is always `bulletPriorityMap`, never
  `riflePriorityMap`. INFERRED: a missile is traced as a point against the
  same per-bone player boxes a pistol bullet is (3.1).
- When `(tr.surfaceFlags & 0x1F00000) == 0x1400000`, that is surface type
  `0x14`: `VectorNormalize2(s.pos.trDelta, d)` and `d[2]` is forced
  non-negative, a temp entity is spawned at `r.currentOrigin` carrying event
  `0xAD` (173, `EV_BULLET_HIT_SMALL`) with `eventParm` `DirToByte(tr.normal)`,
  `entityState+216` `DirToByte(d)`, `s.surfType` (`+0x88`) the same five bits
  shifted down, and `otherEntityNum` (`+0x74`) the missile's own number, and
  the trace is then re-run with the plain `ent->clipmask`. INFERRED: that is
  the water-entry splash, since the only content the mask gained was water and
  the re-trace is what drops it again.
- When `ent->methodOfDeath (+0x248) == 3` and the hit entity's
  `flags & 0x10000` (byte `gentity+0x17E`, bit 1) is set, the hit entity's
  `contents` (`+0x118`) is zeroed, the trace is run a third time, and the
  contents restored. VERIFIED: `GScr_DisableGrenadeBounce` (`0x5DF8C`) ORs
  that exact bit in and `GScr_EnableGrenadeBounce` (`0x5DF50`) masks it out,
  both against the entity number the script called them on. INFERRED: so
  `disableGrenadeBounce` makes a grenade pass through that entity, and the
  re-trace is how it does it.
- `r.currentOrigin` then takes `tr.endpos`, and `tr.fraction` is forced to 0
  when `tr` reports start-solid (`tr+0x2F`).

### 12.2 The ground snap and the touch pass

VERIFIED: the offsets, immediates and call targets named in the list below.
INFERRED: the ordering and the conditions in it.

- When `s.eFlags & 0x03000000` is non-zero and either `tr.fraction` is
  exactly 1.0 or `tr.normal[2]` is above `0.7` (`.rodata 0x75B58`), a second
  `trap_LocationalTrace` runs straight down from `r.currentOrigin` to
  `r.currentOrigin` with `1.5` (`.rodata 0x75B5C`) subtracted from z, into the
  same trace struct. When that one comes back below 1.0,
  `r.currentOrigin[2]` becomes `tr.endpos[2] + 1.5` and `s.pos.trBase[2]`
  moves by the same delta. INFERRED: that is a per-frame snap that keeps a
  rolling grenade on the floor, since it runs on a frame that hit nothing at
  all as well as on one that hit a floor.
- INFERRED, and the trap an implementer walks into: the trace struct 12.3
  reads is the downward one whenever that second trace ran, so a frame whose
  move trace hit a wall but whose downward trace found no floor within 1.5
  units reaches 12.3 with `fraction` 1.0 and takes the "hit nothing" arm.
- `trap_LinkEntity(ent)` runs next.
- When `ent->methodOfDeath == 3`,
  `G_GrenadeTouchTriggerDamage(ent, oldOrigin, r.currentOrigin,
  ent->+0x23C, 3)` runs, `oldOrigin` being the position saved at the top of
  the frame. VERIFIED: `G_GrenadeTouchTriggerDamage` is `0x655A0`, walks
  `trap_EntitiesInBox` over the bounds of the two positions with mask
  `0x400000`, keeps entities whose `classname` is `scr_const+0x96`
  (`"trigger_damage"`) and whose `flags & 0x8000` is set, requires
  `trap_SightTraceToEntity` between the two positions to succeed, then
  notifies the trigger with `scr_const+0x1A` (`"damage"`) and two arguments
  and calls `Activate_trigger_damage` (`0x650C0`). VERIFIED:
  `GScr_EnableGrenadeTouchDamage` (`0x5DE94`) ORs `0x80` into byte
  `gentity+0x17D`, which is that bit, `GScr_DisableGrenadeTouchDamage`
  (`0x5DEF0`) masks it out, and both refuse an entity whose `classname` is not
  `scr_const+0x96`. INFERRED: so that pair is the per-trigger switch, and this
  is how a grenade rolling through a `trigger_damage` brush sets it off
  without touching anything else.

### 12.3 What happens at the end of the move

VERIFIED: the immediates and call targets named in the list below. INFERRED:
the ordering and the conditions in it.

- When `tr.fraction` is 1.0 the missile did not hit anything: the length of
  `s.pos.trDelta` is taken, and if it is not exactly zero,
  `s.groundEntityNum` (`+0x7C`) takes `0x3FF`. `G_RunThink(ent)` closes the
  frame.
- Otherwise, when `tr.surfaceFlags & 0x10` is set, `G_FreeEntity(ent)` runs
  and the frame ends with no event, no explosion and no think.
  UNVERIFIED: what that surface bit is called. INFERRED: it is the sky, since
  freeing the missile silently is what a sky brush is for.
- Otherwise `G_MissileImpact(ent, &tr, dir)` runs, and `G_RunThink(ent)`
  follows only when `s.eType` is still 4. VERIFIED: `G_MissileImpact`'s
  explode path sets `s.eType` to 0 (13.1), so the test is what stops a
  detonated missile from thinking again in the same frame.

### 12.4 `G_BounceMissile`

VERIFIED: `G_BounceMissile` is `0x537C0`, `0x2F3` bytes, reads stack slots
`+8` and `+0xC`, and returns 0 or 1 in `eax`. INFERRED: the two slots are
`(ent, tr)`, read off the two `G_MissileImpact` call sites. VERIFIED: the
offsets, immediates and constants named in the list below. INFERRED: the
ordering and every condition in it.

- `contents = trap_PointContents(ent->r.currentOrigin, -1, 0x20)` is taken
  first.
- The impact time is `level.previousTime (level+0x1EC) + (int)((level.time -
  level.previousTime) * tr.fraction)`, and `BG_EvaluateTrajectoryDelta` at
  that time gives the incoming velocity `v`.
- `dot = v . tr.normal`, and `s.pos.trDelta` takes `v + (-2.0) * dot *
  tr.normal` (`.rodata 0x75B18`). INFERRED: that is a mirror reflection with
  no energy lost, and the damping is applied after it.
- When `tr.normal[2]` is above `0.7` (`.rodata 0x75B20`, a double)
  `s.groundEntityNum` (`+0x7C`) takes `tr.entityNum`.
- With `s.eFlags & 0x02000000` clear, no damping is applied at all and no
  rest test is reached.
- With that bit set and either `contents` non-zero or the hit entity's
  contents (`tr+0x20`) carrying `0x02000000`, `s.pos.trDelta` is scaled by
  `0.125` (`.rodata 0x75B28`). VERIFIED: `0x02000000` is the contents
  `G_SetClientContents` (`0x41530`) gives a live player. INFERRED: a grenade
  that bounces off a player or lands in water keeps an eighth of its speed.
- With `s.eFlags & 0x01000000` also set and neither of those two true, the
  reflected delta is split: writing `t` for the reflected delta plus
  `dot * tr.normal`, the new `s.pos.trDelta` is `0.75 * t`
  (`.rodata 0x75B2C`) minus `0.3 * dot * tr.normal` (`.rodata 0x75B30`).
  INFERRED: with the `-2.0` reflection above, `t` is the tangential component
  of the incoming velocity, so `0.75` is sliding friction and `0.3` the
  restitution normal to the surface. This is the branch a stock grenade takes,
  since `fire_grenade` sets both bits (11.1).
- With `0x02000000` set and `0x01000000` clear, `s.pos.trDelta` is scaled by
  `0.5` (`.rodata 0x75B34`).
- The rest test: when `tr.normal[2]` is above `0.7` and the length of the
  damped `s.pos.trDelta` is below `20.0` (`.rodata 0x75B38`),
  `G_SetOrigin(ent, ent->r.currentOrigin)`,
  `G_MissileLandAngles(ent, tr, angles, 1)` and `G_SetAngle(ent, angles)`
  run and the function returns 0. VERIFIED: `G_SetOrigin` (`0x67D38`) writes
  `s.pos.trType` 0 (`TR_STATIONARY`), `trTime` 0, `trDuration` 0, `trBase`
  and `r.currentOrigin`, and `G_SetAngle` (`0x67D9C`) does the same to
  `s.apos`. INFERRED: that is where a grenade stops rolling.
- Otherwise the missile is nudged: `d = 0.1 * tr.normal`
  (`.rodata 0x75B3C`) with `d[2]` replaced by 0 when it is positive,
  `r.currentOrigin` and `s.pos.trBase` both take `r.currentOrigin + d`,
  `s.pos.trTime` takes `level.time`, and `s.apos.trBase` takes
  `G_MissileLandAngles(ent, tr, angles, 0)` with `s.apos.trTime` `level.time`.
  INFERRED: clamping `d[2]` means the nudge never lifts the grenade off a
  floor, only off a wall or a ceiling.
- The return value is 0 when `contents` is non-zero, 0 when the length of
  `newDelta - v` is at or below `100.0` (`.rodata 0x75B40`), and 1 otherwise.
  INFERRED: the caller reads that as "was this bounce loud enough to hear",
  since the only thing it gates is the bounce event (13.2).

---

## 13. The explode: event and blast

VERIFIED: CoD 1.1 has two explode paths, `G_MissileImpact` and
`G_ExplodeMissile`, and neither calls `G_TempEntity`; both write the event onto
the missile's own `entityState` through `G_AddEvent`, set its `s.eType` to 0
and set `freeAfterEvent`. INFERRED: that is therefore the shape a client sees,
and the `eType` change is what stops it still reading as a missile while the
event frame goes out.

### 13.1 `G_MissileImpact`

VERIFIED: `G_MissileImpact` is `0x53AB4`, `0x2B7` bytes, and reads stack slots
`+8` and `+0xC`. INFERRED: those are `(ent, tr)`; the third argument
`G_RunMissile` pushes is never read.

VERIFIED: the offsets, immediates, event numbers and call targets named in the
list below. INFERRED: the ordering, and every "when" and "otherwise" in it.

- `other = &g_entities[tr.entityNum]`, the stride being `0x314`.
- **When `other->takedamage` (`+0x171`) is zero**: with `ent->s.eFlags &
  0x03000000` clear the function falls through to the explode block below.
  With those bits set, `G_BounceMissile(ent, tr)` runs and the function returns
  after it, having taken these five tests in this order and returned at the
  first that fires:
  1. the bounce returned 0 (`0x53AFC`): return, no event;
  2. the trace reports start-solid (`tr+0x2F`, `0x53B04`): return, no event;
  3. `classname` (`+0x176`) is `scr_const+0xF4`, `"WP"` (`0x53B0E`): return,
     no event;
  4. `classname` is `scr_const+0x30`, `"flamebarrel"` (`0x53B22`):
     `G_AddEvent(ent, 0xC2, 0)`, that is 194, `EV_FLAMEBARREL_BOUNCE`, with a
     literal parm of 0;
  5. otherwise `G_AddEvent(ent, 0xB1, (tr.surfaceFlags >> 20) & 0x1F)`, that
     is 177, `EV_GRENADE_BOUNCE`, with the surface type as its parm. A stock
     grenade is always this arm, its classname being `"grenade"` (11.1).
- **When `other->takedamage` is non-zero and `ent->damage` (`+0x238`) is
  zero**: `G_BounceMissile(ent, tr)` runs and the function returns with no
  event at all.
- **When `other->takedamage` is non-zero and `ent->damage` is non-zero**:
  `LogAccuracyHit(other, &g_entities[ent->r.ownerNum])`,
  `BG_EvaluateTrajectoryDelta` for the missile's velocity with `velocity[2]`
  forced to `1.0` if the length came out zero, then `G_Damage(other, ent,
  attacker, velocity, &ent->r.currentOrigin, ent->damage, 0, ent->+0x248, 0)`,
  the attacker being NULL when `r.ownerNum` is `0x3FF` and `&g_entities[
  ownerNum]` otherwise. VERIFIED: `ent->+0x248` is the literal 3
  (`MOD_GRENADE`) `fire_grenade` wrote. INFERRED: a direct hit on a player
  therefore exists and does `weapDef->damage` at `MOD_GRENADE`, with `dflags`
  0 and `hitLoc` 0, and it falls straight through into the explode block, so
  the victim also takes the splash the block computes.

The explode block, reached from all three arms:

- With `ent->damage` non-zero, `G_CheckHitTriggerDamage(attacker,
  &ent->r.currentOrigin, &tr.endpos, ent->damage, ent->+0x248)`, the attacker
  being `&g_entities[1022]` when `r.ownerNum` is `0x3FF` and
  `&g_entities[ownerNum]` otherwise. VERIFIED: that is the same call step 1 of
  2.4 makes for a bullet, and the world entity substituted here is 1022, not
  the 1023 the damage arm above substitutes NULL for.
- `G_AddEvent(ent, 0xB4, DirToByte(tr.normal))` when `LogAccuracyHit`
  returned non-zero or `tr+0x2A` (the bone name id) is non-zero, and
  `G_AddEvent(ent, 0xB3, DirToByte(tr.normal))` otherwise. VERIFIED: `0xB3`
  is 179 (`EV_ROCKET_EXPLODE`) and `0xB4` is 180
  (`EV_ROCKET_EXPLODE_NOMARKS`). INFERRED: the "nomarks" arm is the one a hit
  on a player takes, which is what stops a blast decal landing on a body.
- `ent->s.surfType` (`+0x88`) takes `(tr.surfaceFlags >> 20) & 0x1F`,
  `ent->freeAfterEvent` (`+0x184`) takes 1, and `ent->s.eType` (`+0x4`) takes
  0 (`ET_GENERAL`).
- `SnapVectorTowards(&tr.endpos, &ent->s.pos.trBase)` and
  `G_SetOrigin(ent, &tr.endpos)` place it.
- With `ent->+0x23C` non-zero, `G_RadiusDamage(&tr.endpos, ent, ent->parent,
  (float)ent->+0x23C, (float)ent->+0x240, (float)ent->+0x244, other,
  ent->+0x24C)`. VERIFIED: the ignored entity is `other`, the entity the
  missile hit. INFERRED: the direct-hit victim is deliberately kept out of the
  splash, having already been charged the direct damage.
- `trap_LinkEntity(ent)` closes it.

### 13.2 `G_ExplodeMissile`, the fuse

VERIFIED: `G_ExplodeMissile` is `0x53D6C`, `0x260` bytes, takes one argument,
and the module stores its address into a `think` slot at three places:
`0x5440A` in `fire_grenade`, `0x54678` in `fire_rocket` and `0x548B4` in
`G_MissileDie`.

VERIFIED: the offsets, immediates, event numbers and call targets named in the
list below. INFERRED: the ordering and the conditions in it.

- `BG_EvaluateTrajectory(&ent->s.pos, level.time, org)`, each component of
  `org` truncated toward zero, then `G_SetOrigin(ent, org)`.
- `ent->s.eType` (`+0x4`) takes 0, `s.eFlags` gains `0x100`, `flags`
  (`+0x17C`) gains `0x1000`, and `r.svFlags` (`+0xF4`) gains `0x8`
  (`SVF_BROADCAST`). INFERRED: the broadcast bit is what makes the explosion
  reach every client rather than only those the missile was in PVS of.
- When `classname` (`+0x176`) is `scr_const+0x30` (`"flamebarrel"`),
  `freeAfterEvent` takes 1, the entity is linked, and the function returns
  with no event and no blast.
- Otherwise `trap_Trace(&tr, &ent->r.currentOrigin, vec3_origin, vec3_origin,
  down, ent->s.number, 0x11)` runs, `down` being `r.currentOrigin` with `16.0`
  (`.rodata 0x75B44`) subtracted from z.
- `G_AddEvent(ent, 0xB2, DirToByte(tr.normal))`. VERIFIED: `0xB2` is 178,
  `EV_GRENADE_EXPLODE`, and its `eventParm` is the packed normal of that
  downward trace. INFERRED: that normal is what orients the blast mark, which
  is why the trace exists at all.
- `ent->s.surfType` (`+0x88`) takes `0x14` when
  `trap_PointContents(r.currentOrigin, -1, 0x20)` is non-zero, and
  `(tr.surfaceFlags >> 20) & 0x1F` otherwise. INFERRED: `0x14` is the water
  surface type, the same one 12.1's splash branch matches on.
- `freeAfterEvent` (`+0x184`) takes 1.
- With `ent->+0x23C` non-zero, `G_RadiusDamage(&ent->r.currentOrigin, ent,
  ent->parent, (float)ent->+0x23C, (float)ent->+0x240, (float)ent->+0x244,
  ent, ent->+0x24C)`. VERIFIED: the ignored entity here is the missile itself,
  not a victim, which is the one argument that differs from 13.1's call.
- `trap_LinkEntity(ent)`, then a second entity from `G_Spawn()` takes the
  missile's `r.currentOrigin`, `think` `Concussive_think` (`0x54808`),
  `nextthink` `level.time + 100` and `+0x274` `(float)level.time + 500.0`
  (`.rodata 0x75B48`). VERIFIED: `Concussive_think` re-arms itself every 100
  ms and swaps its own `think` for `G_FreeEntity` once `level.time` passes
  `+0x274`. INFERRED: a concussion field lives 500 ms past the blast; nothing
  read here says what reads it.

### 13.3 What is not on this path

VERIFIED: `weapDef->projImpactExplode` is `0x32C` in the field table of
section 0, and the module contains no integer or boolean read of a weapon def
at that offset. INFERRED: the key parses and is never consulted; whether a
missile detonates on contact is decided entirely by `s.eFlags & 0x03000000`
and by the hit entity's `takedamage` (13.1).

VERIFIED: `G_MissileDie` is `0x5489C` and sets `takedamage` 0, `think`
`G_ExplodeMissile` and `nextthink` `level.time + 10` when the inflictor is not
the entity itself. VERIFIED: `fire_grenade` never writes `die` (`+0x218`) or
`takedamage`, so nothing can shoot a thrown grenade down.

### 13.4 As implemented

`crates/server/src/game/missile.rs` carries sections 11 to 13: `fire_grenade`,
the per-frame move with its bounce and rest, and the explode. Five decisions in
it come off the captures rather than off the binary, and each is worth naming
because a reader of 11 to 13 alone would guess otherwise.

VERIFIED, off `mp_carentan-tdm-grenade.txt`: a throw's `trTime` is the
*previous* frame's `level.time`, not the frame the release lands on. The
death-drop capture pins it twice over, `trTime` 1405500 against a death frame
at `serverTime` 1405550. vcod stamps both the thrown and the dropped grenade
that way.

VERIFIED, off the same capture's `!missile` lines: a bounce's `trTime` is a
whole millisecond, so vcod truncates the impact time rather than carrying the
trace fraction into it, and the fraction it snaps the origin back with is the
1.5-unit trace's, not the move trace's. Reproducing the committed arcs to
within 0.1 units needed both.

VERIFIED, off the same lines: the exploded entity stays on the wire for about
300 ms after the event frame, which is `EVENT_VALID_MS` in the file. Retail's
`freeAfterEvent` frees it once the event has been sent to everyone who can
see it; vcod holds it for a fixed window instead.

VERIFIED: the lone capture's second throw comes to rest at `z` 179.3 while its
thrower stands at 144.1, 35 units up on a cart the bare BSP has no surface at.
INFERRED, from that rest height against the bare BSP: retail's server clip
includes the map's static props. `World::from_bsp` therefore takes the paks and
builds the propped collision world for the server too; the client's prediction
world always had them. Two things a prop costs,
because its triangles reach vcod's world without the material they came from:
a bounce off one carries `eventParm` 0 where retail carries the surface type
(21 on the two committed throws that land on that cart), and retail's explode
packs the normal of a 16-unit downward trace whose mask misses a prop's
contents, so a grenade resting on one explodes with parm 0 where vcod finds
the prop and packs 5. `crates/server/tests/missile_ab.rs` excludes both.

VERIFIED: a missile's `index` is 0 on the wire on both sides, and its
`eFlags` `0x03000000` (11.1) sits above the 24-bit netfield, so nothing of it
travels. The model still registers at map load, which `configstrings_ab` pins.

INFERRED, and the reason there is no direct-hit arm: 13.1's `MOD_GRENADE`
path is gated on `ent->damage`, and `fraggrenade_mp` spells `damage` 0, so a
stock frag bounces off a live player at the soft damping instead of
detonating on it. vcod traces against live player boxes and applies that
damping; the arm itself is out.

---

## 14. `G_RadiusDamage` and `CanDamage`

### 14.1 The walk and the falloff

VERIFIED: `G_RadiusDamage` is `0x4A3F4`, `0x4B9` bytes, and reads stack slots
`+8`, `+0xC`, `+0x10`, `+0x14`, `+0x18`, `+0x1C`, `+0x20` and `+0x24`.
INFERRED: those are `(origin, inflictor, attacker, inner, outer, radius,
ignore, mod)`, read off the three call sites: the two in 13, which pass the
three integers `fire_grenade` copied out of `explosionInnerDamage`,
`explosionOuterDamage` and `explosionRadius` in that order, and the
`radiusDamage` builtin of 14.2, whose script argument order pins the same
reading a second time.

VERIFIED: the offsets, immediates, constants and call targets named in the
list below. INFERRED: the ordering and every condition in it.

- The function returns 0 immediately when `attacker` is NULL.
- `radius` is raised to `1.0` when it is below it.
- The search box is `origin` plus and minus `radius * 1.4142135`
  (`.rodata 0x74430`) on each axis, and `trap_EntitiesInBox(mins, maxs, list,
  1024, -1)` fills the candidate list. INFERRED: the square root of two is
  there so the box circumscribes the sphere rather than inscribing it.
- A candidate is skipped when it is the `ignore` entity or when its
  `takedamage` (`+0x171`) is zero.
- The distance vector is `ent->r.currentOrigin - origin` when `gentity+0xFC`
  is zero, and otherwise the per-axis gap to the entity's world-space box:
  `absmin[i] - origin[i]` when `origin[i]` is below `absmin[i]`
  (`gentity+0x11C`), `origin[i] - absmax[i]` when it is above `absmax[i]`
  (`gentity+0x128`), and 0 in between. INFERRED: `gentity+0xFC` is
  `r.bmodel`, being the one int between `singleClient` (`+0xF8`) and `mins`
  (`+0x100`); this is Quake III Arena's `G_RadiusDamage` distance rule with a
  brush-model gate added, so a player is measured origin to origin and a door
  nearest-point to origin.
- A candidate is skipped when that distance is at or above `radius`.
- A candidate with a client (`+0x158`) is skipped when `level+0x29F4` is
  non-zero (14.2).
- The damage before line of sight is
  `outer + (1 - distance / radius) * (inner - outer)`, which is `inner` at the
  blast and `outer` at the radius, linear in between.
- `CanDamage(ent, origin)` returns a float. With that float above zero:
  `LogAccuracyHit(ent, attacker)`, then `G_Damage(ent, inflictor, attacker,
  dir, origin, (int)(canDamage * points), 1, mod, 0)`, where `dir` is
  `ent->r.currentOrigin - origin` with `24.0` (`.rodata 0x74434`) added to z.
  VERIFIED: the `dflags` argument is the literal 1, which is the
  `iDFLAGS_RADIUS` the gametype scripts spell.
- With `CanDamage` at or below zero, a second chance runs: `trap_Trace(&tr,
  origin, vec3_origin, vec3_origin, midpoint, 0x3FF, 0x11)` against the
  midpoint of the entity's world box (`0.5`, `.rodata 0x74438`, a double),
  and the candidate is kept only when that trace was blocked
  (`tr.fraction < 1`) and the midpoint is nearer than `radius * 0.2`
  (`.rodata 0x74440`). The damage on that arm is `(int)(points * 0.1)`
  (`.rodata 0x74444`) with the same `dir` and the same `dflags` 1.
  INFERRED: that is a token amount for a victim hugging the far side of the
  wall the blast went off against.
- The return value is 1 when any `LogAccuracyHit` returned non-zero and 0
  otherwise.

### 14.2 The player-ignore flag is on `level`, not on `gclient_t`

VERIFIED: `0x5EF6C` is a script builtin of one boolean argument, and its whole
body is `level+0x29F8 = Scr_GetBool(0)`. VERIFIED: `level+0x29F8` is written
nowhere else in the module and read at exactly one place.

VERIFIED: `0x5EEF4` is the `radiusDamage` builtin. VERIFIED: it takes
`Scr_GetVector(0)` and `Scr_GetFloat(1)`, `(2)` and `(3)`, copies
`level+0x29F8` into `level+0x29F4`, calls `G_RadiusDamage(origin, NULL,
&g_entities[1022], arg2, arg3, arg1, NULL, 0x18)`, then writes 0 back into
`level+0x29F4`. VERIFIED: `0x18` is 24, `MOD_EXPLOSIVE` in 4.1's table, and
`&g_entities[1022]` is the offset `0xC49D8` at a stride of `0x314`.

INFERRED: the script signature is therefore
`radiusDamage(origin, range, maxDamage, minDamage)`, with `range` reaching
`G_RadiusDamage`'s `radius`, `maxDamage` its `inner` and `minDamage` its
`outer`, so 14.1's falloff is exactly "linear from `maxDamage` at the blast to
`minDamage` at the range". VERIFIED: `level+0x29F4` is read at one place, the
client test in `G_RadiusDamage` (`0x4A5D3`), and written at two, both inside
the `radiusDamage` builtin. INFERRED: `setPlayerIgnoreRadiusDamage` therefore
suppresses player damage only for the duration of a scripted `radiusDamage`
call and has no effect at all on a grenade's own blast, whose two callers
never touch `level+0x29F4`.

What is left of the `radiusDamage` divergence entry in
`cod11-gsc-language.md` after this: the victim walk, the standing box the
builtin measures a victim with, and the `undefined` the callback gets where
retail hands over the world entity. The flag is not among them.

### 14.3 `CanDamage`

VERIFIED: `CanDamage` is `0x4A098`, `0x35C` bytes, takes `(targ, origin)` and
returns a float on the x87 stack rather than an integer. VERIFIED: both of its
arms trace five times with `trap_LocationalTrace(&tr, origin, point,
targ->s.number, 0x02802091, bulletPriorityMap)`, the same mask
`fire_grenade` gives a missile (11.1). VERIFIED: the only call to it in the
module is at `0x4A601`, inside `G_RadiusDamage`.

**With no client** (`targ+0x158` zero). VERIFIED: the five points are built
from the world box midpoint, `0.5 * (r.absmin + r.absmax)` (`.rodata
0x74424`), with `15.0` (`.rodata 0x74420`) and `-15.0` (`.rodata 0x74428`)
added to the x and y of four of them and z left alone on all five. VERIFIED:
the loop returns `1.0` the moment one trace comes back with `fraction ==
1.0`, and `0.0` when none does. INFERRED: that is Quake III Arena's
`CanDamage` unchanged.

**With a client**. VERIFIED: the eye point is `targ->r.currentOrigin` with
`client+0xD0` added to z and `G_AddLean(targ, eye)` applied, the five points
are built around `0.5 * (eye + r.currentOrigin)`, and the horizontal offset is
`15.0` and `-15.0` times the vector `(-d[1], d[0], d[2])` where `d` is the
normalized horizontal direction from the target's origin to the blast, `d[2]`
being pinned to 0. VERIFIED: `0.5 * (eye[2] - r.currentOrigin[2])` is
subtracted from and added to the z of the offset points. INFERRED: the set is
the body centre plus four points at the corners of a 30-unit-wide, body-tall
rectangle held broadside to the blast. UNVERIFIED: which of the four corners
gets which sign pair, which the register shuffling did not make legible and
which does not matter to a symmetric set.

VERIFIED: the count of traces returning `fraction == 1.0` maps to the return
value as 0 for none, `1.0` for four or five, and `count / 3.0`
(`.rodata 0x7442C`) otherwise. INFERRED: so a client behind partial cover
takes a third or two thirds of the falloff damage, and three of five clear
points is already full damage. INFERRED: nothing on the bullet path consults
any of this, which 4.6 already said.

**As implemented.** `crate::game::combat`'s `radius_damage` and `can_damage`,
with `Server::tick` charging each of the frame's explosions before
`deliver_hits` so a grenade damages on the frame it goes off, and the
`radiusDamage` builtin (`builtins/combat.rs`) wrapping the same two functions
for a script's own blast. The divergences left are listed in
`cod11-gsc-language.md`'s `radiusDamage` entry. The falloff is computed at
double precision because f32 loses a point of damage at the round ratios a
script picks -- `50 + (1 - 100/300) * 1950` truncates to 1349 in f32 and 1350
in f64 -- and retail's own x87 arithmetic is not reproducible in either
width.
`crates/server/tests/combat.rs`'s two blast tests are the end-to-end gate, and
the grenade capture's own `throw_down` pins the number a third time.
VERIFIED: its pain frame reads `aimSpreadScale` 94.00 off a counter that was
0. INFERRED: section 6's step 5 adds `damage * 100 / maxHealth` there, so
retail charged 94, which is what the falloff gives at the 78 units the replay
measures between the blast and the thrower.
