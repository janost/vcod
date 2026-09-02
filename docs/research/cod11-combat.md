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
event `EV_RELOAD` (151). UNVERIFIED: why the captured reload step in
`player-model-anim-system.md` produced `weaponstate` 2 and event 156 instead.
VERIFIED: no instruction in the reload path writes 2 into `weaponstate`, and
1.8 lists the only two that do. INFERRED: so the cause is outside this path,
most likely the probe's own `cmd.weapon` differing from `ps->weapon` for that
step, which is the one input the weapon-change check acts on.

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

INFERRED: a reload or a rechamber can therefore be interrupted by a weapon
switch and a shot or a melee cannot. INFERRED: the raise does not wait for
`weaponTime`; `raiseTime` only holds off the *next* action, since every other
check is gated on `weaponTime` being 0.

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

UNVERIFIED: what then produced the `weaponstate` 2 and event 156 that
`player-model-anim-system.md` records at a jump and at a stance change. On the
evidence above the cause is outside `PM_Weapon`, and it is carried as open in
section 8; a reader implementing this must not assume a jump or a stance change
raises `EV_PUTAWAY_WEAPON` on its own.

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
`pm_flags & 2` and loads the prone and the ducked spread minimum off those
tests, and the weapon-change check tests `pm_flags & 0x10` and holds a store
of weapon 0. INFERRED: the prone minimum is the `& 1` arm, the ducked one the
`& 2` arm, and the weapon-0 store is the `& 0x10` arm. INFERRED: so `0x1` is
prone, `0x2` is crouch and `0x10` is the ladder.

---

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
`aimSpreadScale` between shots. VERIFIED: the offsets, immediates, weapon-def
fields and call targets named in the list below. INFERRED: the ordering, and
every "when", "otherwise" and "skipped" in it, which are branch conditions.

- It subtracts `hipSpreadDecayRate` (`0x24C`) scaled by the frame time,
  multiplied by `hipSpreadDuckedDecay` (`0x25C`) when `eFlags & 0x20` is set
  and by `hipSpreadProneDecay` (`0x260`) when `eFlags & 0x40` is set.
- It adds `hipSpreadTurnAdd` (`0x254`) per view-angle delta and
  `hipSpreadMoveAdd` (`0x258`) when either movement axis is non-zero.
- It skips all of the additions when `ps->fWeaponPosFrac == 1.0`.
- It clamps the result to 0..255.

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
`riflePriorityMap` is the one a `rifleBullet` weapon takes. INFERRED: the
trace is per-bone rather than against the link box, because it returns a
hit-location index (2.4) and takes a per-hit-location priority table as an
argument; the link box has no such partition.

VERIFIED: the trace result at `[ebp-0x30]` is read at these offsets --
`+0` fraction, `+4` endpos, `+16` normal, `+28` surface flags, `+32` a second
flag word, `+40` a 16-bit entity number, `+44` a 16-bit hit location.
VERIFIED: bit `0x4` of `+28` suppresses the impact effect, and bits 20 to 24
of the same word are shifted down into the temp entity's `surfType`
(`entityState+136`).

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

INFERRED: the map is a per-location weight the engine trace resolves ties
with, so a rifle bullet grazing both a leg and the head is scored as the head,
a pistol bullet scores whichever of the two the geometry gives first, and
`gun` at 0 is never preferred over anything.

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
3. With bit `0x10` of the trace's `+32` word set, the bullet continues:
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
`g_fHitLocDamageMult[]` and into script. INFERRED: the partition therefore
lives in the engine binary, is per-bone rather than per-height-fraction, and
is steered from the game module only by the priority map of section 2.3.

### 3.1 The multiplier table and what a missing file does

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
   `self->r.currentOrigin` with z raised by 40.0 (`.rodata 0x743ec`), a
   random direction built from three `rand()` calls, and a speed of 160.0
   (`.rodata 0x743e8`).
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
    neither. This store is the direct evidence for the dead-yaw meaning
    `docs/protocol-1.1.md` gives `stats[1]`: VERIFIED.
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
| `+0xF4` | the literal `0x200` |
| `+0x161` | the literal 1 |
| `+0x190` | the literal `0x10001` |

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
in `crates/server/tests/fixtures/playerstate/`. Everything below is VERIFIED
unless labelled otherwise: it is read off those captures.

It takes two runs, because one gametype cannot give both halves:

- `mp_carentan-dm-hit-target.txt` and `-hit-shooter.txt` hold the **death
  half**. Deathmatch's spawn picker puts a respawning client at the point
  farthest from the other one, 2000 to 4500 units of town on this map, and the
  shooter's walk does not cross that: its fixture carries
  `# BROKEN no line of sight` and the target's `damageEvent` never moves. That
  pair was also taken before the probe's aim was corrected, so its shooter
  `viewangles` column does not point at the target; nothing in the death half
  depends on it.
- `mp_carentan-tdm-hit-target.txt` and `-hit-shooter.txt` hold the **hit
  half**, captured with `+set g_gametype tdm +set scr_friendlyfire 1` and both
  probes on `allies`. Team deathmatch spawns a player next to its team, so the
  two start a few hundred units apart, and `scr_friendlyfire 1` is what makes
  a teammate's bullet do full damage (`tdm.gsc`'s damage callback returns
  early when the cvar is `<= 0` and only reflects when it is `2`).

### 8.1 The death

The `kill` command lands in one snapshot, ~50 ms after it goes out. Not every
one lands: the committed run sent five and three took, at 10 s, 100 s and
190 s, with the two in between doing nothing at all. UNVERIFIED: what refuses
them. In the frame one lands:

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

`EV_DEATH` with parm 0 is what 5.1 reads out of `player_die`, and this is the
wire confirming it. INFERRED: the `EV_RAISE_WEAPON` beside it is the same
frame's weapon work rather than part of the death, since nothing in
`player_die` writes the weapon channel.

`pm_type` 6 is the dead value the gametype script sets. 5.1 shows `player_die`
writes no `pm_type` at all, so 6 is `sessionstate` reaching pmove through the
script, and it is the value a server has to reproduce.

**`deadViewHeight` never moves.** It reads 8 before the death, through it and
after the respawn, which is 5.1's "no store to `ps->deadViewHeight`" seen from
outside. What moves is `viewHeightCurrent`: 60 at the death frame, then 51, 42,
33, 24, 15, 8 over the next six snapshots, reaching `deadViewHeight` ~300 ms
later. INFERRED: that is the ordinary view-height lerp running to the dead
height, not a death-specific path, since the step per 50 ms snapshot is the
same ~9 units the stance lerp uses.

`stats[1]` reads 0 on every one of these deaths. 5.1 item 11 stores
`vectoyaw(attacker->origin - self->origin)` there, and for a suicide that
vector is zero, so 0 is the expected value and this capture does not exercise
the interesting case.

The four damage fields stay 0 across every death: a `kill` reaches
`player_die` without `P_DamageFeedback` writing anything.

The magazine goes with the death. `ammoclip` reads `3:7, 6:3, 10:15` alive and
`3:7, 6:3` dead, and `ammo` reads `3:56, 10:400` alive and `3:56` dead; index
10 comes back at the respawn. UNVERIFIED: which weapon index 10 is.

### 8.2 The corpse

Every death puts a corpse in the body queue in the same snapshot as
`EV_DEATH`, and the queue is used in order from entity **64**: the four deaths
of the committed run land in 64, 65, 66 and 67, which is what makes 64..71 the range a
client has to read corpses out of. 5.2 could only infer that first slot from
the `& 7` on the index; the capture measures it.

Corpses do not expire on a timer here: the first is still in the body queue at
the end of the run, 190 s after it died. An earlier
run of the same script, on a server where the queue already held bodies, shows
one vanishing and coming back with the same entity number and clientNum.
INFERRED: those edges are PVS, not lifetime, since the target had respawned
elsewhere in between. Nothing in either capture shows a corpse being freed.

### 8.3 The obituary and the respawn

Each death broadcasts one `EV_OBITUARY` (201) in the same snapshot. A `kill`
gives `otherEntityNum` 0 (the victim), `attackerEntityNum` 0 and `eventParm`
**150**, `0x96`, `MOD_SUICIDE`; the tdm capture's two bullet deaths give the
victim 0, the attacker 1 and `eventParm` **136**, `0x88`, `MOD_HEAD_SHOT`.
Both are rows of `cod11-hud-protocol.md` section 2's table. The shooter, 2000
to 4500 units away with no line of sight, receives every one: the event is
`SVF_BROADCAST`.

INFERRED, an instrument caveat rather than a fact about retail: a run can
report more obituaries than deaths. `EventTracker` fires an event entity again
when its slot leaves the snapshot and comes back with the same contents, which
is what a temp entity crossing the PVS boundary does.

The respawn needs the use press and takes it after a delay of its own. The
target presses use 3 s after death and again once a second; it goes live about
1 s after the first press, 4.0 s dead in all, and one run needed three presses
on one death and stayed dead 5.1 s. INFERRED: the presses before that are
refused, so the script holds the body for a few seconds before it will take
one.

On the frame the player is alive again: `stats[0]` 100, `pm_type` 0,
`eventSequence` 0 with a cleared ring, `legsAnim` 634, `torsoAnim` 0,
`viewHeightCurrent` 60, and `eFlags` **24** where it read 16 before. 24 is 16
with bit `0x8` set, the anim-restart toggle 5.2 names on the clone, so a
respawn flips it the same way.

---

### 8.4 What a bullet does

From the tdm capture: `m1carbine_mp` (`damage` 45 in `weapons/mp/m1carbine_mp`)
fired from 586 units at a standing teammate's eye, twice, killing it the second
time. The obituary's `eventParm` is 136, `0x88`, `MOD_HEAD_SHOT`, so both were
head hits.

**The hit frame**, against the settled frame before it:

| field | before | on the hit |
|---|---|---|
| `stats[0]` (health) | 100 | 33 |
| `damageEvent` | 0 | 1 |
| `damageCount` | 0 | 67 |
| `damageYaw` | 0 | 64 |
| `damagePitch` | 0 | 255 |
| `eventSequence` | 0 | 1 |
| `events[0]`, `eventParms[0]` | 0, 0 | **187** (`EV_PAIN`), **33** |
| `velocity` | 0, 0, 0 | -1.4, 80.0, 0 |

**67 damage from a 45-damage weapon on a head hit.** 45 * 1.5 is 67.5, and the
applied damage is 67. INFERRED: the shipped `mp_lochit_dmgtable` gives the head
1.5 and the product is truncated, since 6.4's `damage * multiplier` is the only
scaling on this path and nothing else in the capture is a factor of 1.489.

`damageCount` 67 is 6's `damage * 100 / maxHealth` with `maxHealth` 100, so
this capture cannot separate the two; a server with another max health would.

**`damageYaw` and `damagePitch` are the world angles of the direction the
damage came along, from the attacker toward the victim.** The shooter stood at
`(1810.0, 2109.5)` and the target at `(1800.0, 2696.0)`, a bearing of 91.0
degrees; `(int)(91.0 / 360 * 256)` is 64, which is what `damageYaw` reads.
Both eyes sat at the same height, and `damagePitch` reads 255, one short of
the 256 a level shot wraps to. That is 6's step 8 measured from the wire.

**The knockback is one frame of velocity along that direction.** The target
stood still, and the hit frame reads `-1.4, 80.0, 0`: 80 u/s along the bearing
the bullet travelled, decaying to 64, 46, 28 and 0 over the next four
snapshots. INFERRED: the decay is ordinary ground friction, not a knockback
timer, since the target held no movement input.

`EV_PAIN`'s parm is 33, the victim's health as a percentage of max after the
damage, which is 6's step 9.

**The fatal hit leaves `damageEvent` alone.** The second bullet took health 33
to 0 and `damageEvent` stayed 1 while `damageCount`, `damageYaw` and
`damagePitch` kept the first hit's values. That is 6's step 1, the
`pm_type > 5` return, seen from outside: the feedback for the killing hit never
runs. A death by a bullet also carries the same `pm_type` **6** as the suicide,
`legsAnim` 17, `torsoAnim` 512 and events `[187, 189, 155]` in one frame.

`stats[1]` finally reads something: **270** on the bullet death, where the
suicides read 0. 5.1 item 11 stores `vectoyaw(attacker->origin -
self->origin)`, and the shooter was at bearing 271.0 degrees from the target,
truncated to 270. It is the opposite of `damageYaw`'s direction, which the two
numbers agree on: 64 is 90 degrees, 270 is 90 + 180.

The four damage fields read 0 again after the respawn: `damageEvent` does not
carry across a life, whatever `P_DamageFeedback` does inside one.

---

## 9. Open cells

- UNVERIFIED: what sets `pm_flags 0x800`, which stops `PM_Weapon` outright,
  and what `pm_flags 0x400` and `0x4000` mean. INFERRED for `0x400`: it is set
  whenever a fire, a melee finish or a weapon change happens while
  `pm_flags & 1`, so it is prone-specific.
- UNVERIFIED: the meaning of bit `0x10` of the trace word at offset 32, which
  is what makes a bullet continue through a surface at full damage.
- UNVERIFIED: what `Bullet_Endpos` (`0x69624`) is for; nothing on this path
  calls it.
- UNVERIFIED: the hit-location partition itself. It is decided inside
  `trap_LocationalTrace`, which lives in the engine binary and not in the game
  module, so nothing above pins how a point on a player maps to one of the 19
  names.
- UNVERIFIED: the contents of the shipped `info/mp_lochit_dmgtable`, and hence
  the real per-location multipliers. Only the all-ones default and the zero
  for `gun` are read out of the binary here.
- UNVERIFIED: why the captured reload step produced a putaway rather than a
  reload (1.7).
- UNVERIFIED: what puts a jumping or stance-changing player into `weaponstate`
  2 with event 156, which `player-model-anim-system.md` measured on retail.
  VERIFIED: only two instructions in `game.mp.i386.so` write `weaponstate` 2,
  both inside the putaway function `0x37a9c`, and the module holds exactly two
  calls to that function, both inside the weapon-change check (1.8).
  INFERRED: those two call sites act only on `cmd.weapon`, `ps->weapons` and
  `pm_flags & 0x10`, so the cause is outside `PM_Weapon` and 1.8's list is the
  complete set of putaway sources in this module.
- UNVERIFIED: the exact meaning of `client+0x220C` and `client+0x2210`, the
  two floats `FireWeapon` substitutes for the view pitch and yaw.
