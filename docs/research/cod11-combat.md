# CoD 1.1 combat: the weapon state machine and the damage path

From a held fire button to a dead player: the pmove weapon machine
(`weaponstate`, `weaponTime`, `weaponDelay`, `weapAnim`); the bullet trace and
spread; the hit-location table; `G_Damage` and the script callback that
subtracts health; `player_die` and the corpse clone; the damage-feedback
fields; grenades, missiles and blasts; where a shot runs in the frame; and
what hits and leans do to the drawn view.

No document-level default: provenance is mixed, and each claim names its
source and label. Sources:

- `game.mp.i386.so`, the 1.1d Linux dedicated server's MP game module, with a
  full dynamic symbol table. Addresses are module-relative (image base 0), as
  in `cod11-gsc-object-model.md`. It is PIC, so a plain `objdump -d` hides
  every call and cvar; `python3 tools/re/annotate_func.py <elf> <symbol>`
  resolves them. `.data` pointers read as 0 in the file unless `.rel.data` is
  resolved, and `.data`'s VA runs `0x1000` above its file offset (`readelf
  -S`: `Addr 0x7b3a0 / Off 0x7a3a0`). **Every address here is a VA**, `.data`
  tables included, so a dump out of the file subtracts `0x1000` first.
- `cgame_mp_x86.dll` 1.1 (image base `0x30000000`), the MP client game,
  decompiled with Ghidra. Pmove is shared code, so the whole `PM_Weapon`
  family is in it. A claim read there says so, with the `.so` address when
  the `.so` confirms it.
- `cod_lnxded`, the 1.1d engine executable. It is stripped, so engine
  function names are vcod's and are marked as such where they first appear.
  First LOAD segment at `0x08048000`, file offset 0: VA = file offset +
  `0x8048000`.
- CoDExtended's `src/shared.h` (community struct headers): three claims rest
  on it, each labelled and cross-checked against an offset the binary uses.

An instruction run is written as a list opened by two labelled sentences:
operands, offsets, immediates and call targets are read out of the
instructions (VERIFIED); ordering and conditions are read off the branches
(INFERRED).

The measured side of the weapon machine, off two retail captures, is
`docs/research/player-model-anim-system.md`, "The weapon channel: what writes
`torsoAnim`". Where this document sharpens one of its INFERRED readings it
says which.

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
| `0x260` | `hipSpreadProneDecay` | `0x2F4` | `reloadStartAdd` |

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
frame take the `else` and store 0, after which the weapon can fire again. The
net probe taps the bit for that reason (`Pulse`, AGENTS.md "The probe
client").

VERIFIED, same block (`.so` 0x38842-0x388c4): after the store of 1 it
compares `weaponstate` against 4, then against 3, 4, 10 and 11. Both arms
hold the same inline write, `weapAnim = (weapAnim & 0x200) ^ 0x200` (the
idle index, the toggle flipped), behind three tests: `pm->cmd.weapon`
non-zero, `weapAnim & ~0x200` non-zero, and `ps+0x4` (`pm_type`) not above
5; both then store 0 into `weaponstate` at 0x388bd. INFERRED, off the
branches: a held semi-automatic leaves states 3, 4, 10 and 11 for state 0
on the cmd its `weaponTime` runs out, with the idle pose unless it already
shows it. The dll's arms (`0x30010050` and the index-preserving setter)
are the same two writes.

VERIFIED, capture: the latched rechamber end writes `weapAnim`: in
`mp_carentan-dm-slope-8ms-springfield-held-1.1.txt` (below) the rechamber's
516 reads 0 with `weaponstate` 0 and `weaponTime` 1 on the snapshot after
its time ran out under the held trigger. An unlatched rechamber ends with
`weapAnim` unchanged (pavlov's, `player-model-anim-system.md`, "What
`weapAnim` is not written by"): it ends in the trigger-release check
(`PM_Weapon` 0x392e7-0x39334), which stores 0 into `weaponstate` from any
state and writes the idle pose only from state 3 (VERIFIED disassembly;
that it is the path is INFERRED, off the order of the calls).

VERIFIED decompile (ghidra 0x487c0): the store of 0 into `weaponstate` sits
in the latched arm only; the unlatched arm stores 0 into `weaponTime` and
nothing else. INFERRED, off the call order: a released shot keeps state 3
past this function and the rechamber check of 1.9, which opens only from
state 0, and the trigger-release check of 1.6 ends it later in the frame, so
a bolt-action released before `fireTime` runs out rechambers one cmd after
its time does. VERIFIED, capture: `mp_carentan-dm-slope-8ms-springfield-1.1.txt`
(local 1.1d, the `--probe-moves` script with `--probe-weapon
springfield_mp`, 8 ms cmds) reads `weaponstate` 0 and `weaponTime` 0 on the
snapshot whose `commandTime` is the cmd each of its four shots' time ran out
on, and the rechamber's 162 on a later one (`moves_ab` compares it). VERIFIED, capture: the
per-snapshot combat captures catch that one-cmd idle on some of their
fourteen rechambers, a sample reading `weaponstate` 0 with the idle index
between the shot's 3 and the rechamber's 4 (`mp_pavlov-dm-combat.txt`
`single_shot` and `crouch_fire`, `mp_pavlov-dm-ads.txt` `ads_shot`,
`mp_carentan-tdm-ads-sniper.txt` `hip_shot`, `mp_cassino-dm-combat-uo.txt`
`sustained_fire` and `crouch_fire`: six of fourteen on the committed
captures, four on older takes); whether a snapshot lands on it is the cmd
grid's business. `playerstate_combat_ab`, whose 1.1 and UO captures all log their
cmds (1.13) and which replays them frame for frame, keeps it and compares
it. UO's jeep passenger
does the same (`coduo-vehicles.md` 7), and UO's timer (0x360fa) and 1.5's
(0x2f5ce) have the same shape (VERIFIED decompile). Pavlov's 3-to-4 step
spans a whole server frame of cmds, so it cannot show the idle either way.

A held trigger takes the latched arm, and the rechamber check (`.so`
0x375d0) lets state 0 start the rechamber while `weaponTime` is non-zero as
long as `weaponDelay` is 0: with `weaponTime` running it returns only for
states 3, 4, 10 and 11 or a running `weaponDelay` (0x37670-0x3769e,
VERIFIED disassembly). INFERRED from the two: a bolt-action whose trigger
is held through `fireTime` rechambers on the cmd its time runs out on.
VERIFIED, capture: `mp_carentan-dm-slope-8ms-springfield-held-1.1.txt`
(local 1.1d on stock paks only, the `--probe-autofire` script
with `--probe-weapon springfield_mp`, 8 ms cmds: the trigger held 1.5 s at
the hip and down the sight, then four taps) reads `weaponstate` 4 and
`weaponTime` 909 at commandTime 12490 after the expiry cmd stamped 12449,
950 ms of `rechamberTime` from that very cmd, then `weaponstate` 0 and
`weaponTime` 1 from the rechamber's end until the release, and no second
shot (`moves_ab` compares it, `weapAnim` included).

VERIFIED, `.so` 0x388d0: the unlatched arm stores 0 into `weaponTime` and
writes neither `weaponstate` nor `weapAnim`. VERIFIED: the melee finish
(`0x38eb4`) tests `weaponstate` 11 at 0x38f15, writes the index-preserving
idle (0x38f33-0x38f3b), stores 0 into `weaponstate` (0x38f48) and returns
1, which ends `PM_Weapon` (0x39294); it runs only past the gate at
0x3925a (an expired delay, or `weaponTime` and `weaponDelay` both 0). The
trigger-release check stores 0 into `weaponstate` whatever state reached
it (0x3932d); its idle write is gated on state 3 (0x392fa). INFERRED: an
unlatched relax (no `semiAuto`, or an empty clip) ends on the frame both
timers are out, and that frame neither fires, reloads nor swings; an
unlatched rechamber (4) stands until the release check or the next shot
ends it in that same frame. vcod: `pmove::weapon::advance_timers` leaves
both, `pm_weapon` ends 11 behind the gate and the release check ends any
state; `a_held_trigger_fires_the_frame_after_the_relax_ends`. No stock
capture holds one.

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

VERIFIED, `.so` 0x38af0-0x38b51: step 1's `adsFire` store replaces the
`fireDelay` one with `(1 - fWeaponPosFrac) * (1 / weapDef[0x414])`
truncated, before the ammo check; 0x38d53 is step 3's test of
`weaponDelay`, after step 2. VERIFIED: `PM_Weapon` calls the fire path
(0x39344) with the expired-delay flag the timers returned (0x391ff),
skipping the `weaponTime`/`weaponDelay` gate on it (0x3925a), and the ADS
lerp (0x372fc) runs at 0x39143, ahead of the timers (0x391fa). 1.5b's
`PM_Weapon` (0x303ad) and UO's call the same pieces in the same order (the
lerp first, the fire path `0x2ff54` / `0x36c0e` arming, then testing
`weaponDelay`). INFERRED: an `adsFire` pull from short of the sight fires
nothing, forces the sight up (section 1.13) and shoots on the frame the
delay runs out, whatever the trigger does by then; the re-arm then
computes 0 off the raised sight. A gun with a non-zero `fireDelay` re-arms
it on every pass and never fires; no stock gun has one (the four 1.1
grenades, which skip the arm, and UO's binoculars do). vcod:
`an_ads_fire_shot_waits_for_the_sight`,
`a_fire_delay_rearms_on_every_pull_and_never_fires`.

VERIFIED, `.so` `0x38d28`: the fire path calls step 1 (`0x38a44`) before
step 2 (`0x38bbc`), so a dry trigger has already stored `fireTime` into
`weaponTime`, 3 into `weaponstate` and the weapon's `weaponrechamber` bit
(for a `boltAction` weapon) when step 2 adds its 500. VERIFIED, `0x38c46`:
step 2's idle write is the index-preserving one (`weapAnim & ~0x200` tested
first). VERIFIED, live, `mp_carentan-dm-weapsel.txt` (local 1.1d,
`client-probes/probe_weapsel`): the first dry pull's snapshot reads
`EV_NOAMMO`, `weaponstate` 3, `weaponTime` 615 and `weapAnim` 0 on the
carbine (`fireTime` 0.135 plus 500, 20 ms in; another run read 618), and
state 3 holds until the time runs out. Trap: step 2 alone leaves
`weaponstate` 0, `weaponTime` 500 less a frame and the idle written with the
toggle flipped. vcod: `pmove::weapon::fire`, pinned by
`an_empty_weapon_with_no_reserve_clicks` and `weapsel_ab`'s
`the_dry_pull_holds_the_firing_state`. INFERRED: step 7's condition is what
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
- VERIFIED, the same two tests in 1.5b's `game.mp.i386.so` (the
  begin-reload at 0x3df40, `ammoclip` against 0 and `weaponDef+0x70`
  against 0) and UO's `game.mp.uo.i386.so` (0x34585 and 0x3459b, UO's
  `weaponType` at `+0x7c` and `reloadEmptyTime` at `+0x244`, its event
  0x9a). VERIFIED, `pakuo00.pk3`: `bazooka_mp` and `panzerschreck_mp` are
  `weaponType projectile` with `reloadTime 6` and no `reloadEmptyTime`.
  INFERRED: a dry rocket launcher reloads over `reloadTime` with
  `EV_RELOAD`. vcod: `WeaponDef::is_bullet`,
  `a_dry_rocket_launcher_reloads_over_reload_time` (the empty branch on a
  launcher is a 0 ms reload that never loads the clip).
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
- Same function, ending the start segment: from state 7 or 8 with
  `weaponTime` 0 it tests `ammoclip` against 0 on the state-8 arm, calls the
  reload test, and has the same three exits the loop segment has: a loop
  segment through `0x30010330`, state 9 with `WEAP_RELOAD_END`,
  `reloadEndTime` and `0x9A`, or state 0 with `WEAP_IDLE`, the
  `weaponrechamber` bit cleared ahead of the last two. A loop segment starts
  only when the test still passes and the state is not 8 with rounds in the
  clip; otherwise the reload ends there. In the `.so` this is `0x3820c`,
  the test `0x3803c`, the segment starter `0x378bc` and the bit
  `Com_BitClear`.
- dll `0x3000fea0` (`.so` `0x37500`), the add the delay edge lands on: in
  state 7 or 8 it reads `weaponDef->reloadStartAdd` (`0x2F4`) and returns
  without a store when that field is 0; otherwise it reads `reloadAmmoAdd`
  (`0x2F0`). Either count is compared against the clip size and the room
  left in the clip, and the smaller of count and room lands, the whole room
  when the count is 0 (loop only) or not below the clip size.

VERIFIED: the offsets, immediates and call targets in the two items above,
and that `0x2F4` is `reloadStartAdd` (field-table record 157 at VA `0x7D0FC`
in the `.so`, type 4). INFERRED: the "only when", "otherwise" and "returns
without a store" in them, which are branch conditions. VERIFIED: the three
stock segmented files ship `reloadStartAdd` equal to `reloadAmmoAdd`
(enfield 5, kar98k_sniper 1, springfield 1), so no stock capture can tell
the two fields apart. VERIFIED, `mp_carentan-tdm-ads-sniper` `ads_reload`
and `ads_release`: after one shot from a full five, the reload reads
`weaponstate` 7 with `weapAnim` 13 and event 153 for `reloadStartTime`,
its `weaponDelay` counting 1399 to 0 from `reloadStartAddTime` (1367 and
1366 on older takes; the first sample's phase), then 9 with
`weapAnim` 526 and event 154 for `reloadEndTime`, then 0; state 5 never
appears. INFERRED: the start's one-round add filled the clip, the reload
test refused a loop segment, and the reload went straight to its end.

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
`cmd.weapon`, 0 before and the held weapon after.

### 1.8 The weapon-switch path from the usercmd `weapon` byte

VERIFIED: the offsets, immediates, weapon-def fields, event numbers and
call targets named in the list below. INFERRED: the ordering, and every
"when", "unless", "otherwise" and "skipped" in it, which are branch
conditions.

- dll `0x300112e0`, the check: while `weaponTime != 0` and `weaponstate`
  is not one of 4, 5, 6, 7, 8, 9, it returns for `weaponstate` 3, 10 or 11
  and when `weaponDelay != 0`; with `weaponTime` 0 it runs whatever the
  state (VERIFIED decompile; UO's 0x362ea has the same compares, VERIFIED
  disassembly). INFERRED: so the cmd a released shot's time runs out on,
  still in state 3, reaches the switch, as does a grenade pullback, which
  holds `weaponTime` at 0.
- Same function: with `pm_flags & 0x10` set and `ps->weapon` non-zero it
  begins a change to weapon 0. VERIFIED, cgame `0x300112e0`: with the bit set
  and `ps->weapon` 0 it returns before `cmd.weapon` is read, so a climber
  with empty hands starts no change whatever its cmd byte says (9.7).
- Same function: otherwise, when `pm->cmd.weapon` differs from `ps->weapon`,
  it begins a change to `cmd.weapon` provided `cmd.weapon` is 0 or the
  matching bit is set in `ps->weapons` (`ps+0x30C`), and it skips that when
  `pm_flags & 0x4000` is set with a non-zero current weapon. INFERRED,
  `cod11-mantle.md` ("`pm_flags` 0x4000") and `coduo-diff.md` for 1.1d and
  UO: no server sets that bit. 1.5b's `game.mp.i386.so` holds one
  `or $0x4000` (0x530e7, in `turret_use`), and it lands on
  `client+0x80`, `ps.eFlags`'s `EF_MOUNTED_PRONE`, not on `pm_flags`
  (`ps+0xc`) (VERIFIED objdump; no `orb $0x40` into `ps+0xd` either). vcod
  models no 0x4000 skip on any of the three.
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
  VERIFIED, `0x37bb5` and `0x37bee`: both events go through `PM_AddEvent`,
  whose parm is 0. VERIFIED, `0x37b87`: the spent `clipOnly` test skips the
  `WEAP_DROP` write with the event. VERIFIED, `0x37c94`..`0x37d7a`, the alt
  path's tail: it walks the current weapon's `altWeapon` chain for the slot
  one of its modes stands in (types 1 and 2 in slot 1 or 2, types 3 to 5 in
  their own slot) and writes the target into that slot when the target is
  held and of a slot type that fits it, so `weaponslots` names the mode in
  hand.
- dll `0x300107c0`, `.so` `0x37d84`, the alt arm of the pickup: VERIFIED, a
  new weapon that is the old one's `altWeapon` takes `weaponTime` from
  `altRaiseTime` (`+0x20c`, `0x37ef4`) with no `EV_RAISE_WEAPON`, raises
  `aimSpreadScale` to at least 128.0 (`.rodata` `0x72534`, `0x37f81`)
  rather than setting 255.0, and writes `WEAP_ALTSWITCHTO` (16).
- dll `0x300107c0`, the pickup half, which runs only in state 2: the new
  weapon is `cmd.weapon`, forced to 0 when `pm_flags & 0x10` is set, when the
  player does not own it, or when it exceeds the weapon count.
  vcod: `weapon::pickup` reads the cmd's byte too, so a putaway whose cmds
  go back to the held weapon brings it back to idle. `switchToWeapon`
  writes no playerstate; a bot answers its `a <n>` (9.8) by moving its own
  byte (`Bot::observe`), as a cgame's selection would.
- Same function: it writes `ps->weapon`, refreshes the cached weapon def, and
  when old and new are equal sets `weaponstate` 0 with `WEAP_IDLE`.
- Same function: otherwise it sets `weaponstate` 1, raises event `0x9B`
  (`EV_RAISE_WEAPON`, 155) unless the new weapon is the old one's alt, sets
  `weaponTime` from `raiseTime` (or `altRaiseTime`), sets `weapAnim` to
  `WEAP_RAISE` (or `WEAP_ALTSWITCHTO`), and sets `aimSpreadScale` to 255.0.
- Same function, the arm taken when the old weapon is 0 (read in the cgame
  copy at `cgame_mp_x86.dll` `0x300107c0`, whose `BG` code the game module
  shares): VERIFIED, it stores `weaponstate` 1, `weaponTime` from the new
  weapon's `raiseTime` (`def+0x204`), `aimSpreadScale` 255.0 and `WEAP_RAISE`
  (10) and returns, and the function's one store of `0x9B` into the event
  ring (`ps+0x88 + (eventSequence & 3) * 4`) is outside that arm. INFERRED:
  a raise out of empty hands raises no `EV_RAISE_WEAPON`, and a change to
  weapon 0 from a real weapon does, weapon 0 raising on its own def's
  `raiseTime`. VERIFIED, three captures agree: the grenade capture's
  `throw_down` moves `eventSequence` 15 to 16 with a 155 on the frame
  `weaponstate` leaves 3 after the last frag, a step that ends on `weapon`
  0 (9.5); every `kill` death frame in the
  `dm` hit-target capture reads 189 then 155 (8.1), with `weapon` 0 in the
  follow runs (`cod11-spectator-follow.md` 5);
  and both swaps in `mp_carentan-dm-pickup.txt` read `weapon` 0 to the new
  weapon at 34800 and 36300 with `eventSequence` unmoved.
- dll `0x30010a30`, inlined in the `.so` at `0x392b4`: `weaponstate` 1 is
  cleared to 0 on the frame after, unconditionally. VERIFIED decompile
  (`PM_Weapon`, the same block): the `WEAP_IDLE` write beside it, toggle
  flipped, is guarded by `pm_type < 6` and `pm->cmd.weapon != 0`, the
  guard of 1.2's setter, and so is the trigger-release idle of 1.6 in the
  arm after it. VERIFIED, capture (`mp_carentan-tdm-grenade.txt`): the last
  frag's `throw_down` raises empty hands (`weaponstate`
  1, `weapAnim` 10) on the frame its last cmd ran the throw's end, the probe
  sends `cmd.weapon` 0 for the weapon it no longer holds, and the state goes
  0 a frame later with `weapAnim` still 10 through `idle_after`. VERIFIED,
  an older take: it went 3 to 0 on one frame and read 512.
  INFERRED: that frame's cmds still carried 8, the probe echoing a
  snapshot from before the take. vcod: `pm_weapon` puts `weapAnim` back after a
  step whose cmd byte is 0.
- VERIFIED, 1.5b's `game.mp.i386.so`: the setter at 0x2d9b8 returns
  unless `pm->ps->pm_type` (`[pm]+0x4`) is at most 5 (0x2d9d1) and
  `pm->cmd.weapon` (`pm+0xa`) is non-zero (0x2d9e1), then writes
  `(weapAnim & 0x200) ^ 0x200 | anim` to `ps+0x3d4` (0x2d9fd..0x2da10);
  the index-preserving setter at 0x2da19 tests the same cmd byte
  (0x2da33) and compares `weapAnim & 0xfffffdff` (0x2da4b) before
  calling it. The GOT slot both read (`0xe10(%ebx)`, 0x89ba8) is
  `R_386_GLOB_DAT pm`. VERIFIED, UO's `game.mp.uo.i386.so` (Ghidra
  0x43f9c and 0x44024, file 0x33f9c and 0x34024, slot 0xb4800 `pm`):
  the same two guards, `weapAnim` at `ps+0x624`. So 1.1, 1.5b and UO
  all refuse the write on a cmd byte of 0.
- VERIFIED, UO only, 0x33fcd..0x33ff2: past the guards the setter
  compares `[pml+0x88]+0x7c` against 4 and on equal stores the bare
  index, the toggle cleared. `+0x7c` is UO's `weaponType` (above,
  section 1.7) and 4 its `gas` (coduo-diff.md, the fire events' row);
  the name table at `.data` file offset 0xab468 starts `bullet`,
  `grenade`, `projectile`, `spotter`, `gas`. VERIFIED, `snapshots-uo-flame.bin`: the followed
  flamethrower's `weapAnim` reads 2 in every burst and 0 between them,
  never with 0x200, from 33050 to 35400. INFERRED: `pml+0x88` is the
  held weapon's def. vcod clears the toggle after a UO step on a `gas`
  weapon whose `weapAnim` changed (`pm_weapon`).

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
other one, and the semi-automatic latch of 1.4 is what holds it open. 1.14 has the numbers.

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
for it. Neither a jump nor a stance change puts a weapon away on its own.

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
- `.so` `0x375d0` is the same check. VERIFIED: it returns 1 at one site only,
  after the `0xA3` event with `weaponTime` (`ps+0x2C`) non-zero, and 0 at
  every other exit, the start of a rechamber included. INFERRED: `PM_Weapon`
  skips its prone-move cone store and the rest of the machine only on that
  one frame; a rechamber that starts or runs leaves the cone open for a
  crawling player.

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
the prone move key arm ("The prone move key", below). VERIFIED: `PM_UpdatePlayerWalkingFlag` (`0x33694`) sets
`pm_flags 0x80` only when `0x20` is set, the player is not prone, and
`weaponstate` is not one of 5, 6, 7, 8, 9. INFERRED: `0x80` is the ADS walk
slow-down, and it is off for the whole of a reload while `0x20` stays on.

INFERRED, from the two rules together: the sight can be *asked for* during a
reload -- the flag update excludes only `weaponstate` 1, 2, 10 and 11 -- but
the fraction's target is 0 until the reload has `adsReloadTransTime` left
to run, which for `m1carbine_mp` is the last 600 ms of 2650 and for
`mosin_nagant_mp` the last 600 of 2400. So the sight comes up at the tail of a
reload and not at the key.

**What the `--save-ads` captures measured**, one per gate map
(`crates/server/tests/fixtures/playerstate/<map>-dm-ads.txt`, `!trace` per
snapshot with `fWeaponPosFrac` and `aimSpreadScale` as decimals), the probe
standing still and holding each input in turn. VERIFIED, every number;
INFERRED, that they are the file's fields, which is arithmetic.

- The ramp is linear and the rates are the reciprocal times: up, both
  weapons step about 0.1667 a snapshot, the carbine reading 0.0533 at 17 ms and
  1.0 at 315, the mosin 0.0533 at 16 and 1.0 at 308, which is
  `adsTransInTime` 0.3 s; down, the carbine steps about 0.125 a snapshot and
  reaches 0 at 402 ms (`adsTransOutTime` 0.4) and the mosin about 0.0833
  a snapshot, 0 at 632 (0.6).
- Through a reload with the sight held: the fraction ramps *down* from the
  reload's first frame at the out rate (carbine 1.0 to 0 over 430 ms, mosin
  over 670), sits at 0, and ramps back up at the in rate from about 2080 ms
  on the carbine (`reloadTime` 2650 less 600) and 1840 on the mosin (2400
  less 600), reaching 1.0 some 300 ms before `weaponstate` leaves 5. A
  reload asked for on a full clip does nothing.
- The sight released mid-ramp turns straight round: carentan's
  `ads_release_2` reads 0.8750 at 48 ms off a fraction that read 1.0 on the
  step's first sample, and `idle_after` 0.8775 at 48 ms the same way.

The trace fields in a snapshot are the state after the last of that
frame's pmove steps, and the probe sends a cmd every 16 ms, so a 48 ms gap
holds three steps. That is why the counter below can read a decay on the
same snapshot the fraction first reads 1.0.

**The cmds behind each sample.** The combat and sight captures log every
cmd the probe sent while a step was current (`!cmds`: each `serverTime`, the
`buttons`/`wbuttons` that differ from the step's input, the weapon byte
where it changes, `w=`, the absolute view shorts before `delta_angles`
comes off, `a=`, and `forward`/`right`/`up`, `m=`) and each snapshot's
`commandTime`; the first step's log opens on the cmds already sent past its
first snapshot's `commandTime`. VERIFIED, the four 1.1d captures
(`mp_carentan-dm-combat`, `mp_pavlov-dm-combat`,
`mp_pavlov-dm-ads`, `mp_carentan-tdm-ads-sniper`): a snapshot's frame can
run fewer cmds than their stamps put before it, 15 of 1774 snapshots across
the four, as at carentan's `idle` 29300, whose `commandTime` is 29282 while
the probe had sent 29298 too; the rest run in the next frame. VERIFIED: a
frame whose last cmd is the tap reads exactly `hipSpreadFireAdd` 0.7 times
255, 178.50 on the carbine (carentan `sustained_fire` 32150, `commandTime`
32132, the tap's stamp). INFERRED: the shot's own cmd decays before it
adds, since `PM_AdjustAimSpreadScale` runs ahead of the fire (2.1). vcod: `playerstate_combat_ab` replays each
logged step's cmds at retail's stamps, each frame running up to the
`commandTime` retail's snapshot read (`common::CmdLog`), and then the
sight fraction and the spread counter match on every sample to the
capture's print, the `weaponstate` runs and `weapAnim` indices exactly.

Every 1.1 capture `playerstate_combat_ab` replays is logged this way, and
the gate compares every channel sample for sample with no slack (VERIFIED,
`mp_carentan-dm-ads` and `mp_carentan-tdm-grenade`: 2 of 397 and 5 of 665
snapshots behind their stamps). VERIFIED, the grenade capture: the replay
needs two inputs. The switch's byte: `to_frag`'s first cmd carried the
rifle's 12 and the frag's 8 only from the second on, where a replay that
asked for the switch on the step's clock put the putaway a frame early
(14 samples of `weaponstate` 2 against 13). And the cmds a first step
opens on, which the probe sent before the step's clock ran and only the
traces' `commandTime`s name: left out, `melee_tap`'s `weaponDelay` read
100 against retail's 134 a cmd after the press. INFERRED: the first logged
cmd's msec then spans them and pmove chops it, the press landing in the
first chop. Trap, VERIFIED on older takes: a probe that stalls between a
script's first snapshot and its first cmd (100 to 200 ms reading the weapon
file) folds the stall into the first cmd, on the grenade script the whole
swing in one chopped cmd (`weaponDelay` 15 on its first sample); the probe
starts the first step an iteration after the read.

UO's `mp_cassino-dm-combat-uo` and `-combat-uo-stall` (whose `advance` walk
stalled) and 1.5b's `mp_carentan-dm-combat-15` replay through the same
exact comparison on protocol 22 and 6, with no gap (VERIFIED, gate;
`coduo-diff.md` 20, `cod15-protocol-diff.md`). A step whose line carries
`a=` replays retail's view and movement cmd for cmd (`common::CmdLog::aim`),
so walks are compared too, and a walk with no `a=` fails the gate. Trap,
VERIFIED on an uncommitted 1.5b take: with the first step's pre-start cmds
unlogged the replay opened `advance` on the last traced `commandTime`
(31685) and ran the first walking cmd (31733) as 48 ms of walking, the
spread counter reading 48.96 where retail read 17.34, one 17 ms cmd of
`hipSpreadMoveAdd` over decay (the probe had sent a standing cmd at 31716
that ran in the same frame).

VERIFIED, gate: both 1.1d `advance` walks stalled and turned twice (+45,
then +90 off the spawn's facing), and the replay matches them sample for
sample. Teeth: a +22.5 degree turn injected at carentan
`advance`'s cmd 187 fails 30 of its 121 samples, a turn at cmd 40 of the
ads `walk` 17 of 30 (VERIFIED, gate). A gate that puts the body at the
header's spawn (`common::replay_as`) sends the raw cmd angle
`place_client`'s delta assumes, 0, from the first settle frame: rebased
off the stale snapshot's `delta_angles`, the first two cmds turned the
view out and back, UO's `hipSpreadTurnAdd` opened the cone, and the
first sample of the new UO `advance` read 172.12 where retail's read
0.00 (VERIFIED, gate).

**As implemented.** `crates/common/src/pmove/weapon.rs` carries all of the
above: `update_ads_flag` the flag, `ads_pm_flags` the `0x20`/`0x80` pair for
the wire, `advance_ads` the ramp, and `fire` the `adsFire` `weaponDelay`
override. Three divergences, each deliberate. `pm_flags 0x400` is not
modelled, since it is unnamed (section 10). The dead clear lives in
`pmove::dead_move` rather than in the flag update, because no pmove step runs
for a dead player at all, which leaves the fraction frozen where the death
found it rather than ramping it down. And `pml.groundPlane` is read as
`ps.on_ground`, the nearest thing vcod's mover carries. `advance_ads` runs first in `pm_weapon`, ahead of
the grenade and the timers, as 0x39143 does, so the reload window and the
`adsFire` force read the timers the frame starts with.

#### The prone move key

`PmoveSingle` opens with a test on the cmd's moves, ahead of the view
update and of every flag (1.1d 0x33e75..0x33f2d, 1.5b 0x27fde with its
call at 0x2808f, UO 0x2d67b with its call at 0x2d72c).

- VERIFIED, 1.1d 0x33e75..0x33efe: the tests are `pm_flags` 1 (prone),
  `cmd.forwardmove` against `oldcmd.forwardmove` and the strict compare of
  their magnitudes (`fcompp` of the two `fabs`, `and ah,0x45; cmp ah,1`,
  C0 alone), the same for `rightmove`, the call to
  `PM_InteruptWeaponWithProneMove`, then `and [pm_flags+1],0xfb` and
  `PM_ClearAimDownSightFlag`; 1.5b's and UO's decompiles read the same.
  INFERRED, off the branches: on a prone body whose forward or right move
  differs from the last cmd's and grew in magnitude, the arm calls the
  interrupt, and when that returns non-zero clears 0x400 and the sight.
- VERIFIED, `PM_InteruptWeaponWithProneMove` (1.1d 0x37488, 1.5b 0x2d4bc,
  UO 0x338b0), its compares and stores; INFERRED, off its branches: it
  returns 1 for `weaponstate` 0, 1, 2 and 5 to 9, and 0
  for 3 (firing) and 11 (melee relax). Any other state (1.1's and 1.5's
  4, the rechamber, and 10, the swing's windup) it sets to 0 with
  `weaponTime` and `weaponDelay` 0, calls the weapon anim helper with
  `WEAP_IDLE` (1.1d 0x3ae30, which writes only when `cmd.weapon` is
  non-zero, `pm_type` is under 6 and the anim differs) and returns 1.
- INFERRED, off UO 0x338b0's branches: it returns 0 first for a sighted
  (`pm_flags` 0x20) weapon of `weaponClass` 3, an LMG on its bipod, and it
  lists state 4 among the ones it keeps. So on UO the states it drops are 10, 12 and 13.
- VERIFIED, live (`--probe-pronemove rifle`, springfield_mp, fixtures
  `playerstate/mp_carentan-dm-slope-8ms-pronemove-rifle-{1.1,1.5,uo}.txt`):
  a forward or right press under the sight clears 0x20 on the press's cmd
  and the body crawls; a forward press 500 ms after a shot raises
  `EV_RECHAMBER_WEAPON` again on 1.1d and 1.5b with `weaponTime` back at
  924 (the rechamber restarts: the bolt bit is still set, so the next
  `PM_Weapon` starts it from state 0), and UO's runs on; a forward press 50
  ms into a swing's windup takes state 10 to 0 and `weapAnim` to 512, so
  the swing never lands. `--probe-pronemove lmg` (mg30cal_mp and
  binoculars_mp, `-lmg-uo.txt`): a press on the deployed bipod changes
  nothing, a press in its breakdown restarts the breakdown (state 0, then
  the deploy driver's breakdown again, its `LMG_BREAKDOWN` restarting the
  torso), and a press 100 ms into the binoculars' raise takes state 12
  to 13.

vcod: `weapon::prone_move_key` from `cmd::player_step`, ahead of the view
and of `pmove`, reading `PlayerState::last_cmd_moves` (the raw previous
cmd, as `last_cmd_ads`). `moves_ab` replays all four captures; before the
port the 1.1d one missed 10 fields (the sight, the origin, the weapon
state and timers) on 1 to 3 snapshots each, and UO's lmg one 10 fields.

---

### 1.14 What the grenade and melee capture measured

`crates/server/tests/fixtures/playerstate/mp_carentan-tdm-grenade.txt`, a lone
`--save-grenade` run against the retail 1.1d server: one melee swing, a switch
to the frag, a cooked throw, a cook held past the fuse, a cook cancelled by
switching back, and a throw at the ground. The step names below are the
fixture's own. VERIFIED: the committed take holds the spawn point's yaw
(-180 at (976, -792, -4.5)) with every cmd logged (1.13); an older take from
the same spawn flies the same three arcs, every bounce's `pos` equal to the
print. The first capture (commit `c52e9606`) held yaw 90 at (1200, 1760,
144), and the first `!missile` line of each of its throws already carries a
bounce (177).

**The fuse does not run down.** VERIFIED: `grenadeTimeLeft` reads a flat 4000
(`fuseTime` 4) from the pullback frame to the throw frame and 0 on every other
sample, under a 200 ms, a 1000 ms and a 5000 ms hold alike. VERIFIED:
`pin_out` holds the trigger 5000 ms and the throw's event 159 lands on the
release, at +5048 ms (+5038 and +5032 on older takes), with the explode 3950 ms of server time after the release snapshot. INFERRED: 1.11's
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

**The raise ends when `raiseTime` does.** VERIFIED: `weaponstate` 1 holds
6 snapshots (300 ms of server time) on `to_frag` and 5 (250) on
`to_frag_2`, 306 and 241 ms on the probe's clock (258 and 245, 295 and 263
on older takes), against the frag's
`raiseTime` of 250 and inside a snapshot's 50 ms, and 2.7 s on `cancel`,
whose trigger is held through the raise.
INFERRED: the state ends on `weaponTime` reaching 0 and the semi-automatic
latch of 1.4 pins it at 1 for as long as the bit is down, so a held trigger
keeps a weapon coming up.

**The swing.** VERIFIED: `melee_tap` reads `weaponstate` 10 with event 164 and
`weapAnim` 520, then 11 with event 165 once `meleeDelay` (0.15 on the carbine)
runs out, 150 ms of server time later, then 0 with `weapAnim` 0 at
`meleeTime` (0.65), 500 ms after that, and `torsoAnim` 729 across both
states (732 on the first capture). VERIFIED: the standing
`meleeattack` clause of `mp/playeranim.script` lists five torso swings.
INFERRED: 729 and 732 (indices 217 and 220) are two picks from that list. VERIFIED: `aimSpreadScale` reads 255.00 at every sample of
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

**A `both` land clause is not.** VERIFIED, first capture (`c52e9606`): the
sample after the one where `throw_down`'s frag knocks the thrower off his
feet reads `legsAnim` 100,
index 100 with the toggle clear, between two samples of 634, and `torsoAnim`
512 across all three. VERIFIED: the landing clause the stance and class
select is `weaponclass pistol AND grenade`, whose body is
`both pb_standjump_land_pistol duration 5`. INFERRED: the land
event puts its anim on the legs and leaves the torso alone, so the rule above
is the throw's and not every `both` clause's; the single sample is the
`duration 5`. VERIFIED: the later takes' thrower takes no damage (no 187 `EV_PAIN`, `damageEvent` 0 to the end) and the step
holds no land sample.

INFERRED, and unmeasured: vcod applies the throw's rule to the rest of the
event clauses, so it also reaches `fireweapon`'s pistol-ADS clause and
`jump`'s two run clauses. No capture covers either.

**As implemented.** `pmove::weapon`'s `pullback`, `grenade_hold`,
`melee_check` and `melee_finish`, with the two-arm `pickup` and the short
`putaway` branch beside them, and `spectate.rs::play_event` for the two
paragraphs above it, whose landing branch clears the torso of the selection
before it plays it. `crates/server/tests/playerstate_combat_ab.rs`'s
`the_grenade_and_melee_channels_match_retail_on_mp_carentan` replays the
whole capture, from the spot the capture's own header
records: where a blast lands, and so who it hurts, is the map's business.

## 2. `Bullet_Fire_Extended`: spread, the trace, and what a bullet does

### 2.1 Where the spread number comes from

VERIFIED, `FireWeapon` `0x68d68`: the shot's view angles are
`ps->viewangles` with components 0 and 1 replaced from `client+0x220C` and
`client+0x2210`, and `AngleVectors` turns them into a forward/right/up axis
triple. VERIFIED: the muzzle is the entity's origin with
`ps->viewHeightCurrent` (`ps+0xD0`) added to z (`0x68e14`), and the function
calls `G_AddLean` on it (`0x68e25`) and rewrites each component through
`fistp` and `fild` under a control word `| 0xc00` (`0x68e34`, `0x68e5c`,
`0x68e84`), rounding control 11, which truncates toward zero, the same as
`CalcMuzzlePoints` (`0x6948c..0x694dc`, object-model doc 23.1). INFERRED,
off the three sitting past the call: the lean is applied before the
truncation.

The origin it starts from is already whole. VERIFIED: `ClientThink_real`
calls `BG_PlayerStateToEntityStateExtrapolate` (`0x2d004`, call at
`0x404f2`) or `BG_PlayerStateToEntityState` (`0x2cbe8`, call at `0x40510`),
picked on `g_smoothClients` (relocation at `0x404cc`), each with a snap
argument of 1 (`0x404d3`, `0x40503`); both truncate `s.pos.trBase` under a
control word `| 0xc00` when that argument is non-zero (`0x2cc30..0x2cca2`,
`0x2d1db..0x2d25a`); `ClientThink_real` copies `s.pos.trBase` into
`r.currentOrigin` at `0x4051e..0x40533`, ahead of the `ClientEvents` call at
`0x40589`, and writes the unsnapped `ps.origin` (`client+0x14`) back into
`r.currentOrigin` at `0x405c7..0x405dc`, past `G_TouchTriggers`. INFERRED,
off that order: `FireWeapon` reads a truncated origin, so the muzzle is
`trunc(trunc(origin) + (0, 0, viewHeightCurrent) + lean)`, which differs from
`trunc(eye)` by one unit on z wherever the feet sit at a negative fractional
height. VERIFIED: both throws of the pair capture
(`mp_carentan-tdm-grenade-shooter.txt`) stand at z -23.9 and leave from
`trBase` z 37, where `trunc(eye)` is 36 (11.4).

VERIFIED: the melee swing's muzzle is built by `FireWeaponMelee` (`0x69504`),
which calls `Weapon_Melee` (`0x69614`): the same `ps+0xD0` add (`0x6957e`),
the same `G_AddLean` call (`0x6958f`) and the same three truncations
(`0x6959e`, `0x695c6`, `0x695ee`). INFERRED, off the addresses: the same
order.

**As implemented.** `combat::muzzle_point`, shared by the shot, the swing and
the throw (11.3), truncates the leaned eye per component.

**Measured, the leaned muzzle.** `mp_carentan-tdm-lean.txt` (`--save-lean`,
1.1d, axis, `kar98k_sniper_mp`): seven scoped shots standing
still at a wall 270-550 units out along wire yaw 54, level, leaning left
twice, right twice, level again and crouched leaning left. With the sway of
15.6 taken off the view, the impact sits off the ray through the unleaned
eye by `(-14.5, 10.6, -6.4)` leaning left, `(16.6, -12.2, -6.4)` leaning
right and `(-10.6, 7.7, -4.1)` crouched, where the model above (offset
`lean_offset(yaw, leanf)`, then the three truncations) gives `(-15.0, 11.0,
-6.0)`, `(16.1, -11.8, -6.0)` and `(-11.0, 8.0, -4.0)`. The level shots sit
`(0.4, -0.3, -0.2..-0.8)` off, the same constant, so every residual is
inside one unit once it is taken out. VERIFIED (capture). The same capture
puts `leanf` on record: it moves about 0.143 a 50 ms frame toward a held
side (settled at -1 362 ms in, and from -1 to +1 in 721 ms), about 0.179 a
frame back to centre when released (308 ms from 1), and a crouched lean
settles at -0.5. VERIFIED (capture). `crates/server/tests/lean_ab.rs`
replays it with each step opened on retail's level clock, so both sides
sway alike: every round leaves on retail's snapshot and strikes retail's
whole-unit impact, and each ramp settles within one frame of retail's.
VERIFIED, vcod measurement 2026-11-14. On our own clock (about 13.7 s
behind) the offsets read up to 1.03 apart, the sway model's and the two
snaps' error.

#### 1.5 and United Offensive

Neither truncates. VERIFIED, 1.5b `game.mp.i386.so` `CalcMuzzlePoint`
(0x7e91f; Ghidra's addresses, image base 0x10000): the muzzle is
`ps->origin` (`ps+0x14`, the unsnapped origin, not `r.currentOrigin`)
with `viewHeightCurrent` (`ps+0xd0`) added to z. Then, unless the
`eFlags` mount bits 0xc000 (`ps+0x80`) are set: z gains
`BG_GetVerticalBobFactor` (0x40a54), the point moves along the right of
`ps->viewangles` (`ps+0xc0`, `AngleVectors` third argument) by
`BG_GetHorizontalBobFactor` (0x40b37), both fed `BG_GetBobCycle`
(0x40a05) and `BG_GetSpeed` (0x38af6) capped at `bg_bobMax`, then
`G_AddLean` (0x4d20d, `AddLeanToPosition` with 16 and 20 as on 1.1), and
z is raised to no less than `origin[2] + 8` (rodata 0x8f1c4 = 8.0).
VERIFIED. `CalcMuzzlePoints` (0x7eb1d) builds the axis off the aim pair
(`ps+0x2214/0x2218`) and calls it for the shot, the rocket and the throw
(`FireWeapon` 0x7ebca); `FireWeaponMelee` (0x7ed84) calls it too. VERIFIED.
The bob factors are the cgame's eye bob (`aim::eye_bob` in vcod): the cycle
`2pi + 2 * pi * (bobCycle / 255)` stored as a float (0x87640 pi, 0x87638
255.0, 0x87648 2pi), the vertical `(sin 2c + 0.2 sin(pi/2 + 4c)) * 0.75 *
amp` (0x87650 4.0, 0x87658 pi/2, 0x87660 0.2, 0x87664 0.75), the
horizontal `sin c * amp`, `amp` the speed times the `viewHeightTarget`
stance's `bg_bobAmplitude*`. VERIFIED (rodata read from the file).

VERIFIED, UO 1.51b `game.mp.uo.i386.so` `CalcMuzzlePoint` (0x8b8ea): the
same shape off UO's playerstate (`viewHeightCurrent` `ps+0xf8`, `eFlags`
`ps+0x84`, viewangles `ps+0xe8`), with three changes. A rider (`eFlags`
0x100000, UO's in-vehicle bit) takes 28 (rodata 0xb5f88) in place of the
eye; the mount test is UO's 0x6000; and between the mount test and the
bob, a weapon of `weaponClass` 3 (`lmg`) with `pm_flags` 0x20 (the bipod)
traces an 8-unit box (.data 0xbe93c/0xbe948, -8..8) from the muzzle 19
forward (rodata 0xb5f8c) along `(0, proneDirection, 0)` (`ps+0x5a4`) under
mask 0x11, passing the shooter, and takes the end point unless the trace's
byte +0x2e is set. The floor is 8 (0xb5f90). VERIFIED. INFERRED: byte
+0x2e is `allsolid` (`cod11-mantle.md`'s ground trace reads it so).
`FireWeapon` (0x8bd05) and `FireWeaponMelee` (0x8bef7) call it as 1.5's
do. VERIFIED.

**Measured.** `mp_carentan-scriptmodel-throw-1.5.txt` (1.5b,
the prone part with the thrower's origin logged every frame of his first
second and as each frag shows): the thrower slid from the site to
(1368.0339, 2608.136, -23.874777) and both frags leave from (1368.034,
2608.136, 36.125), z bits 0x4210803a: his origin plus 60, nothing
truncated. VERIFIED (capture). The earlier
`mp_carentan-scriptmodel-prone-1.5.txt` throws left from (1368, 2608,
36.148) on the same site, so that thrower's feet stood at -23.852 (where
1.1d's point is 37.0); INFERRED, off the rule. UO's
`mp_carentan-scriptmodel-throw-uo.txt` (1.51b): the thrower slid to
(1368.204, 2608.102, -23.874743) and both frags leave from z bits
0x42108044, the float of his z plus 60. VERIFIED (capture). An older UO take
read the thrower at (1368, 2608, -23.875) and its frags at z 0x42108002,
36.125 and two float steps. INFERRED: his z was -23.874992, which six
significant digits print as -23.875.

**The thrower's slide.** VERIFIED, `game.mp.i386.so` 1.5b player
`setOrigin` (0x3e148) and UO 1.51b's (0x467a4): unlink, `ps.origin` set
to the argument, z plus the rodata constant, `eFlags` 0x8 flipped,
`BG_PlayerStateToEntityState`, `r.currentOrigin` copied, link. Neither
writes `ps.velocity`. VERIFIED, live (1.5b and UO 1.51b,
`client-probes/probe_scriptmodel` prone part, eight 1.5b and ten UO
runs, the thrower probe's snapshots logged with velocity and
`commandTime` and its cmd times): a thrower `spawn`ed on 1.5b's tdm
point at (976, -864) leaves the spawn's think at (976.016, -863.939,
-5.89044) on its grade moving (1, 4, 0) on the ground; UO's at (976,
-792) leaves at (976.077, -791.959, -4.50126) moving (6, 3, -1). The
script's `setOrigin` onto the site that frame (z -22.75, a unit and an
eighth over the floor) keeps the velocity. The net probe sends its cmds
16 or 17 ms apart from each snapshot's time (12000, 12016, 12032 before
the 12050 frame), so the next frame's `commandTime` is 12032 and he has
fallen 32 ms, 0.6 off the site; the script's `distance > 0.5` check sets
him back on it with the velocity, now (1, 4, -26), and the next frame's
cmds land him at (1368.0339, 2608.136, -23.874777) on 1.5b and (1368.204,
2608.102, -23.874743) on UO, where friction stops him. A spawn that lands
still (1.5b's (1080, 3488), UO's (1800, 2696) and others) puts him at
(1368, 2608, -23.852). INFERRED: the slide is the spawn's landing
velocity carried through `setOrigin`; how far it carries rides the
client's cmd times against the frame, and which spawn the clock-seeded
pick took (`cod11-map-cycle.md` 4.8). Ours' 1.5 and UO pmove match it to
the float when the replay sends the probe's cmd times (even 16 ms cmds run
48 ms into the first frame and slide 0.25 and 0.35 units off).

**As implemented.** `combat::muzzle_point_in` takes the 1.5 path for
`MoveRules::V1_5` and `Uo` (`aim::add_muzzle_bob` for the bob at the game
module's float precision; the bipod trace against the world only, the
entities it would meet not modelled). `scriptmodel_ab`'s
`a_frag_leaves_15s_untruncated_muzzle` and
`a_frag_leaves_uos_untruncated_muzzle` replay the two captures with the
thrower pinned to the spawn point under retail's first `PROBE thrower`
line (dropped as `placeSpawnpoint` drops it) and sending the net probe's
cmds (`ProbeHold::framed`): every frame of his first second sits on
retail's float, and both frags leave from retail's point bit for bit.
The prone-back gates pin the thrower the same way, to the spawn their
capture's `PROBE shooter` line implies.

**Measured moving, leaning, seated and on the bipod.** Four
`--save-slope` captures on retail with the weapon's spread and gun kick
zeroed in the server's homepath (`common::STEADY_WEAPON`), so each round
leaves along the view and its impact on a wall 1090 units out reads the
muzzle across the view (impacts are whole units, truncated toward zero):

- `mp_carentan-dm-slope-8ms-moveshot-walk-1.5.txt` (1.5b) and `-uo.txt`
  (UO 1.51b), `client-probes/probe_moveshot` `--probe-moveshot walk`, a
  thompson: 58 rounds each, strafing both ways at 178, running forward
  and back, crouched and sighted strafes, leaning still both ways
  (`leanf` ±0.5) and leaning while strafing (a lean slows the strafe to
  71). VERIFIED (captures). Replayed cmd for cmd on ours, every round
  lands on retail's unit or the next (`moveshot_ab`). With the bob taken
  out of `muzzle_point_in` 21 of UO's rounds and 15 of 1.5's miss by 1.4
  to 2.2 units; with the lean taken out all 18 leaning rounds on each miss
  by 8 to 15. VERIFIED (gate runs on those reverts). So the rule above holds on the move:
  the bob's side term along the right of the view, its vertical term,
  and `G_AddLean` on top. INFERRED, from the agreement.
- The UO capture found one difference, not in the muzzle: a leaning body's
  `bobCycle` ticked 6 a snapshot on retail and 14 on ours. UO's
  `PM_Footsteps` (`game.mp.uo` nm 0x297e4) takes the walk's cadence from
  `pm_flags` 0x80 only, where 1.5's (0x24ae8) also walks on a non-zero
  `leanf` (0x24bf7) (VERIFIED, both decompiles; `coduo-movement.md` 16).
  The 1.5 capture's leaning rounds and cycles match 1.5's rule. VERIFIED.
- `mp_carentan-dm-slope-8ms-moveshot-bipod-uo.txt` (UO), `--probe-moveshot
  bipod`, an mg30cal deployed prone 20 units short of the wall (face x
  1991.875, origin x 1971, `proneDirection` 0): the rounds along the bipod
  strike (1991, 1930, -12), those with the view at +30 and -30 degrees
  (1991, 1934, -12) and (1991, 1925, -12). VERIFIED (capture). The box
  trace stops 12.875 forward (x 1983.875), and from there the 30-degree
  lines reach the wall at y 1930 ± 4.62; from the untraced eye they would
  reach it at ± 12.05 (1942 and 1917, what ours lands with the trace
  removed). INFERRED, from the geometry; the trace is the one above. The
  trigger held prone with the bipod up fires nothing on retail or ours.
  VERIFIED (capture, gate).
- `mp_peaks-tdm-slope-8ms-jeep-rider-uo.txt` (UO), `probe_vehicle`
  `--probe-vehicle seats` with a steadied kar98k: the jeep passenger's
  `seat_fire` round (view pitch held at the seat arc's 48.8 degrees) strikes
  (-3837, 925, -15), 28 over his origin (-3883.89, 920.46, 10.06), on
  retail and on ours (`vehicle_ab`'s
  `the_jeep_passenger_s_round_lands_as_on_retail`). From his eye (60) ours
  lands at (-3815, 928, -9). VERIFIED (capture, gate).

**The street placement.** After the gametype's `setOrigin` (900, 1930,
-38) retail's body rests at (900, 1930, -42.944004) with no velocity, 0.18
over the grade, which the slope its later strafes cross puts at about
-43.122 under that spot: the ground trace caught it inside its 0.25 reach
before the fall's step met the face, so nothing slid. VERIFIED (UO
capture, first `!snap`); the ground height INFERRED from the strafe rows.
Where a fall ends that way rides the cmd times against the frame, and
the captures log no cmd before the script's first. VERIFIED, `moveshot_ab`:
with ours' wait sending one cmd a frame, the body lands on the face and slides to (900, 1929.7953, -43.13387), which
put one UO round a unit off on two axes; sending the net probe's
`--probe-cmd-ms 8` cadence instead (the probe's loop sends a cmd every
iteration from its first snapshot), ours rests between (900, 1929.945,
-43.126) and (900, 1930, -42.948) over eight cmd phases one millisecond
apart, and every round of the 1.5 and UO captures lands within the
gate's unit on each of them. So the gate keeps no gap row; the rest
height still differs from retail's by up to 0.18 until a capture logs the
wait's cmds. On flat ground retail's body sits at z -23.874996 after a
placement 0.125 over the floor (-23.875; the bipod capture). VERIFIED.
Ours stayed at -23.75 with one cmd a frame: `setOrigin`'s z lift puts the
body 1.125 over the floor, and one 50 ms step of the fall ends at
-23.75, inside the ground trace's reach, where nothing moves it again.
With the probe's 8 ms cmds ours rests on retail's -23.874996 to the bit.
VERIFIED (`moveshot_ab`, both cadences); the mechanism
INFERRED from the arithmetic. The UO throw's two float steps above (36.125 plus
two ulps) are that z: -23.874996 + 60. INFERRED.

VERIFIED, `ClientEndFrame` `0x410f1`: `client+0x2240` is
`ps->aimSpreadScale / 255.0`. INFERRED: it is computed once a frame, since
that is when `ClientEndFrame` runs. VERIFIED, `FireWeapon`
`0x68eb9`: it compares `ps->fWeaponPosFrac` against 1.0 and has two arms, one
computing `adsSpread + (hipSpreadMax - adsSpread) * client->0x2240` and the
other `min + (hipSpreadMax - min) * client->0x2240` where `min` is
`BG_GetMinSpreadForWeapon(ps, weapon, level.time)`. INFERRED: the `adsSpread`
arm is the one an equal comparison takes.

VERIFIED, UO `ClientEndFrame` (0x432b6): `client+0x4698` is
`ps->aimSpreadScale` (`ps+0x628`) over a double 255.0, stored as a float,
ahead of its `P_DamageFeedback` call, and UO's `FireWeapon` (0x7bd05)
reads it as its blend. INFERRED: on all three servers a shot reads
the scale the last end frame left, so a hip shot's own `hipSpreadFireAdd`
(1.5, step 8) widens only the next shot and a hit's feedback add the
frame after it. VERIFIED, `snapshots-uo-bazooka-draw.bin` with gdb on the
same run: the shooter's `aimSpreadScale` reads 0 on every
snapshot before the shot and 91.8 on the shot's own, and the rocket left
at a spread of 7.0, `bazooka_mp`'s `hipSpreadStandMin` (its
`hipSpreadMax` 12, `hipSpreadFireAdd` 0.4): the scale the shot read was
0 (the post-add scale would give 9.0). vcod keeps the end frame's in
`ClientSim::spread_frac`.

`BG_GetMinSpreadForWeapon` `0x37114`, read with `tools/re/annotate_func.py`
(field names as in `cod11-mantle.md`, "The eye through a stance change"). VERIFIED: it compares `viewHeightCurrent` (ps+0xD0, float)
against `viewHeightLerpTarget` (ps+0xD8, loaded as an int) at `0x37140` and
`viewHeightLerpTime` (ps+0xD4) against 0 at `0x3714c`, and its first path
tests `pm_flags & 1` and `& 2` and loads `hipSpreadProneMin` (weapon def
`+0x244`), `hipSpreadDuckedMin` (`+0x240`) or `hipSpreadStandMin` (`+0x23C`).
INFERRED, from the branches: that path is taken when the eye equals the
leg's target or no leg runs, prone first, then the crouch latch, then
standing.

VERIFIED, the second path: it calls `PM_GetViewHeightLerpTime(ps,
viewHeightLerpTarget, viewHeightLerpDown)` at `0x3718d`, subtracts
`viewHeightLerpTime` from its own third argument, divides by the call's
answer as integers loaded to x87 (`0x37192`-`0x371a4`) and clamps the
quotient against `fldz` and `fld1` (`0x371a6`-`0x371c5`); then it compares
`viewHeightLerpTarget` with the prone height (ps+0x33C, `0x371cd`) and the
standing height (ps+0x344, `0x371e3`) and tests `viewHeightLerpDown`
(`0x37200`), loads two of the three minimums per arm and ends in `fsub`,
`fmulp`, `faddp` (`0x3721d`-`0x37221`). INFERRED, from the loads' order: the
result is `from + (to - from) * f` with `f` the leg's elapsed time over its
length, clamped to 0..1, and

- a leg to the prone height blends ducked to prone;
- a leg to the standing height blends ducked to standing;
- any other leg, the ones to the crouch height, blends standing to ducked
  going down and prone to ducked going up.

So every leg blends, the standing-crouch ones included, and the blend is
linear in time, not along the eye's curve: halfway through the 400 ms leg
to prone the minimum is halfway between ducked and prone while the eye is
at 15 of the 40-to-11 drop. `PM_GetViewHeightLerpTime` (`0x345b8`) is the
leg-length pick that section of the mantle doc gives.

VERIFIED: `FireWeapon` passes `level.time` (`level+0x1E8`, `0x68ee4`) as the
third argument, and the `.so` has no other caller. VERIFIED: the cgame's
copy is `0x3000fa50`, the same three comparisons, the same weapon-def
offsets and the same four arms, reading the playerstate at `0x3020715c` and
called only from the crosshair at `0x30016be6` with the time off
`[0x301e2160] + 8`. INFERRED: that is `cg.snap->serverTime`, `0x301e2160`
being `cg.snap` (`cod11-sound-system.md` section 3), so the crosshair reads
the leg against the snapshot the client interpolates from.

**As implemented.** `pmove::weapon::hip_spread_min` ports the function over
a `SpreadStance` (the three flags, the eye and the leg, timed against the
caller's clock), reusing the leg-length pick `pmove::view_height_adjust`
runs on. The server takes the stance at the cmd that raised the shot,
against the tick's clock, and `spread_deg` reads it; the crosshair reads
the predicted playerstate, or the snapshot's fields when not predicting,
against the serverTime of the snapshot the render clock interpolates from.
No committed capture separates it: none records a shot's impact beside a
stance change.

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
  `pml.frametime` (`0x3864e`). VERIFIED, UO 0x35e56: the same test on UO's
  `eFlags` (ps+0x84), whose stance bits follow the eye on 1.5 and UO
  (cod15-movement.md 3). VERIFIED, live (`pronemove-lmg-uo.txt`, ct 24590
  to 24990): a body standing up from prone reads `eFlags` 0x20 while its
  eye rises and decays at the crouch rate; vcod read the stance flags and
  decayed three times as fast until `PlayerState::eflags_stance`.
- The additions are skipped whole when `ps->fWeaponPosFrac == 1.0`
  (`0x38657`), an exact float compare, and in cgame an exact integer one
  against `0x3F800000`. The decay is not skipped, so a settled sight decays
  to 0 whatever the player does.
- With `hipSpreadTurnAdd` (`0x254`) non-zero (`0x38676`), axes 0 and 1 each
  add `fabs(AngleSubtract(SHORT2ANGLE(cmd.angles[i]),
  SHORT2ANGLE(oldcmd.angles[i]))) * 0.01 * hipSpreadTurnAdd / pml.frametime`
  (`0x3868e`-`0x386ed`). The input is a usercmd angle delta across one pmove
  step, not `ps->viewangles` and not a delta across a server frame.
- `oldcmd` is `pers.oldcmd`, and `ClientThink` rotates it before
  `ClientThink_real` looks at the cmd: game.mp.uo `ClientThink` (0x51d13)
  copies `pers.cmd` (`client+0x4524`, six words) over `pers.oldcmd`
  (`+0x453c`) and fetches the new cmd into `pers.cmd`, then calls
  `ClientThink_real` (0x50c91), which clamps the cmd's clock and returns
  when `serverTime - commandTime` is not positive; its `Pmove` reads
  `pers.oldcmd` (`local_10[0x114f..0x1154]`) (VERIFIED decompile). So a
  cmd the engine hands over (past `cl->lastUsercmd.serverTime`, 3.6)
  whose clock is not past `commandTime` moves nothing yet becomes the
  next cmd's `oldcmd`. The case that shows it: a spawn's think sets
  `commandTime` to the frame's time, and the client's first cmd after it,
  stamped at that time and turned onto the new `delta_angles`, is
  dropped but still bases the turn term, so UO's turn add stays shut
  (VERIFIED, `vehicle_ab`'s `the_jeep_driver_killed_in_his_seat_replays_on_ours`
  with the replay sending that cmd after the spawn: with the dropped cmd
  left out of `oldcmd`, ours read `aimSpreadScale` 226 where retail's read
  0). vcod: `ClientSim::stale_cmd`, off `Client::last_usercmd_st`.
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

The `--save-ads` captures (1.13) put numbers on the rest. VERIFIED: a hip shot from the
carbine reads 162.18 on the first snapshot after it, 16 ms into a decay from
`hipSpreadFireAdd` 0.7 times 255, and then 78.54, 60.18, 0, at the decay of
4 times 255 a second; the mosin's reads 241.74,
one 16 ms cmd of decay off 255 (255 on an older take), and comes
down 41.44 a full step. A walk climbs 51 a full step on the carbine
(the add of 8 less the decay of 4, times 255, times the frame) to 255 in
278 ms, and stopping brings it down 51 a step to 0 in 244. A shot down the
settled sight adds nothing, and a walk begun with the sight coming up adds
until the snapshot the fraction reads 1.0 and decays from the next step on:
`ads_walk` on carentan reaches 255 at 279 ms with the fraction at 0.94,
still reads 255 at 312 on the snapshot whose last cmd brought it to 1.0,
and reads 187.68 at 362, 66 ms of decay. INFERRED, that the
skip keys on the previous step's fraction, as 2.1's call order says: the
step that reaches 1.0 still adds, and the one after it does not. The BAR
is the one stock weapon with a turn term (`hipSpreadTurnAdd` 0.8) and it
is not captured.

`PM_Weapon` writes the counter too. VERIFIED, `.so` 0x39228-0x39254: when
the rechamber check (0x375d0) returns 0, a body with `pm_flags` 0x1 and a
non-zero `cmd.forwardmove`/`rightmove` word, or a `weaponstate` of 10 or 11
(a swing), gets `aimSpreadScale = 255.0` (rodata 0x72554); 1.5 has the same
store at `PM_Weapon` 0x30643 (immediate 0x437f0000). That store is where the
swing's 255 of 1.10's `melee_tap` comes from, and it holds a crawling prone
body's cone open where the move term alone would leave it a frame short.
VERIFIED, vcod measurement (`moves_ab.rs`): the one
`aimSpreadScale` miss of each moves capture, the prone press while running,
closes with it (`pmove::weapon::open_cone_on_prone_move`).

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

VERIFIED, United Offensive's `BG_Bullet_Endpos` (`game.mp.uo.i386.so`
0x3011c): the same cone, 10000 units out (0x461c4000), with the forward
product on the x87 stack. 1.5b's `Bullet_Endpos` (0x6de56) keeps 8192.
Every UO `Bullet_Fire` and the artillery strike go through it
(cod11-turrets.md 13.8). vcod: `combat::bullet_range`.

**The rounding.** VERIFIED, 1.1d `Bullet_Endpos` (0x69624): `fptan` of
`spread * pi / 180` (.rodata 0x79cf8, 0x79d00) is stored as a double and
then a float (0x6964d, 0x69653); `k = tan * 8192` and the two offsets
`x * k`, `y * k` stay on the x87 stack (the `fst` at 0x69683 and 0x69688
store a copy and keep the unrounded value), and each component of
`forward * 8192 + muzzle + right * x' + up * y'` is rounded once, at the
last `fstp` (0x696d4..0x696e5). VERIFIED, 1.1d `Bullet_Fire` (0x690dc)
does not call `Bullet_Endpos`; it runs the same chain inline: `fptan`
(0x69105), the double and the float (0x69109, 0x6910f), the offsets
kept past their `fst` copies (0x6913f, 0x69144), and each component of
`forward * 8192 + muzzle + right * x' + up * y'` rounded once (0x69186,
0x69190, 0x691a2) before `Bullet_Fire_Extended` (0x691b4). VERIFIED,
1.1d `FireWeapon` inlines it once more for a player's round: the spread
blend `min + (max - min) * scale` stays on the x87 stack into
`spread * pi / 180` (0x68ecb..0x68f26), then `fptan` (0x68f3a), the
double and the float (0x68f3e, 0x68f44), and each component rounded once
(0x68fe5..0x68fed). VERIFIED, gdb on the retail 1.1d server
(`--save-combat` with its kar98k on mp_carentan; dprintfs at 0x68f1e,
0x68f6a and 0x68ff0): three standing rounds at spread 2.0, a crouched one
at 1.6 and a prone one at 1.324, each at scale 0, read the tangent's
float, `gunrandom`'s pair, the frame and the end point, which
`combat::an_11_rounds_spread_lands_on_retails_bits` holds to the bit. A
round at a scale above 0, where vcod's f32 blend could round the spread
apart from the stack's, was not read.
VERIFIED, 1.5b `Bullet_Endpos` (0x6de44)
and UO `BG_Bullet_Endpos` (0x3011c): `tan` (libm, double) of
`spread * pi / 180` (UO .rodata 0x9cd10, 0x9cd18) stored as a float
(0x30150), `k = tan * range` stored (0x30159), `x * k` and `y * k` stored
(0x30174, 0x3017d), and the end point stored after each of its three adds:
`forward * range + muzzle` (0x30193), then `+ right * x'` (0x301da), then
`+ up * y'` (0x30225). VERIFIED, gdb on retail UO 1.51b
(`probe_uofx` with its kar98k, three hip rounds at spread 2.0): `k`
0x43ae9a96 and the three end points, which
`combat::a_uo_rounds_spread_lands_on_retails_bits` holds to the bit.
VERIFIED, UO `AngleVectors` (0x3b068): each angle times the double
pi/180 (.rodata 0x9dad0) stored as float radians, `fsincos` (0x3f464)
storing the sine and the cosine, every component stored once; the right
vector's -1.0 is .rodata 0x9dad8. INFERRED: with roll 0 (`CalcMuzzlePoints`
reads `ps+0xf0`, the view's roll) every component is one product of two
stored floats, so 1.1d, 1.5b and UO give the same frame; `right.z` is
`-sin(0) * cos(pitch)`, a negative zero (VERIFIED in gdb:
0x80000000). vcod: `combat::aim_frame`, `combat::spread_point`.

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
   when `d < 0.125` (the `fcom` at `0x68b5b` and the `and ah,5` / `jne` after
   it, which jumps to a `fldz` on C0 or C2), and the function recurses at the
   same damage, with the same `passEnt` and `depth + 1`. The 0.125 and 0.25
   are `.rodata 0x79c60` and `0x79c64`. VERIFIED: the ray is `end - start`
   of this call normalized (`0x68b12`..`0x68b35`), the nudged point is
   stored through the `start` pointer itself (`[edi]`, loaded from the
   argument at `0x68899`, stores at `0x68b7c`..`0x68b94`), and the recursion
   pushes the incoming `passEnt` and `attacker` (`0x68baa`, `0x68bae`).
   INFERRED, off the `je` at `0x68b0c` and the `jmp` to the epilogue at
   `0x68bb7`: a glass hit never reaches `G_Damage`, its own impact is step
   2's, and a pistol round passes glass as a rifle round does, since the
   test is ahead of any read of `dflags`.
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

VERIFIED, the step 5 call's pushes (`0x68c14`..`0x68c2e`): the shooter and
`params` arguments passed on unchanged, `depth + 1`, the halved damage, the
caller's own `end`, `&trace.endpos` as `start`, the caller's `attacker`, and
the hit entity as `passEnt`. VERIFIED: the halving is `(damage + (damage >>>
31)) >> 1` on the incoming `damage` argument (`0x68c02`..`0x68c0d`), C's
truncation toward zero, and not on the location-multiplied value `G_Damage`
computes. VERIFIED: the two tests step 5 rests on, the hit entity's client
pointer (`gentity+0x158`, `0x68bf3`) and `dflags` (`0x68bfc`), sit after the
`G_Damage` call at `0x68beb`. INFERRED, off the `je` at `0x68bc7` to the
epilogue: step 5 is reached only on the `takedamage` arm, and the first
player's damage callback runs before the round's next leg is traced.
VERIFIED: both callers start `depth` at 0 (`push 0x0` at `0x68ff2` in
`FireWeapon` and at `0x691a7` in `Bullet_Fire`). INFERRED: so a round has at
most 13 legs; each keeps the shot's `end` and so runs on along the same line;
each passes only the player the leg before it hit, so a later leg cannot meet
that player again; and each is a whole call, so a world surface a later leg
reaches raises its own 173/174 by step 2 and a second player takes `damage /
2` through its own location multiplier by step 4, for as long as the halving
leaves a positive damage.

Measured on retail: `client-probes/probe_passthru` (its section
in that directory's README) on mp_carentan stood two `--probe-target` clients
100 units apart on one line and a `--save-hit --probe-sweep` shooter with the
carbine (`damage` 45) 200 units in front of the first, and logged every damage
callback with the frame's time ahead of `dm.gsc`'s own. VERIFIED, that log:
at server time 33600 the front player took 40 at `torso_upper` and the back
one 33 at `head`, both with `iDFlags` 32; the same back player took 67 at
`head` from a direct round at 34700. INFERRED: 40 is `(int)(45 * 0.9)` and 33
is `(int)(22 * 1.5)`, the 45 halved and then put through the back player's own
multiplier. VERIFIED, the shooter's drained events for that frame: a 174 with
`surfType` 7 at (1122 -376 -90) and another at (1229 -377 -85), and a 174 with
`surfType` 10 at (3941 -406 24), on the world behind both. VERIFIED, the
one-target run of `cod11-events-and-fx.md` section 2: every hit put
a world 174 behind the target in the hit's own snapshot beside the flesh copy,
the first at (1925 -413 -119) behind a flesh hit at (1122 -376 -91). The turret
capture shows the same behind its target (`cod11-turrets.md` 12.5).

**As implemented.** `fire_round` (`crates/server/src/game/combat.rs`) runs
the recursion as a loop of at most 13 legs: a player hit becomes a hit at the
leg's damage through its multiplier, and a `rifleBullet` round goes on from
the hit point along the same line with that player passed and the damage
halved, and any round goes on past glass from the nudged point, until a leg
meets any other surface (its impact), nothing, or a halving to 0.
VERIFIED, the same probe run against ours (`vcod-server mp_carentan
--gametype-script .../probe_passthru.gsc --set probe_teleport=1`): at 39550
the front player took 67 at `head` and the back one 33 at `head` in one frame,
with a world 174 behind them; the turret gate's two world impacts behind its
target read retail's (1248 1308) and (1248 1312) to the unit
(`cod11-turrets.md` 13.1). VERIFIED, retail's 33600 frame: the flesh 174s
took entities 176 and 178 and the wall 174 took 180, so a leg's callback
(and the flesh copy `finishPlayerDamage` raises) runs before the next leg is
traced.

`fire_round` traces every leg first but returns what the round did as one
ordered list of impacts and hits, and the server applies it in that order: an impact goes on the wire, a hit runs `CodeCallback_PlayerDamage` on
the spot, so the flesh pair `finishPlayerDamage` raises numbers between the
leg's impacts the way retail's does. The turret pass applies its rounds the
same way. INFERRED: tracing ahead changes nothing else within a round. The
next leg passes the player the callback ran on, a corpse that callback
clones is `CONTENTS_CORPSE`, which the shot mask 0x2802031 leaves out, and
no stock damage or kill callback moves another player. The list is applied
inside the cmd that fired the round, before the next cmd of any client runs,
which is where retail's `FireWeapon` runs (16): a player one round kills is
out of the way of every later round of the frame.

Glass, measured on retail: `client-probes/probe_glass` (its
section in that directory's README) on mp_depot stood a `--probe-target`
client 76 units behind the pane brush at x -772..-764 (lump 4 brush 434,
material `glass@window2a` 0x8000010) and a `--save-hit --probe-sweep`
shooter 16 units in front of it, with the enfield (`damage` 120,
`rifleBullet` 0). VERIFIED, the server's log: the target took 180 at `head`
three times, 107 at `torso_upper`, 72 at `left_arm_upper`, 60 at
`left_arm_lower` and 48 at `right_foot`, `iDFlags` 0, each 120 through the location's own
multiplier and none halved. VERIFIED, the shooter's drained events: each of
those taps put a 173 on the pane's +x face at x -763 in the same snapshot as
the flesh 173 behind it at x -840, the pane's with the lower entity number
(169 and 179 at +65984 ms). VERIFIED, lump 4: that face's side is
`textures/common/caulk`, surface flags 0x400a0, and every pane impact read
`surfType` 0. The impact takes the side's surface flags, not the brush
material's 0x900000: the brush clip's epilogue (`cod_lnxded` 0x8055565)
stores the lead side's material's flags into the trace (VERIFIED
decompile; `bsp-ibsp59-format.md`, lump 3). VERIFIED: the pane's other face, x -772, is side material
`glass_nosight@fwindow5` 0x8000010, and the render soup drawn there is a
different lump-0 entry of that name, contents 0x1; the round went on
through both.

Glass does not break in MP. VERIFIED: `game.mp.i386.so` carries no
`func_glass` string, and its only strings naming glass are the six
`EV_*_GLASS` movement events. VERIFIED: the one stock script that breaks
windows is single-player `maps/_window.gsc` (`pak4.pk3`), which waits on
`targetname "window"` damage triggers, and no stock MP map's entity lump
carries a `window` targetname or a classname naming `glass` or `damage`.
INFERRED: a pane passes rounds and stays.

vcod follows step 3: `CollisionWorld::hit_contents` reads the contents word
off the prim a trace stopped on, glass brushes (contents 0x10 without SOLID)
enter the clip, and `fire_round` goes on past one from the nudged point.
VERIFIED, the same probe against ours: the target took 180 at `head` and 107
at `torso_upper` through the pane, the pane's 173 numbered below the flesh
one. Traces report the struck side's surface flags (a brush-material read
gives the pane `surfType` 9 where retail's caulk face reads 0; not rerun
against this probe since). One difference remains: a round meeting glass at
under 0.125 of its normal is not nudged, so each later leg starts where the
last stopped and meets the pane again at fraction 0 until the depth runs out:
13 impacts on the pane and nothing behind it. INFERRED from step 3 and Q3's
`CM_TraceThroughBrush`, whose `SURFACE_CLIP_EPSILON` leaves `endpos` 0.125 in
front of the plane; no capture has a grazing shot.

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

VERIFIED, `game.mp.i386.so`: after `G_CheckHitTriggerDamage` the function
tests `trace.surfaceFlags & 0x10` (0x687a9) and the fraction against 1.0
(0x687b3..0x687c2), both jumping to the return at 0x68886 past the temp
entity and the damage; UO's `Weapon_Melee` (0x7a524) takes the same two
tests (0x7a609, 0x7a620) and the `takedamage` (+0x17d) test at 0x7a6c7.
INFERRED: a swing at nothing or at the sky spawns no event. VERIFIED, live
(`client-probes/probe_skyshot` air part,
`fixtures/missiles/mp_carentan-skyshot-air.txt` on 1.1d and `-air-uo.txt`
on UO 1.51b): three swings into open air (`weaponstate` 10 then 11) put no
temp entity on the wire; the same swings 80 degrees down at the floor
(`-floor.txt`, 1.1d) each raise 167 with parm 5 naming the world. VERIFIED,
live (UO 1.51b, `client-probes/probe_takedamage`,
`mp_carentan-takedamage-melee-uo.txt`): a swing at a `setTakeDamage(1)`
script model raises 167 naming the model and takes it 71, 73 and 73 as
`MOD_MELEE` with the M1 carbine. INFERRED, off the mask (0x2802031, the
bullet's): a swing strikes a shootable grenade's box as a round does. vcod
ports all four (`a_swing_damages_what_takes_damage`,
`crates/server/tests/missile_edge_ab.rs`). One gap stands: the floor swing's truncated
event origin reads x 1375 on ours and 1374 on retail, y 2615 on both, from
(1368, 2608.1) facing yaw 45; the start or the aim is a fraction of a unit
off ours in a way that capture cannot pin (`FLOOR_POS_GAP`).

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

### 2.7 What a `bulletTrace` clips

VERIFIED, `GScr_BulletTrace` (`game.mp.i386.so` `0x5abc4`): it stores the
mask 0x2802031 (`0x5abd2`), calls `Scr_GetBool(2)`, `hitCharacters`
(`0x5abfd`), and has a second store of 0x802031 to the same slot (`0x5ac09`).
INFERRED, off the `jne` at `0x5ac07` that jumps over that store: the narrower
mask is the one taken when `hitCharacters` reads false.
VERIFIED: it loads 0x3ff as the pass entity (`0x5abcd`), checks argument 3's
`Scr_GetType` against 7 and `Scr_GetPointerType` against 0xd
(`0x5ac1d..0x5ac32`), and replaces the pass entity with `Scr_GetEntity(3)`'s
number (`0x5ac3e`). INFERRED, off the two `jne`s past that load: the pass
entity stays 0x3ff unless argument 3 is an entity.
VERIFIED: it calls `trap_LocationalTrace` with `bulletPriorityMap`
(`0x5ac56`).
VERIFIED: the result's `entity` is written by a `Scr_AddUndefined` call
(`0x5acb4`) and a `Scr_AddEntity(&g_entities[n])` call (`0x5acd3`), and the
compare between them is the hit number less 0x3fe against 1 as an unsigned
16-bit word (`0x5aca6..0x5acb2`). INFERRED, off the `ja` at `0x5acb2`: the
undefined is written when that difference is at most 1, the entity otherwise,
so `entity` is undefined for the world and for a miss and names any entity
the trace stopped on.

The result is an array with five string keys. VERIFIED, `GScr_LoadConsts`
(`0x58550`): the `scr_const` slots the builtin names are allocated from
these `.rodata` strings: 0x2c `entity` (`0x75f28`, stored at `0x58778`), 0x32
`fraction` (`0x75ff5`, `0x587c0`), 0x5c `normal` (`0x760c6`, `0x589b8`), 0x64
`position` (`0x760ea`, `0x58a18`), 0x80 `surfacetype` (`0x76174`,
`0x58b68`) and 0xf8 `none` (`0x764c8`, `0x59108`).
VERIFIED: `fraction` is the trace's first float, `[ebp-0x48]`
(`0x5ac63..0x5ac7f`), and `position` the vector four bytes into the same
result, `[ebp-0x44]` (`0x5ac8a..0x5ac9e`); `entity` follows as above.
INFERRED: `position` is the trace's end position, so a miss reads the `end`
argument.
VERIFIED: `fld1` / `fcomp` against the fraction at `0x5acf0` and a `jne` on
the C0/C2/C3 mask at `0x5acf8` whose target is `0x5ad50`. INFERRED, off that
branch: a `fraction` below 1 falls through to `0x5acfa` (the hit) and 1 takes
the jump (the miss).
VERIFIED: the block at `0x5acfd..0x5ad44` writes `normal` from the vector at
`[ebp-0x38]` and `surfacetype` from `trap_SurfaceTypeToName` of bits 20..24
of the dword at `[ebp-0x2c]`. INFERRED: those are the hit plane's normal and
the surface flags' material field.
VERIFIED: the block at `0x5ad50..0x5adb3` writes `normal` as
`VectorNormalize` of the second argument less the first, and `surfacetype`
as `Scr_AddConstString(scr_const 0xf8)`, `"none"`.
VERIFIED, the stock `maps/` scripts in `pak0..pak9`: all three `["normal"]`
reads are in `maps/mp/_utility.gsc` `getPlant`, each handed to
`orientToNormal`.

VERIFIED, live (retail 1.1d, mp_carentan, `client-probes/probe_surfside`): five hits on the tops of rock, caulk and
wood brushes all read `surfacetype` `"default"`. On a
dedicated server `trap_SurfaceTypeToName` lands in the null renderer's
stub, which returns `"default"` whatever it is asked (`cod_lnxded`
0x806dbc8, VERIFIED decompile; INFERRED that the trap reaches it, off the
capture).

vcod: the builtin writes the five keys, with the miss arm's `normal` and
`"none"` as above and a hit's `surfacetype` `"default"`. With
`hitCharacters` a living body whose bones the segment scores stops it
(section 3) and names its entity; on United Offensive a vehicle's posed
hull stops it too (`coduo-vehicles.md` 13).

VERIFIED: `SP_script_model` (`0x60ff4`) writes `r.contents` (`ent+0x118`)
0x2080 (`0x61010`), ORs 4 into `r.svFlags` (`ent+0xf4`, `0x61020`) and calls
`trap_LinkEntity` (`0x61028`). 0x2080 meets both masks above in bit 0x2000.

VERIFIED, `cod_lnxded`, the per-entity clip `0x809105c` (section 3.1): it
tests `ent+0x118` against the clip's mask, tests the locational-trace flag
and `svFlags & 4`, and calls the model record lookup (`0x806e498`), a test
of the model's collision contents against the mask (`0x80c53d0`), a bounds
reject on the model's bounds offset by the origin (`0x80c4f6c`,
`0x805a788`), the axis build from `ent+0x140` (`0x806709c`), a transform of
`start` and `end` into the frame of that axis and the origin at `ent+0x134`
(`0x8066520`), and `0x80c52c0`, which walks the model's collision surfaces
through `0x80c203c`, the same clip the static models take
(`docs/research/cod11-mantle.md`, "Static models are clipped as a
segment"). INFERRED, off the branches: the function returns early when the
contents and the mask share no bit, the model path runs only on a locational
trace and only with `svFlags & 4`, and the calls run in the order listed.
INFERRED, off the same: a linked `script_model` with collision stops a
`bulletTrace` on its own mesh, at its origin and angles as they are when the
trace runs, and a closer world hit keeps the world's.

VERIFIED: `ScriptEntCmd_NotSolid` (`0x612cc`) and `ScriptEntCmd_Solid`
(`0x61204`) both reference the string `"cannot use the solid/notsolid
commands on a script_model entity"` (`0x78d00`), and their only writes of
`ent+0x118` are at `0x61378` and `0x612b8`. INFERRED, off their classname
branches: the print is the `script_model` arm and the write sits on the
`script_brushmodel` arm only. VERIFIED: `ScrCmd_Hide` (`0x5dcac`) writes
nothing but an OR of 0x10 into `ent+0x17d` (`0x5dcdf`). VERIFIED, live
(retail 1.1d mp_carentan and UO 1.51b mp_peaks,
`client-probes/probe_scriptmodel`, fixtures in
`crates/server/tests/fixtures/scriptmodel/`): a spawned
`xmodel/barrel_black1` stops a `bulletTrace` at the same fraction (0.190383,
`surfacetype` `"default"`, the entity named) shown, hidden and
`notSolid()`ed; once `delete`d the trace runs on to the world.

VERIFIED, off `maps/mp/mp_carentan.bsp`: `bombzone_A` (`*4`, origin
`-146 2490 16`) stands over two script models at `-146 2490 -32`, angles
`0 290 0`: `xmodel/turret_flak88_static_antiairlow`, and
`xmodel/turret_flak88_static_antiairlow_d` with `targetname` `exploder`, which
`_load.gsc` hides. VERIFIED, off the paks: both carry collision surfaces of
contents 1. INFERRED, off the plant test in
`crates/server/src/game/script.rs` reproducing the fixture's slot 0: the
flat plate of that mesh at z -23 is what `getPlant`'s `(+16, +16)` fallback
trace lands on, where the floor under the planter is clip with no shot
contents and the two 18-unit traces meet nothing (object-model doc 23.6).

The weapon traces and the missile take the same arm. VERIFIED:
`Bullet_Fire_Extended` and `Weapon_Melee` trace under 0x2802031 (2.3, 2.5),
and `G_RunMissile`'s move trace is `trap_LocationalTrace` under
`ent->clipmask` (`0x540c6`, 12.1), which `fire_grenade` sets to 0x2802091
(11.1); each shares bit 0x2000 with 0x2080. INFERRED: a round, a swing
and a grenade in flight all stop on a script model's mesh.

VERIFIED: 1.1's script entity field table (`0x78e68`, 34 records, read with
`tools/re/dump_script_fields.py`) has no `takedamage`; `SP_script_model`
(`0x60ff4..0x61033`) writes no `takedamage` byte (`+0x171`); and
`Scr_SetHealth` (`0x5c68c`) writes only `+0x230` and `+0x234`. INFERRED: a
1.1 script model never has `takedamage`, so a round on one raises its impact
off the struck surface (2.4, step 2), takes no `G_Damage` (step 4) and ends,
since the trace's contents are the entity's 0x2080 and carry no glass bit
(step 3). A grenade bounces off it as off a wall (12.4). VERIFIED, live
(UO 1.51b, mp_peaks, `probe_scriptmodel`'s rounds part): a `--probe-uo-weapon
aim` Thompson held on an empty Willys 250 units away through a barrel three
quarters of the way there lands no round on the jeep while the barrel is
shown and while it is hidden, and 22 a round (`MOD_PISTOL_BULLET`) from the
first frame after its `delete`.

VERIFIED, United Offensive: `SP_script_model` (game.mp.uo `0x705b6`) ORs 4
into `svFlags` (`+0xf8`, `0x705ea`) and writes 0x2080 into `contents`
(`+0x120`, `0x705f6`), so the same holds there, except that UO's
`setTakeDamage` gives a script model `takedamage` (`coduo-diff.md` 22).

vcod: the `bulletTrace` builtin, every bullet, swing, turret round, UO
artillery aim and missile step clip every live `script_model` whose xmodel
has collision, loaded once per model name, with the segment moved into the
entity's frame (`GameHost::shot_models`), beside a United Offensive
vehicle's hull, the same xmodel arm walking each collision surface in its
posed bone's frame (`coduo-vehicles.md` 13). `CanDamage` clips the same
set (14.4). A round on a script model draws its impact and ends; it runs
`G_Damage` only on a UO model `setTakeDamage` armed. A swing's miss names
the model as `otherEntityNum`. A deleted model leaves the set at once,
with its unlink (`game.mp.i386.so` 0x5da81; the slot's reuse waits), not
when the slot frees. `scriptmodel_ab` replays both
probe parts (the trace on 1.1d and UO, the rounds on UO).

**The whole-model bounds reject.** VERIFIED, decompile: the bounds the
entity clip tests come from the DObj's first model (DObj `+0x1c`), whose
asset record's `+4` points at the loaded model; its `+0x4c..+0x60` are the
six floats the header parser copies from the descriptor's bytes 2..26, the
`mins`/`maxs` after the version (1.1d getter `0x80c4f6c` over `0x80c28e0`,
parser `0x80c1164`, loader `0x80c13f8`; 1.5b
`0x80d17a2` over `0x80cec5c`; UO `0x80c6b36` over `0x80c3ff0`, parser
`0x80c3310`, loader `0x80c382c`). The test is the slab walk over `[0, fraction]` (1.1d
`0x805a788`, 1.5b `0x805e553`, UO `0x805e387`) on those bounds added to
`r.currentOrigin`, never turned, in the clip of all three (1.1d
`0x809105c`, 1.5b `0x8099732`, UO `0x809a72a`). VERIFIED, off the paks:
1002 of the 1012 1.1 descriptors store a cube centred on the origin, and
the other 10 store all zeros, two of those with collision (`cow_dead2`,
`vehicle_german_yacht_openingsequence_animrig`). 384 1.1 models carry
collision; it pokes out of the cube on seven (`vehicle_car` by 17 units
at the nose, `x` to 111.2 against 94.37;
`static_turret_flak88_static_antitank` to 240.2 against 222.2; the two
`vehicle_german_condor_lowpoly`s, the two T-34 wrecks, and `v2rocket` by
under a hundredth), and UO adds many single-player rigs whose bounds are
zero. INFERRED: on every other model the cube, whatever the
yaw, holds the whole collision mesh, so the reject only saves work there.

VERIFIED, live (retail 1.1d, 1.5b and UO 1.51b, mp_carentan,
`client-probes/probe_modelbounds`, fixtures
`mp_carentan-scriptmodel-bounds{,-1.5,-uo}.txt`, recipe in each header):
level `bulletTrace`s across a `vehicle_car` 200 above the first tdm spawn
stop on it at its nose's 94 and not at 95 or past, at yaw 0, 90 and 180;
at yaw 45 they stop on it out to 98 along its forward (69 on each axis,
inside the cube). No trace stops on `cow_dead2` at any yaw. The three
servers log the same rows; 1.5b and UO match each other to the digit and
1.1d one fraction in its last digit.

vcod: `PlacedModel::bounds` holds the descriptor's bounds
(`GameHost::model_bounds_of`, the cache the area tree links a script model
with) for a script model and a UO vehicle, and `PlacedModel::clip` runs
`collision::segment_misses_box` on them about the origin before any
triangle. `model_bounds_ab` replays the probe on 1.1d, 1.5b and UO, every
row to 1e-5.

The client has no counterpart. VERIFIED, `cgame_mp_x86.dll` 1.1:
`CG_ClipMoveToEntities` (0x30028df0) has two callers, the point trace
(0x30029040, syscall 0x26) and the capsule trace (0x300290b0, syscall 0x28),
and clips each entity of the solid list as an inline model or the box its
`solid` packs (`cod11-player-clip.md` 5). VERIFIED, `CoDMP.exe`'s dispatcher
(0x401df0, table 0x4041f0): the trace syscalls 0x26..0x29 call 0x424b50 and
0x425040. INFERRED: those are Q3's `CM_BoxTrace` and
`CM_TransformedBoxTrace` over a clip handle, so no cgame trace walks a DObj's
collision surfaces and the cgame never meets a script model's mesh. VERIFIED, game.mp 0x61010 and `cod_lnxded` 0x80908f0
(`cod11-player-clip.md` 4.1): a `script_model`'s contents are 0x2080, which
misses `SV_LinkEntity`'s 0x2000001. INFERRED: it goes out with `solid` 0 and
stays off the cgame's solid list (`CG_BuildSolidList`, 0x30028d50), and the
whole-model bounds reject is the server's alone; vcod's predictor, which
clips only `solid` bodies, matches.

VERIFIED, live (retail 1.1d, mp_carentan, `probe_scriptmodel`'s pair parts, `+set probe_part rounds|grenade|blast`, fixtures
`mp_carentan-scriptmodel-{rounds,grenade,blast}.txt`; recipe in each
header): with an axis player 256 units off an allies `--probe-uo-weapon
aim` Thompson and a barrel 26 units in front of him, no round reached him
while the barrel stood, shown or hidden (the shooter firing from about 1 s
into the 3 s shown phase on), and every 100 ms one did from the delete on
(30 a leg, 40 the lower torso). A round on the model ends there: it neither
passes nor carries through to the player behind. A frag thrown
(`--probe-uo-weapon throw`) at the barrel's middle from 64 units strikes
its side at about 100 ms and comes off sideways at full bounce, as off a
wall (12.4), and goes off at its rest 3.5 s on. With a barrel stood 32
units from a resting frag, between it and both players (at 96 and about
190 units), the blast reaches neither; the same frag in the open hits the
two for 88 and 7.

vcod: `scriptmodel_ab` replays all three on ours. The rounds and the
shelter matched as they stood; the barrel bounce matches every frame to
0.01. The second throw in the grenade part, the barrel gone, struck the
player behind, which is what 12.4's "The bounce off a player" is about.

VERIFIED, retail `bulletTrace` along the same segments on 1.1d and 1.5b
(`client-probes/probe_patchseams`): the two servers stop a
round on a curved patch at different points of its facet seams, because
1.5b's and UO's engine build rounds where 1.1d's does not. A round on a
1.5 or UO server clips patches and terrain in that build's arithmetic
(`bsp-ibsp59-format.md`, "Patch collision on 1.5 and UO").

The `position` on a world hit or a miss is the world trace's own end
point, not a lerp the builtin makes. VERIFIED, 1.5b game.mp
`GScr_BulletTrace` (0x5f456): it passes `trap_LocationalTrace`'s result
straight to `Scr_AddVector` (0x5f559). INFERRED, off the control flow
of 1.5b `cod_lnxded`'s `SV_Trace` (0x8099fd1, reached for
`trap_LocationalTrace` through syscall 0x2e with no `mins` or `maxs`):
a point trace walks the entities (0x805f48a) and keeps the trace they
leave, and `SV_ClipMoveToEntity` (0x8099732) writes the end point only
when a body or model lowers the fraction (0x8099b18, 3.1, "The
arithmetic on 1.5 and UO"); otherwise the end point stays
`CM_BoxTrace`'s (`collision::cm_endpos`, `cod15-movement.md` 7). The
box path's own rewrite (0x809a44d..0x809a4d4) is never reached by a
locational trace. VERIFIED, live (retail 1.5b and UO 1.51b, gdb on
`BG_CheckProneValid`'s `fTorsoHeight` store,
1.5b game.mp 0x1e589, UO 0x22ecf): `probe_scriptmodel`'s prone target,
put on `ground_at` + 0.125 by `setorigin`, starts his drop at z
-22.749998 (0xc1b5ffff) on both servers, where a lerp rounding the
product and the sum apart gives -22.75. vcod's builtin reads the trace's
`endpos` for the world and keeps `combat::trace_point` for a script model or
a body.

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
game syscall dispatcher, vcod's name `SV_GameSystemCalls`, is at `0x8087dcc`; it
reads `args[0]`, rejects anything above `0xd3` and jumps through the
212-entry table at `0x80d4eac`, whose default arm at `0x8089100` prints
`"Bad game system trap: %i"` (`0x80d4e8c`). VERIFIED: table entry 43 is
`0x8088316`, and that case pushes eleven arguments and calls `0x80916f4`.
INFERRED, mapping the pushes back to the trap's arguments: the call is
`SV_Trace(results, start, mins=NULL, maxs=NULL, end, passEntityNum,
contentmask, 0, locational=1, priorityMap, staticmodels=1)`. VERIFIED:
`0x80916f4` is the same address CoDExtended calls directly
(CoDExtended's `src/sv_world.c:20`), and the prototype
published there names the `locational` flag and the `char *priorityMap`,
which confirms the tail of that signature independently.

**Per entity: pose first, then the bone trace.** VERIFIED, the operands,
offsets, immediates and call targets in this list are read out of the
instructions of `SV_ClipMoveToEntity` (vcod's name, `0x809105c`); INFERRED, its
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
  Bit `0x4` selects the xmodel collision trace (`0x80c52c0`, section 2.7);
  bit `0x2` without `0x4` is the
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
  frame, so the bone trace works entirely in entity space, and `0x80c4564` (vcod's
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
  greater, by the skip above -- raises `bestPriority` and takes the hit.
  VERIFIED, `cod_lnxded` `0x080c4564` and UO's `coduo_lnxded` `0x080c7394`:
  each bone's slab test starts its exit bound at the stored fraction
  (`*param_5`, the caller's fraction before any hit), so a box the segment
  enters past the hit already stored is rejected whatever its priority.
  INFERRED: a higher priority beats distance only when its bone comes first
  in the walk; listed after a nearer lower-priority hit, it never scores.
  VERIFIED, vcod measurement (`leanhit_ab`, 3.6): ours took the farther
  higher-priority box, and with this rule the 1.5b and UO rounds that took
  `torso_upper` where retail took `torso_lower` now match. The result gets the fraction, the bone's name id, the
  effective hit-location code, and a normal that is a signed row of the bone
  matrix.
- A segment that starts inside a box sets the flag byte at `+0x18` of the
  local result. VERIFIED, `cod_lnxded` 0x80c4bc5 and UO's `coduo_lnxded`
  0x80c7a8a: only when its end is inside the box too does it set `+0x19`,
  zero the fraction and return; a segment that leaves the box goes on to
  the next bone with the flag set. INFERRED, off `SV_ClipMoveToEntity` (UO
  0x809aa78): with no bone hit the two bytes are ORed into the trace's
  start-solid and all-solid. vcod's `bone_trace` follows the rule on both
  builds (`a_segment_leaving_a_box_it_starts_in_goes_on`); nothing in vcod
  reads the start-solid flag of a walk that scored no bone.

So the engine's box record is 40 bytes, `{ vec3 mins; vec3 maxs; vec3
sphereCentre; float radius }`. The file supplies the first 24; INFERRED that
the engine derives the sphere from them at load, since the file carries nothing
else. INFERRED, off the broad phase (UO 0x80c77f1): `radius` at `+0x24` is
compared against a squared distance, so it holds the radius squared.

#### The arithmetic on 1.5 and UO

1.5b's and UO's engines (gcc 3.2.3, unoptimized) store every float
assignment, where 1.1d's keeps them on the x87 stack. VERIFIED, the
disassembly, 1.5b `cod_lnxded` and UO `coduo_lnxded`, one instruction
sequence apart from addresses and struct offsets in each pair:

- VERIFIED, the body's frame: `AnglesToAxis` (UO 0x806a46e, 1.5b
  0x806a55e) over `AngleVectors` (UO 0x80674ad): each angle times the
  double `pi / 180` stored as a float, its `fsincos` (UO 0x806b4de)
  stored, each component's products and sum on the x87 stack and the
  component stored once. A player's `(0, yaw, 0)` gives forward `(cos,
  sin, 0)`, left `(-sin, cos, 0)` and up `(0, 0, 1)`. The angles are
  `r.currentAngles`, so a body a `setPlayerAngles` turned (3.7) traces
  in the same arithmetic of the angles it set.
- VERIFIED, `start` and `end` into the body's frame (UO 0x8069416, 1.5b
  0x8069506): each `p - origin` stored, then each axis's dot product
  rounded once.
- VERIFIED, each bone's matrix (UO 0x80c7d82 from the skeleton build
  0x80c5c96, 1.5b 0x80d29ee): off the bone's quaternion, `2 / |q|^2`
  stored, `xx`, `yy`, `zz` each stored twice (the square, then times the
  scale), `x * s` and `y * s` stored, `xy`, `xz`, `xw`, `yz`, `yw` each
  stored, `z * w * s` rounded once, and each element (`1 - (yy + zz)`,
  `xy + zw`, ...) rounded once. Rows 0, 1, 2 are the bone's axes, the
  translation the bone's origin copied as built.
- VERIFIED, each point into the bone's frame (UO 0x80c72ea, 1.5b
  0x80d1f56): as the body's frame above, against the bone's matrix.
- VERIFIED, the slab test (UO 0x80c788a..0x80c7a5f, 1.5b 0x80d24f6..0x80d26cb):
  each `(p - plane) * sign` stored, each `d1 - d2` stored, each `d1 /
  (d1 - d2)` stored; the comparisons `d1 > enter * den` and `d1 > exit *
  den` run on the stack.
- VERIFIED, the world normal (UO 0x8069154, 1.5b 0x8069244): the bone row
  times its sign, turned by the body's axes, each component rounded once.
- VERIFIED, the end point on a body or a model: `SV_ClipMoveToEntity`
  (1.5b 0x8099b18 in 0x8099732, UO 0x809ab10), once the bone trace
  lowered the fraction, rounds each `start + (end - start) * fraction`
  once, the difference and the product on the x87 stack. A box trace's
  last step in `SV_Trace` (1.5b 0x809a45f, UO 0x809b457) stores `end -
  start` first, but `trap_LocationalTrace` is a point trace and never
  reaches it. VERIFIED, gdb on retail 1.5b (a breakpoint on
  0x809a4da, which both arms of that step reach): it does not fire
  between the frag's strike on a prone body and its next trace. VERIFIED,
  the strike's start, end and fraction from the same run (1.5b
  0x80d2000's caller, `-0x98(%ebp)` after the call) give retail's next
  start, z 0xc12aabc1, in the kept form and 0xc12aabc2 in the stored one.
- VERIFIED, 1.1d's end point (`cod_lnxded` 0x8091352, after the bone
  trace at 0x80912f0): the same form, each component's difference,
  product and sum on the x87 stack and rounded once (0x8091366,
  0x8091375, 0x8091387). vcod measurement: replaying the turret
  captures, 8 of 166 1.1d body traces round a step apart in `f32`: 7
  grid rays, whose cells print the fraction and not the point, and the
  `-spawn` capture's wound, whose script logs no point. No fixture shows
  the difference.

vcod: `bonetrace::bone_trace_in` under `Arith::Stored` (the frame, the
matrix, the slab), `DobjStamps::axis_stored` (`x87::angles_to_axis` of
the player's `r.currentAngles`),
`combat::trace_point` for a round, a swing, a missile and a
`bulletTrace` that stops on a body. The
bone's quaternion and origin come from the engine's own pose (next
section).

VERIFIED, vcod measurement (`scriptmodel_ab`, the prone captures): before
the port a frag's first strike on a prone body on mp_carentan landed 4
float steps off retail's z on both 1.5b captures and on UO
(-10.666936 against -10.666932); the frame, slab and end point alone
brought it to 1 step and left the rest on him 10 off; with the stored
matrix the strike is retail's bit on all three and the contacts after it
sit 1 to 3 steps off in z. That bit was two errors cancelling: the end
point's stored move against a turned body's frame, next section's
"The prone strike on mp_carentan".

#### The pose on 1.5 and UO

The bones a 1.5b or UO trace meets are not a normalized pose. VERIFIED,
read out of a live UO 1.51b server under gdb (`client-probes/probe_pronebones` with `probe_rays 1`, the prone target
on mp_brecourt's grade): at `SV_LocationalTraceModel`'s entry (UO
0x80c7394) the DObj's skeleton (`dobj+4`, `dobj+0x17` bones) holds a
64-byte matrix per bone from `+0x30` and then a 32-byte record per bone:
a quaternion, a weight and a translation. At the skeleton build's entry
(0x80c5c96) the records hold the frame's local pose: each animated
bone's quaternion is its key's four shorts times `1 / 32767` (0x80ea1ac,
float `0x38000100`), not normalized (`|q|^2 - 1` from -2.3e-5 to
1.2e-5 over the body), and a bone no clip keys holds its xmodel bind
rotation's shorts the same way, `w` rebuilt in integers as a key's is.
The build turned those records into every matrix and world record of
the dump to the bit (vcod measurement, a Python run of the functions
below on the dumped records). The same holds on 1.5b
(`cod_lnxded` 1.5b, trace 0x80d2000, build 0x80d0902, one layout).

- VERIFIED, the sampler (UO 0x80b9b72, 0x80ba17a, the end-key arm
  0x80ba782): per bone a clip tracks, each short lerped between its two
  keys by the stored fraction `(frame - key) / (next - key)` and times
  the stored `weight / 32767` is added to the record's sum, each sum
  stored; translations lerp and scale by the weight the same way; the
  weight is added to the record's `+0x10`. A track with no rotation
  adds the weight to `w`.
- VERIFIED, the tree walk's arithmetic (UO 0x80bdbdc, called by the
  anim calc 0x80bf2a8 on the root at weight 1 into the records): the
  root's division multiplies every sum by the stored `1 / w`; a nested
  node's normalization scales by the stored `w / (float)sqrt(|q|^2)`,
  the square root libm's double. INFERRED, off its branches: a node
  with one weighted child passes the call to it at its own weight; with
  two or more, the first child writes the buffer and the rest add to it,
  then the root divides by the summed weight and a nested node
  normalizes its sum to its weight in place or, with a buffer of its
  own, adds it so normalized into its parent's. VERIFIED, the dump: a
  clip that plays alone reaches the build as its raw shorts.
  VERIFIED, 0x80bdbdc's loop: a node's children are the tree nodes from
  its first child (`+0xe`) on, its child count at `+8`, visited by
  ascending node index, and the game's anim index is that node index
  (VERIFIED, `BG_FinalizePlayerAnims` walks `trap_XAnimGetAnimTreeSize`
  entries by it). VERIFIED, 0x80be0f3..0x80be121 (1.5b 0x80c8d5f..
  0x80c8d8d): a nested node's `|q|^2` is `((q0² + q1²) + q2²) + q3²` on
  the x87 stack, stored once; vcod summed from q3 until 2026-11-14
  (`skeleton::walk`). vcod measurement: ordering the legs group's clips
  by ascending anim index puts the 1.5b `turret_ab` hitgrid row 30500
  (`torso_upper`) 1.9e-6 off in z, the clock's fading-first order
  (`ChannelClock::layers`) and descending index both hold it to the bit,
  and no other pose gate tells the three apart (nor legs ahead of torso
  under `main` from torso ahead, file order). Open: which tree node a
  vcod layer stands for in that walk (a sibling pair 32/33 or 94/122 sits
  in the row); vcod keeps the clock's order.
- VERIFIED, the controls (game.mp.uo `G_DObjSetLocalTag` 0x88e34,
  `G_DObjSetControlTagAngles` 0x88ec0, both through 0x88cf6): the
  record is `QuatMultiply(roll, QuatMultiply(pitch, yaw))` (0x4d25f,
  each component stored), each half angle `(float)(pi / 360) * angle`
  passed as a float and its sine and cosine stored (0x4f464), the
  weight 0 and the translation the tag's (the origin for a control).
- VERIFIED, the build's arithmetic (UO 0x80c5c96): a bone's quaternion
  is its record times its parent's (0x80c8056) or, for a control bone,
  the other order (0x80c818e, the control in model space), each
  component stored; its matrix off that quaternion (0x80c7d82, above);
  its origin is its record's translation plus its xmodel bind position
  (stored), turned through the parent's matrix and added to the parent's
  origin, each component rounded once (0x80c7f9a). INFERRED, off its
  branches: a root keeps its record, the control mask picks 0x80c818e,
  and a part's bone that repeats an earlier part's copies that bone's
  record and matrix.

vcod: `skeleton::Record`, kept by `PoseBuffer::blend_nodes` and the
controllers, `PoseBuffer::stored_worlds` (the build), read by the stored
bone trace and by a parent's tag in `linkTo`, a client's
(`link::client_bone_stored`) or any other entity's bind records
(`link::model_bone_stored`), `G_CalcTagParentAxis` through the parent's
stored `AnglesToAxis` and `Frame::within_stored` (cod11-movers.md 16,
"The tag on 1.5 and UO").

VERIFIED, vcod measurement (`pronepose_ab`'s `probe_rays` gates, the
target's own cmds replayed): on UO the target's origin, eight bone marks
and all 406 rays a frame that stop on him are retail's bits; before the
port the rays sat 1 to 139 steps off. Every record and world record of
the gdb dump matches ours. On 1.5b the origin and the marks are
retail's bits, and since the clip's float time below so is every ray.
Before that the rays sat within 3 steps (1.5b's prone idle moves, and
the dump's locals up to 8 steps off ours where a key is lerped).

#### The clip's float time

The frame a 1.5b or UO clip is sampled at is not its seconds times its
frame rate. VERIFIED, `cod_lnxded` 1.5b 0x80c84f0 (UO `coduo_lnxded`
0x80bd884, the same code), the leaf arm, reached through syscall 0x70
from `G_DObjUpdateServerTime` (1.5b game.mp 0x7c5b1, `level+0x1f0`
times 0.001 passed as a float, 0.05 at `sv_fps 20`): for a node whose
weight (`+0x3c`) is not 0 the new time is the node's old time (`+0x2c`)
plus `dtime * rate (+0x40) * frequency (parts +0x8)`, the sum left on
the x87 stack (`flds`, `fmuls`, `fmuls`, `flds`, `faddp`, 0x80c85ae) and
stored once as a float. A looping clip then adds or takes 1 until the
time is in [0, 1), each step stored; any other clip stops at 1 and keeps
its old time when the sum is at or under 0. A nested node passes its
children `dtime * rate`. VERIFIED, UO 0x80b816e: the frequency is the
frame rate over `numframes` (`frame_count - 1`, header frames plus the
loop bit less one), divided as floats (0x80b83e9) and stored to parts +8 (0x80b8407), 0 for one
frame. VERIFIED, 1.5b: the loader 0x80c2dda divides at 0x80c3055 and
stores at 0x80c3073, `fldz` at 0x80c305f. VERIFIED, the sampler (UO 0x80b9b72, 1.5b 0x80c47de, the same
instructions): the frame is the time times `numframes` as floats,
stored, and a key's fraction is taken off that.

VERIFIED, 1.5b game.mp's player anim setter (the `trap_XAnimSetTime`
calls): a looping movement clip replacing another starts at the old
clip's `trap_XAnimGetTime`, and a death anim on a body not freshly
cloned at 1.0.

A notetrack never splits a player's step. INFERRED, off the control
flow of `G_XAnimUpdateEnt` (1.5b game.mp 0x50300) and the engine's step
(UO 0x80bf1a2, 1.5b 0x80c9e0e): the game runs `Scr_RunCurrentThreads`
and calls `G_DObjUpdateServerTime` again at the frame's whole `dtime`
while it returns non-zero; the step returns non-zero only when the
fraction of the step that reaches the next note (0x80bd01a, 1.5b
0x80c7c86) is under 1, after stepping the node to `dtime * f + 0.001`
from the frame's start time; the next call steps the whole `dtime` from
that same start, so the split's last step stores the bits an unsplit one
does. VERIFIED, UO 0x80bf22e: the 0.001 is the float at 0x80ea3d0, added
to `dtime * f`. VERIFIED, UO 0x80bbc4c: the fraction is 1.0 whenever the
DObj's owner (`dobj+0x12`, loaded at 0x80bf169) is 0, and the game's
`DObjCreate` wrapper for a player passes 0 (UO game.mp 0x1e732, 1.5b
0x1a35a; INFERRED that it lands in `+0x12`). VERIFIED, a census of the
stock clips (1.5's `pak*` and UO's `pakuo00`/`01`, the notes after the
keys): 83 of the 170 clips `mp/playeranim.script` names carry notes on
1.5 and 86 of 217 on UO (the `pb_combatrun_*` footsteps, the prone
crawls, the takeoffs' `gravity on`, the deaths' `bodyfall`, `pt_*`
shots, swings and reloads), and the loader (1.5b 0x80c2cc4, UO
0x80b8058) appends an `end` note at 1.0 to each. So a split would reach
every run, and the owner test is what keeps it away. vcod's one step per
frame is retail's for a player. Open: an entity whose DObj names an
owner (`G_DObjUpdate` passes `G_GetGameId`) does split.

vcod: `animclock::Play` keeps the node time beside the seconds, stepped
in whole 50 ms frames with the sum in `f64` and stored as `f32`
(`Play::step`), `Layer::time` hands it out, and the server's pose on
1.5 and UO samples at `time * numframes` (`PoseInputs::node_time`).
A clip under a node with children (whose frequency the clock does not
hold) stays on the seconds, and 1.1d keeps them throughout: its pose is
the glam one, not the stored build.

A leaf under a plain node keeps its own time. VERIFIED, UO
`coduo_lnxded` 0x80bd884: a node with children whose flags (`+0xc`) carry
neither sync bit (`& 3`, 1 `loopsync`, 2 `nonloopsync`) calls itself on
each child with `dtime * rate (+0x40)`, and only a node whose weight is
not 0 steps or descends; a sync node steps a time of its own at the
weighted mean of its children's `frequency * rate` (0x80bb732, children
with weight and a frequency only) and copies it into every descendant
(0x80bd7e4). VERIFIED, 0x80c05a2 (the goal-weight setter): a node with
no info is allocated (0x80bf9be) and its time block zeroed (0x38 bytes
from `+0xc`) when given a goal weight; VERIFIED, 0x80bcc1e: one whose
weight and goal both read 0 at the frame's weight step is freed, so set
again it starts at time 0. VERIFIED, game.mp.uo `G_XAnimUpdate`
(0x69983): every entity's weight step (`trap_DObjInitServerTime`) runs
before any time step (`G_XAnimUpdateEnt`), so a time step reads the
frame's stepped weight. VERIFIED, game.mp.uo disassembly: the seat
blend `G_PlayerVehiclePositionAndBlend` sets its two leaves and its group
at rate 1.0 (`movl $0x3f800000,0x10(%esp)` at 0x908ef, 0x909c6 and
0x90a41, ahead of `trap_XAnimSetGoalWeight`), and the gunner placement
(0x6ac06) sets every goal weight at rate 1.0; a seat's legs anims carry
no `moveSpeed`, so their own rate is 1 (INFERRED, the clock's 0x2ada5
arm). The UO seats' groups (`vehicleJeepDriverIdle` and kin) are
`: complete` with no sync; the MG42 groups, the tank gunner's seat
included, are sync (VERIFIED, `animtrees/multiplayer.atr` in
`pakuo00.pk3`).

vcod: `PlayerAnims::leaf_motion` holds the frequency of each leaf under
a plain `complete` group, `playerpose::SeatRamp` starts a
`animclock::LeafTime` at 0 for a leaf set from no weight and keeps it
while the leaf keeps weight, and the server's 1.5 and UO pose samples a
seat leaf at it (`PoseInputs::leaf_times`). A sync group's leaves sample
the group's own time: an MG42 gunner's at the weighted mean frequency
(cod11-turrets.md 13.9); the tank gunner's seat stays on the seconds. VERIFIED,
vcod measurement: `vehicle_ab`'s jeep driver bones move by
at most 0.01 on 8 of 2170 samples, and no UO vehicle or turret gate
changes: every stock seat leaf is a single frame (`c_mp_jeep_driver_turnL`,
`_turnR`, `c_mp_jeep_gunner_idle`, the MG42 aim leaves), a two-frame clip
that clamps on its first step (`c_mp_jeep_driver_idle`) or
`c_mp_jeep_driver_drivingA` (30 frames, looping), whose node the legs
clock already started at 0 when the leaf was allocated.
VERIFIED, vcod measurement: `pronepose_ab`'s 1.5b mp_brecourt rays, the
origin and the eight marks are retail's bits; the 1.5b mp_carentan prone rest stays one step off, as it was
on seconds (next section).

#### The setter on a weighted node

1.5b's and UO's player anim setter (game.mp 0x28ced, UO 0x2ce5f; the
cgames' 1.5 0x30003870 and UO 0x30003a90 make the same calls, VERIFIED) differs from
1.1d's in when a new anim's node time is set.

- VERIFIED, 1.5b game.mp 0x28ced and UO 0x2ce5f: for a non-death anim
  with a `moveSpeed` (`animation_t +0x44`), the setter reads
  `trap_XAnimGetWeight` of the new node before its
  `trap_XAnimSetCompleteGoalWeight`, and calls `trap_XAnimSetTime` with
  the phase only when that weight equals 0.0 (the float at 1.5b 0x83ccc,
  file offset 0x73ccc, and UO 0xa95ec, file 0x995ec). 1.1d's 0x2a600 sets the phase whenever the anim
  has a `moveSpeed` (cod11-movers.md 15, "The phase").
- VERIFIED, the same calls: the goal weight call's restart argument is
  `!legs` (`legs` is `lf == ci+0x380`, 1.5b 0x28ced's first compare), so
  the torso setter restarts its node and the legs setter does not. 1.1d
  passes 1 on both channels. A fresh corpse's death anim (the `param_4`
  arm) passes 0 on either channel and sets no time.
- VERIFIED, 1.5b `cod_lnxded`: syscall 99 goes to 0x080cbe66, which sets
  the node's goal weight (0x080cb20e, raising a zero weight to
  `0.001 * goal`, 0x080e5b24), raises each ancestor whose weight is 0,
  then calls 0x080cba7a. That clears the node's server time (0x080cb0c2,
  `+0x28..+0x32`) when the restart argument is set or 0x080ca848 reads
  the node inactive: an ancestor at weight 0 (0x080ca6e8) or the node's
  own weight 0 (0x080ca768). After the goal call neither holds, so with
  restart 0 the time stays. UO's 0x080c004a, 0x080c0e0e and 0x080bfbdc
  have the same calls in the same order.
- VERIFIED, 1.5b `cod_lnxded` 0x080c769e (the server half's weight step):
  a leaf whose weight and goal weight are 0 on both halves is freed.
  INFERRED: a node that read weight 0 is a new allocation, zeroed by
  0x080cb20e's memset, so it starts at 0 and only `XAnimSetTime` moves
  it.

So on 1.5b and UO a legs anim whose node still has weight (a clip
blending out and set again, or the same anim re-toggled) keeps playing
from its own time. A still clip there gets no phase in either case. A
weighted torso node restarts at 0, even for a movement clip.

vcod: `animclock::ChannelClock::set` under the windowed (1.5/UO) setter
keeps a fading clip's `Play` when the legs, or a fresh corpse's death,
take it back. A torso one restarts at 0, and only a node no fading entry
holds gets the phase (`a_weighted_node_keeps_its_time_on_15`).

The two channels never share a node on stock content. VERIFIED,
`BG_PlayAnim` (1.1d game.mp 0x2c338, UO 0x1bfb9): a `both` line writes
its anim into `legsAnim` and index 0 into `torsoAnim`
(player-model-anim-system.md, "The takeoff and the landing"). VERIFIED,
1.5b and UO `BG_PlayerAnimation` (1.5b game.mp 0x1aed5, the legs setter
call at 0x1af97 before the torso's at 0x1afc2; UO 0x1f462, 0x1f524,
0x1f54f): each setter runs only when its channel's wire value changed.
VERIFIED, `mp/playeranim.script` on 1.5 and UO: no `torso` line names
an anim a `legs` or `both` line names. INFERRED: so vcod's two clocks never
stand for one node. One UO edge is left (VERIFIED, the files): `torso
c_mp_lmg_deploy_prone` names a leaf of the atr's `legs` group, which
the torso setter then raises beside the legs clip under `legs`, while
vcod poses every torso layer under `torso` (`playerpose::pose_player`);
no gate poses that deploy.

VERIFIED, live (retail 1.5b and UO 1.51b, `probe_legs` with
a `--save-legs --capture-tag stopgo-*` client, fixtures
`movers/mp_carentan-dm-legs{,-client}-stopgo-{1.5,uo}.txt`, recipe in
each header): the legs leave a run some 350 ms after its keys let go
(the body has to slow under 10 units/s), and a go 400-500 ms after the
release takes the run back 100-200 ms into its blend to the idle. On
both servers the run's bones then go on from the run's own time:
`legs_ab`'s `a_body_stopping_and_going_is_posed_like_retail_on_15` /
`_on_uo` match every row from the first stop on to 0.05 units and 0.3
degrees (and the settle and the first run too, with
the spawn idle dated as cod11-movers.md 15, "The spawn idle on 1.5 and
UO", has it), and with `animclock::restart_weighted_nodes` forcing 1.1's
restart the same replays miss 83 (1.5b) and 78 (UO) rows from the first
go on, up to 29 units and 97 degrees. A strafe swap (right, 150 ms
left, right) takes the right strafe back the same way.

#### The prone strike on mp_carentan

VERIFIED, live (retail 1.5b and UO 1.51b,
`probe_scriptmodel`'s prone part with both clients' cmds logged,
fixtures `mp_carentan-scriptmodel-prone-rest-{1.5,uo}-cmds.txt`; recipe
in each header): the first frag strikes the prone target's back and, on
1.5b, rests on it until he is moved; on UO it strikes him again lower
down and slides off. Replaying both clients' cmds, three inputs must be
retail's:

- the site's ground trace (2.7, the `bulletTrace` end point), which put
  the target's drop a float step high;
- the airborne body's `fTorsoHeight` (`cod15-movement.md` 4);
- the legs on a lie-down pressed mid-fall
  (`player-model-anim-system.md`, "The lie-down in the air").

VERIFIED, vcod measurement (`scriptmodel_ab`'s `..._like_retail_on_15`
and `..._on_uo`): with them the thrower's site offset, the target's
origin at every cmd of his drop, his body and his legs clips match the
captures and the gdb reads, and every frag snapshot carries retail's
`trType` and ground. With them alone the first strike on him still
landed 2 float steps off retail's z on 1.5b (-10.992799 against
-10.992797) and 1 on UO, UO's second strike 3.

VERIFIED, live (retail 1.5b and UO 1.51b under gdb, the same
recipe on ports 34181 and 34183, both clients' cmds logged; scripts and
dumps kept locally): breakpoints on the locational trace (1.5b
0x80d2000, UO 0x80c7394; the DObj's skeleton, the body-frame start and
end, the caller's world start and end), on the skeleton build (1.5b
0x80d0902, UO 0x80c5c96; the records) and after the call (1.5b
0x8099a80, UO 0x809aa78; the fraction and normal), with each run's cmds
replayed on ours. Ours' records, world quaternions, origins and
matrices at the strike matched the dump on all 101 bones (113 on retail
with the head and helmet parts' repeats), and so did the strike's
fraction and normal. Two things did not:

- VERIFIED, the body's frame. The probe turns the target with `setPlayerAngles`
  to (0, 225, 0), which `r.currentAngles` holds until his next cmd
  (3.7). vcod turned that frame through an `f32` quaternion, forward x
  0xbf3504f5; retail's `AnglesToAxis` gives 0xbf3504f4, the stored
  sine and cosine above. Every start and end in the body's frame was 1
  step off in x and further in the near-zero y.
- VERIFIED, the end point: the kept form above, not the stored one.

VERIFIED, vcod measurement: with both, every one of the runs' 19 (1.5b)
and 37 (UO) traces against the target starts and ends on retail's bits,
and `scriptmodel_ab`'s mp_carentan prone gates hold every base and rest
to the bit on all five captures (three 1.5b, two UO, the cmd-less ones
included).

VERIFIED, vcod measurement (`scriptmodel_ab`, the UO mp_brecourt roll):
the first frag strikes the prone target's legs and rests on the grade
by them. On an older capture that logged neither client's cmds,
the strike lands 0.007 off retail's (118 steps in y, 447 in z) whatever
the pose's arithmetic. VERIFIED, three reruns of that recipe: the target's drop onto the grade left his `fTorsoHeight`
at -0.487 and -0.516 in the two runs that lay him along 270 (-0.425 and
-0.491 in UO `probe_pronebones` runs on the same spot, -0.445 on 1.5b), and
the first frag rested at (1666.97, 840.95) and (1667.06, 841.31): the
strike rides the drop. On the rerun whose cmds both clients logged
(`-roll-uo-cmds.txt`), with those cmds replayed, both frags' every state
is retail's to the bit.

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
1.5b's and UO's trace stores every float variable where 1.1d's keeps
them; vcod's trace keeps them on every patch. The stores move an entry
by about 3e-5 units (`cod11-turrets.md` 13.5).

The victim is posed per shot, not per frame, through
`playerpose::pose_player` (`crates/server/src/game/combat.rs`) over the same
grafted body-plus-head-plus-helmet skeleton the client draws (an
attachment's box on a bone that already has one rides on as a second box,
3.6; `crates/server/src/game/hitrig.rs`): the legs and torso clips blended
per bone as two tree nodes (cod11-movers.md 15), then the controllers, whose
torso pitch, torso yaw and legs yaw swing after the view (16.3, 16.4). The
models, clips, start times, pitch and lean are the ones the victim's last end
frame committed (`ClientSim::commit_pose`), the origin, yaw and box the ones
its last cmd left, the split 16.1 reads off retail. A client whose models will
not load has no rig, and its hits carry `none`, which the shipped multiplier
table reads as full damage; retail has no such state.

#### What the sweep measured

`--probe-sweep` (`crates/client/src/probe.rs`) taps once per entry of a
table of pitch offsets around the aim at the target's eye and echoes the
offset on its `!trace` line; the retail server logs `sHitLoc` per hit in its
`games_mp.log`, a `D;` record for a wound and a `K;` record for the kill, and
each hit the target's own trace registers pairs with the shooter's snapshot
at or before it. Run against retail: mp_carentan tdm,
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

### 3.6 Measured: a leaning body, and the helmet's own box

VERIFIED, live (retail 1.1d, mp_carentan tdm,
`client-probes/probe_leanhit`, fixture
`crates/server/tests/fixtures/leanhit/mp_carentan-leanhit.txt`): an axis
target held 256 units from an allies shooter, 13 phases from level time
20000 (standing, crouched and prone, each level and full lean either way,
then the shooter leaning either way). Each phase a grid of level
`bulletTrace`s with `hitCharacters` (2.7) crosses the target: 13 heights by
11 columns along the shooter's facing and 13 by 15 across it, each cell
the distance to where the ray stopped on him. Then the sighted
`springfield_mp` fires once, aimed by the gsc from the shooter's eye
moved by a full lean (19.2 across, 5.5 down, `pmove::aim::lean_offset`).
All 13 rounds struck; the damage callback's location, damage and point are
the fixture's `PROBE hit` lines.

What the run showed, VERIFIED against ours by `crates/server/tests/leanhit_ab.rs`:

- The helmet's box is traced as well as the head model's. VERIFIED, off
  the paks: the head model (`xmodel/basehead_axis_a_01`) gives `bip01
  head` a box spanning -0.43..9.04 along the bone, and
  `xmodel/gear_german_helmet_falshrm_camo`'s root, also `bip01 head`,
  carries its own, 0..10.35 along and a unit wider on one side. VERIFIED, the
  fixture: a standing target's head stops a level ray 70 units up (and
  ours, keeping only the first box, let it by), and with both boxes ours
  matches the head cells in every phase. This is 3.1's per-part bone loop
  with two parts' bones on one matrix; vcod's grafted skeleton now keeps
  the second box as a box-only child of the merged bone
  (`skeleton::adopt_hit_box`).
- The lean pose of 16.4 holds as ported: with the helmet box in, every
  prone phase and the standing lean right match to 0.03 units, the
  crouched leans to 0.5 bar three cells, and every round's point lands
  within 0.3 units of retail's with the same location and damage, a
  leaning shooter's (2.1) included. Scaling any of 16.4's lean terms
  (`tag_origin`'s roll and shift, the three back rolls) by 0.7 to 2 on ours
  only moved the standing lean cells further off.
- A body's torso yaw keeps whatever its spawn left it until something
  swings it (16.4): the first capture, whose target never moved after his
  placement, read a different torso on retail than on ours, which had
  given him a different spawn point. The probe lays him prone and stands
  him up first.

Two inputs decide the cells (52 missed without them), neither the boxes nor
the lean:

- The clock the grid poses at. VERIFIED, `game.mp.i386.so`: `G_RunFrame`'s
  first `Scr_RunCurrentThreads` (0x50653) runs before
  `trap_DObjInitServerTime` and `G_DObjUpdateServerTime` (0x5069f,
  0x506ed; cod11-movers.md 15, "A bone asked for before anim update").
  VERIFIED, `cod_lnxded`: the time step 0x080bbab0 adds `dtime * rate *
  frequency` to a node's old time (`+0x2c`, copied from the time by the
  weight step 0x080bb130 first) for every node whose weight is not 0,
  and the goal weight setter 0x080bd674 raises a zero weight to `0.001 *
  goal`, so a clip set at an end frame advances from the next frame's
  update on; no clip starts a frame late. INFERRED, off those: the
  probe's grid thread, due on the last frame's clock (its `wait` lands on
  a frame time), traces bodies posed at the last frame's level time.
  vcod's `bulletTrace` and the script `radiusDamage` walk pose them at
  `GameHost::dobj_time_ms`. VERIFIED, 1.5b and UO:
  the frame's one thread pass runs before the update as well, and a UO
  spawn's DObj runs a frame ahead (`cod11-turrets.md` 13.7). VERIFIED, vcod
  measurement: the standing cells' median drops from 0.13 to 0.02, the
  cells past 0.5 from 52 to 32.
- The cmd times. Even 16 ms cmds miss; the net probe in its
  `direct` mode sends 16, 16 and 18 ms ones, the last on the frame's time
  (`ProbeHold::framed`). VERIFIED, vcod measurement: with those every cell
  of the first capture matches, and every cell, bone and round of a
  second 1.1d capture (below) does to 0.0007.

VERIFIED, live (retail 1.1d, 1.5b and UO 1.51b,
`probe_leanhit` with `probe_bones 1`, fixtures `mp_carentan-leanhit-bones.txt`,
`-1.5.txt` and `-uo.txt`): before each grid the probe links a
script_origin to eleven of the target's bones (pelvis, spine2, neck,
head, both upper arms, forearms and hands, `tag_weapon_right`) and logs
them and his origin, each as an offset from his spot, on three frames.
Against ours (`leanhit_ab`, cmds as above, each capture's join times):

- 1.1d: every bone and the origin within 0.0007 units, every cell and
  round as retail's.
- UO: every bone, the origin and every cell likewise, the standing,
  crouched and prone leans included, so the `Split::Later` lean terms of
  16.4 hold as ported. One round differed, here and on 1.5b: the crouched
  target leaning left took it in `torso_lower` where ours took
  `torso_upper` a unit farther along the ray. VERIFIED, vcod measurement:
  that is the hit-box walk's clip (3.1); with it the round matches on both.
- 1.5b: the target's origin stands 0.0402 under ours (offset -0.0306
  against ours' 0.0096) in both 1.5b runs; every bone of a standing or
  crouched body carries the same offset and none else (to 0.0006), and a
  prone body's bones match without it. The hold's `setOrigin` raises him
  a unit and he drops to the ground over the next cmds. The `-cmds`
  captures below settle it as the split. That capture logged no cmd
  times; the rerun below replaced it.
- The head and the gear are random draws (`PROBE models`).

VERIFIED, live (retail 1.1d, 1.5b and UO 1.51b, `probe_leanhit`
with `probe_bones`, `probe_fall`, `probe_pin` and `probe_stamp` set,
fixtures `mp_carentan-leanhit-cmds.txt`, `-cmds-1.5.txt` and
`-cmds-uo.txt`, each with both net probe clients' `CMDS` lines, reruns
with `probe_tapwait 1`, below):

- The drop. `probe_fall` logs each player's height off his spot for ten
  frames after each `setOrigin` (`PROBE fall`). He settles in one frame,
  and where rides the cmds: across four runs a server's target landed
  anywhere from 0.125 under the spot to 0.024 over it, 1.1d included.
  VERIFIED, vcod measurement: replaying each capture's own cmd times
  (`ProbeHold::times`), every fall row and bone of ours sits on retail's
  within the gate's 0.005 on all three servers, so 1.5b's fall is not the difference.
- The spawn. `PROBE spawnat` names the spawn point a player stood on at
  his first `setOrigin`, and the gate pins it (`Server::pin_spawns`): a
  sloped one leaves the spawn think's velocity on him through the drop
  (retail's shooter `trDelta` (1, 4, 0) off (976, -864)), and the drift it
  leaves moves with the spawn point's z in the last hundredth, so the
  probe logs the origin to six figures.
- `detachAll` and re-`attach`. `probe_pin` puts a fixed head and gear on
  the target (`basehead_axis_a_01`, the falshrm helmet, `load1`, the
  bandolier) and, after the level phase's grid, takes every attachment off
  and puts the same ones back on their tags, then runs the grid again in
  that frame (`level_re`). VERIFIED: on all three servers `level_re`
  reads cell for cell as `level`. VERIFIED, vcod measurement: so does
  ours', and with the pin every cell of all three captures matches.
- Bones a trace posed. `probe_stamp` crosses the target with one
  `bulletTrace` right after the second of the three `bones` rows, in the
  thread that logs them, which runs ahead of the anim update. VERIFIED: on
  all three servers every bone's third row, `tag_weapon_right` included,
  reads exactly the second, where the first two differ by up to 0.17. So
  the trace posed every bone at the last frame's time and the frame's
  entity pass, which moves the linked marks, read those poses (movers doc
  15). VERIFIED, vcod measurement: ours re-posed the marks' bones at the
  frame's own time (97 rows off); a script trace now stamps every bone of
  each body it poses (`GameHost::dobj_posed`), and they match.
- The rounds. The damage callback logs each round's direction
  (`PROBE dir`). VERIFIED, vcod measurement: with the aim block on the
  last frame's clock (15.1) and the hit-box clip (3.1), every round's part
  and point match on all three servers. The directions matched to 2e-5 but
  for up to 8e-5 (0.02 units at the target) on the shooter's left lean
  against the target's right on all three, every phase in which the target
  leans or lies on 1.5b and UO, and two more on UO. Each is which view the
  round read, the hold's exact angles or a cmd's quantized ones (15.1,
  "The cmd that fires"), and these runs tapped in the grid's frame.

VERIFIED, live (retail 1.1d, 1.5b and UO 1.51b, the same
recipe with `probe_tapwait 1`, ports 33251-33253, now the `-cmds`
fixtures): a second run of the `-press` recipe, which `leanhit_ab`
replays on the `PROBE_IN` lines rather than the logged inputs.

- 1.5b: every row matches, every round's direction within 2e-5. VERIFIED,
  vcod measurement.
- 1.1d: every row matches but the rounds' directions. VERIFIED, vcod
  measurement: all 13 left one 65536th of yaw (0.0055 degrees) left of
  ours. The shooter spawned at yaw 180 and the hold turned him to 0; the
  spawn's own think fed retail's sway a last view of 180 that ours never
  saw, and the turn off it decayed to -1 65536th by the cmd the scope froze
  the sway on (15.3, "The spawn's view"). VERIFIED, vcod measurement: with
  ours' spawn think running the block, 11 rounds leave within 2e-7 of
  retail's direction with no offset. `t_prone` (3.5e-5 low in z) and
  `t_right` (4.4e-5 in y, 3e-5 in z) stay off: ours read the hold's exact
  angles and retail a cmd's quantized ones. VERIFIED, vcod measurement:
  replayed with the shooter's cmd 18 ms before the trigger run after the
  frame (an 18 ms lead), both match and seven others move off.
  VERIFIED, live (the same run's `VEH ... ps` lines, which log each
  snapshot's `commandTime`): snapshot 27750 read `commandTime` 27716 and
  41150 read 41116, so the shooter's cmds 27732 and 41132 ran after those
  frames, and in the other eleven phases the cmd before the trigger ran
  ahead of the frame (24650 read 24632, its last cmd). VERIFIED, vcod
  measurement: the fixture's `CMDS` lines now carry those times (`ct=`);
  `leanhit_ab` runs each cmd between the `commandTime`s of the snapshots
  either side of it (`ProbeHold::ran`) and presses on the logged inputs,
  and all 13 rounds leave within 2e-5 of retail's direction. Each piece alone leaves the two rounds off: the
  `PROBE_IN` press holds the trigger on the whole batch, so cmd 27732
  fires, and the plain windows run it before the frame. A second 1.1d run
  (port 33371, `-cmds-ct`, `ct=` printed by the probe) has
  every cmd before a trigger ahead of the frame and replays the same way;
  its `ct=` matched its `VEH` lines on all 3269 snapshots.
- UO: every row matches, the shooter's `PROBE eye` included. He spawned
  standing on the sloped point (976, -792, -2.04889); VERIFIED, live: the
  target probe read his `trDelta` (6, 3, -1) on the spawn frame. VERIFIED,
  vcod measurement: with the spawn pin taking the placed spawnpoint's own
  float (unpinned, the think left (10, 5, -1)), ours leaves (6, 3, -1) and
  the eye rows match
  without an offset (cod15-movement.md 4, "The think on a slope,
  standing").

VERIFIED, live (retail 1.1d, 1.5b and UO 1.51b, the same
recipe, ports 33471-33476, both net probes logging `ct=`): the rerun that
replaced the 1.1d `-press` fixture and the 1.5b and UO `-cmds` and
`-press` ones. The box was loaded, and four runs lost the clients'
snapshots for 100-250 ms at a time (1.1d at 24650-24850, 26450-26650,
30900-31100 and 89050-89250, the same instants on the servers running
beside it).

- A stall. VERIFIED, live (1.1d `-press`, the shooter's lines): the
  trigger cmd 24650 went out on snapshot 24650; no snapshot reached him
  until 24850, which read `commandTime` 24617, and 24900 read 24866. So
  retail ran the cmds 24650 to 24866 after frame 24850, and the round's
  `PROBE hit` reads 24850, the frame's clock, 200 ms past the press, and
  points 0.08 degrees lower, the sway of that later clock (15.1).
- A cmd under the last. VERIFIED, live (the same run's `CMDS` steps,
  `d=...,1,-1` and `d=...,0`): the net probe stamps a cmd off its own
  clock and the next off a snapshot's time as it arrives, which can read
  a ms under the first or the same; the target's 29301 went out before
  his 29300, which carried the crouch. Snapshot 29300 read `commandTime`
  29301 and no `PMF_DUCKED`, 29350 had it: the server skipped 29300.
  VERIFIED, `cod_lnxded` 1.1d `SV_UserMove` (0x8086fa4), the per-cmd
  loop at 0x8087264: a cmd whose `serverTime` is above the message's last
  cmd's is skipped (`jg` at 0x808727b), so is one at or below
  `cl->lastUsercmd.serverTime` (`cl+0x10624`, `jle` at 0x8087286); the
  rest are copied over `lastUsercmd` (six words, 0x808729c) and, with
  the client at state 4, handed to the game's `ClientThink` (0x80872cb).
  VERIFIED, 1.5b (0x808cade, the loop at 0x808cd38): the same two
  compares (0x808cd75 `jle` to run, 0x808cd96 `jg` to run) ahead of the
  copy-and-think at 0x808ca58, which copies the cmd's six words to
  `cl+0x10624` (0x808ca64..0x808ca93) and tests state 4 (0x808ca9c).
- VERIFIED, vcod measurement: `ProbeHold::ran` takes a frame's
  `commandTime` off the newest snapshot logged at or before it, so the
  stalled frames send nothing and the frame after sends the held cmds,
  and `cmds_times` / `cmds_inputs` keep the send order and drop each cmd
  no newer than one before it. Without them the 1.1d `-press` replay
  fires at 24650 and puts the three crouch phases' bones a frame ahead (63
  bone rows off). With both, every row of all five new captures and of the
  earlier 1.1d ones matches.

VERIFIED, live (retail 1.5b, port 33621, `probe_bones 1` alone, both net probes logging `CMDS` with
`ct=`): the rerun that replaced `mp_carentan-leanhit-1.5.txt`. The target
stood 0.1132 over his spot after the drop (0.0096 in the cmd-time-free
replay), the third height this recipe has given on 1.5b. VERIFIED, vcod
measurement: replayed on `ProbeHold::ran` and the logged inputs, every
bone, the origin, every cell and every round match with no offset and no
cell gap; on the plain 50 ms windows every standing and crouched origin
row reads 0.1036 under retail's. So an older run's 0.0402 was that run's
split of the drop.

**The leaning eye's PVS.** VERIFIED, live (retail 1.1d, mp_carentan tdm, `client-probes/probe_leanpvs`, fixture
`crates/server/tests/fixtures/leanpvs/mp_carentan-leanpvs.txt`): an allies
watcher held at (1368, 2608, -23.875) facing 90 and an axis target at
(1080, 3488, -23.875), the watcher's lean driven in two-second phases
(level, right, level, left, right, level from level time 15000). Retail's
snapshots to the watcher carry the target throughout except from 21050,
a frame into the left lean, to 23350, 350 ms into the right lean that
follows. VERIFIED, vcod measurement, `leanpvs_ab` (same recipe, the net
probe's cmds): ours drops and restores the target on the same two
snapshots. Ours' snapshot eye there reads x 1362.74 leaned left, in
cluster 284, which does not see the target, and x 1368.03 level and
leaned right, in cluster 280, which does: a wall at +x stops the right
lean, the left one stops at 5.3 units, and the cluster boundary runs
between. So `SV_BuildClientSnapshot`'s eye takes the lean as clipped by
pmove (`ClientSim::eye_origin`, `spectate.rs`), on retail as on ours.

### 3.7 The angles a body stands in, and the build each trace makes

The evidence is `client-probes/probe_leanhit` under `probe_turn 1`: the
target stands unleaned 256 units from a sighted springfield, the hold
turns his view at each phase's start (pitch 0, 60, -85, 60, 0, then yaw
50 off facing the shooter, then pitch 45 with yaw -40, then back), and
1.5 s later the shooter's round is the first thing to pose him; the
grid of section 3.6 runs 0.3 s after the round and again a frame later.
Six captures, `crates/server/tests/fixtures/leanhit/`:
`mp_carentan-leanhit-turn{,-1.5,-uo}.txt` (the hold sets the turn every
frame) and `-turnonce{,-1.5,-uo}.txt` (`probe_turnonce 1`: for the
phase's first 0.2 s only), gated by `leanhit_ab`.

**The tilt.** VERIFIED, `game.mp.i386.so` (cod11-gametypes-re-bel.md
8.6): `SetClientViewAngle` stores the angles it is given, pitch and roll
included, into `r.currentAngles` (`ent+0x140..0x148`), and
`ClientThink_real`'s tail writes `(0, ps.viewangles[1], 0)` there after
every cmd. VERIFIED, UO `coduo_lnxded`: the locational clip against an
entity (0x0809a72a) calls game export 0xf, `G_DObjCalcPose` (`vmMain`'s
case 0xf, game.mp.uo 0x88f4f), only once the ray has met the entity's
box (the test at 0x0805e387 returns first), then traces the posed
bones. VERIFIED, the `-turn` captures, all three servers: every pitched
phase's grid reads a body tipped whole by the view's pitch about its
feet (forward under pitch 60, the head 47 units behind the feet and 41
up under -85), and the grid a frame later the upright body; the yaw
phases read the same either frame. INFERRED, off the two: the trace
places the DObj by `r.currentAngles`, so between a `setPlayerAngles` and
the client's next cmd every trace meets the body turned by the angles
the script gave. The hold's thread runs ahead of the grid's in that
frame's thread pass, so the grid reads the tilt; a frame later the
target's cmds ran first.

**Which packet ran first.** VERIFIED, the `-turn` captures' `CMDS`
lines: every round's cmd carries the time of one of the target's cmds
(both answer the same snapshot), so whether the round met the tilt rides
which of the two packets the server ran first. The stamps cannot say, and
neither can the probes' send clocks (`w=`): two sends answering one
snapshot land within microseconds. The `-turn` captures in the tree (all
three 2026-11-13; the 2026-10-19 runs are in git at 55164194) log the
server's own read order: `showpackets 1` prints each client packet as
`Netchan_Process` takes it, ahead of the message's cmds
(docs/dev/retail.md, "Receive order", which has the addresses and the
counts). Each pair's order there decided the round:

- VERIFIED, vcod measurement: `leanhit_ab` replays each run's cmds one a
  packet in the server's read order (`common::cmds_read`), and every
  round's point and direction of all three runs match retail's.
- VERIFIED, vcod measurement: sent instead as one packet per client a
  frame, the shooter's first, the rounds of `t_down`, `t_down2` and
  `t_yaw_down` (1.1d), `t_down`, `t_down2`, `t_up` and `t_yaw_down` (1.5b)
  and `t_down2` and `t_up` (UO) miss the body on ours.

VERIFIED, `game.mp.i386.so`'s entity field table: the `angles` field is
`ent+0x140`, `r.currentAngles`, with the setter `Scr_SetAngles` (0x5c660),
which hands the script's vector to `G_SetAngle` (0x67d9c). VERIFIED,
`G_SetAngle`: it stores the three into `s.apos.trBase` (`+0x3c..0x44`),
zeroes the trajectory's delta and stores them into `r.currentAngles`
(`+0x140..0x148`). INFERRED: a script write of a player's `angles` turns
the traced body until his next cmd as `setPlayerAngles` does.

vcod: a CoD1 player's `r.currentAngles` live in his
`combat::DobjStamps`, which his script `angles` field reads and writes
(`GameHost::get_field`, `write_field`): the cmd's `set_client_yaw`, the
spawn, `setPlayerAngles`, a turret mount and a script write all land
there, and every trace and tag poses the body in `DobjStamps::frame` (all
three angles; `HitBody` carries no turn of its own). VERIFIED,
vcod measurement: every grid cell of the six captures sits within 0.0001
median of retail's, and with the read order every round of the `-turn`
captures matches, with no gap.

**One ease a build.** VERIFIED, game.mp.uo (coduo-vehicles.md 13, "The
glide back"): each skeleton build that finds a bone unbuilt under the
current stamp runs the controller hook, which steps every control 18
degrees toward its target from where the last build left it, and
`ClientEndFrame` only installs the hook. VERIFIED, `cod_lnxded`
(cod11-movers.md 15): the stamp is bumped ahead of each client packet,
ahead of the bots' cmds and ahead of `G_RunFrame`. INFERRED, off the
two: a body nothing traces keeps its controls where its last build left
them, however long, and a round in a packet is a build of its own.
VERIFIED, vcod measurement against the `-turnonce` captures (1.5b and
UO; the round meets the upright body): with the ease on every build and
the packet's stamp, every round and every grid cell matches; eased once an
end frame and the round posed without a build, the `t_down2` round lands on the head 12.2 units nearer the
shooter than retail's (UO: 1235.94 against 1248.12 in x) and the `t_up`
round 2.2 units off; with the round's build posing but not easing, every
round misses retail's by 2 to 20 units. 1.1d's controls are set
outright (its rounds read the settled turn, `-turnonce`, all rows).

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
   zero by an explicit `fldcw` of `0xC00`. This runs once for every means of
   death that reaches `G_Damage`, melee included: the product is taken at
   x87 width, not `f32` width (turrets doc 12.6's 53 from `60 * 0.9`).
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

`Scr_PlayerKilled` (`0x5cb30`) pushes its attacker and inflictor the same
way. VERIFIED: both functions compare the pointer against 0 and call
`Scr_AddUndefined` on the null arm instead of substituting an entity
(`0x5cae5` and `0x5cb01` in `Scr_PlayerDamage`, `0x5cbd4` in
`Scr_PlayerKilled`). INFERRED, off those null arms: a death whose caller
passed no attacker reaches both callbacks with `eAttacker` undefined, and
nothing on this path turns it into `g_entities[ENTITYNUM_WORLD]`. The
`radiusDamage` builtin is not such a caller: it passes the world entity
itself (14.2), so a mine or a bomb reaches the callbacks with the world as
`eAttacker` and only `eInflictor` undefined.

That holds for `Scr_PlayerDamage` only. A death goes through
`finishPlayerDamage`, which starts both its inflictor and attacker at
`g_entities + 0xc49d8` (`0x43778`-`0x43782`, VERIFIED) and replaces each only
on an entity argument. VERIFIED, `cod11-player-clip.md` 8.10: a fall, whose
damage callback reads both undefined, reaches the killed callback with
`ent1022` for both. VERIFIED: the inflictor's replacement at `0x43827` is
`Scr_GetEntity(1)`, the attacker's argument, behind a type test of argument
0. VERIFIED: the inflictor slot (`ebp-0x3C`) is read at one place after
that, the push into the `die` call (`0x43c97`). INFERRED: an entity
inflictor, a grenade's, reaches the killed callback as the attacker, and the
damage callback, which `G_Damage` calls with its own arguments, is
untouched.

VERIFIED, `client-probes/probe_victims` on retail, calling
`finishPlayerDamage` directly with a lethal 500: inflictor a `script_origin`
(179) and attacker slot 1 reached the killed callback as `inflictor 1
attacker 1`; inflictor slot 2 and attacker the `script_origin` as `179 179`;
inflictor `undefined` and attacker slot 1 as `1022 1`. vcod hands the killed
callback the attacker for an entity inflictor and the world for any other
(`finish_player_damage`, `crates/server/src/game/builtins/combat.rs`), and
the same probe against `vcod-server` read the same three rows. What retail
does with an entity inflictor and no attacker entity is not read; vcod hands
over the world.

What makes that safe for the stock scripts is `isPlayer`, `functions[81]` at
`0x5efd4`. VERIFIED: it calls `Scr_GetType(0)` and branches to `Scr_AddInt(0)`
for anything but type 7, then `Scr_GetPointerType(0)` and branches the same
way for anything but 0x0d, and only then calls `Scr_GetEntity(0)` and tests
the entity's `+0x158` -- the `gclient` pointer -- returning 1 when it is
non-null and 0 when it is not. INFERRED, from those branches: `isPlayer` of
an undefined, a string or a number is 0 rather than an error, which is what
lets `dm.gsc`'s `Callback_PlayerKilled` call `isPlayer(attacker)` unguarded
at line 492 on every world death.

INFERRED, and a vcod divergence worth knowing: retail decides player-ness by
that `gclient` pointer, where `crates/server/src/game/builtins/entity.rs`
decides it by the entity number being below `MAX_CLIENTS`. The two agree on
every entity a stock map produces, since numbers 0..63 are client slots by
construction, and they would disagree only on a non-client entity numbered
into that range, which nothing allocates.

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

The rounding. VERIFIED, 1.1d: `vDir` goes through `VectorNormalize2`
(called at 0x43922, body 0x3da68): the squared length and its root on the
x87 stack, the length stored as a float, each component times the
unrounded reciprocal and stored. The scale stays on the stack; the y and z
products are stored before their adds, the x product is added unrounded
(0x439e7..0x439fb). VERIFIED, 1.5b (called at 0x3ee76, body 0x33216) and UO
(0x47634, body 0x3a466): `VectorNormalize2` stores the squared length, the
root (`sqrt` in double), the reciprocal and each component, and the
knockback stores each product and each sum. VERIFIED, vcod measurement:
normalized with `Vec3::normalize_or_zero` (float products and sums) the
1.1d `-hitgrid` target's velocity after the wound and the kill read one
float step off retail's in each component; with the two builds' rounding
every `turret_ab` capture's target velocity is retail's to the bit on all
three servers.

**As implemented.** `finishPlayerDamage` normalizes `vDir` with
`x87::normalize` on 1.1 and `x87::normalize2_stored` on 1.5 and UO;
`ClientSim::take_damage` (`crates/server/src/spectate.rs`) adds the velocity
and starts the timer; what the timer does in pmove is 16.1's last
paragraph.

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
client-flavoured one to the victim alone. The engine's side of that, which
compares `r.singleClient` with the snapshot's `ps.clientNum` rather than the
slot, and the retail run that measured both halves, are
`docs/research/cod11-events-and-fx.md` section 2.

VERIFIED: the attacker pointer the two `otherEntityNum` writes read
(`ebp-0x40`) starts as `g_entities + 0xc49d8` (`0x43778`), entity 1022 at
the gentity size `0x314`, and is replaced only by `Scr_GetEntity` at
`0x43864`. INFERRED: a call with no attacker entity names the world.

INFERRED, from the weapon test and 4.2's step 5: the pair follows the weapon
the callback was handed, not the means of death, so a melee hit with a rifle
raises it beside `EV_MELEE_HIT`, a turret round raises it off the gunner's
weapon, and a grenade's blast, whose inflictor carries the frag, raises
none. VERIFIED live for melee (events-and-fx doc section 2): each
landed swing put a 174 with `surfType` 7 in the swinger's snapshot and a 176
in the victim's. INFERRED from the callback being the only site: a hit the
gametype's `Callback_PlayerDamage` refuses before calling
`finishPlayerDamage`, a teammate's with friendly fire off, raises no impact
at all, since `Bullet_Fire_Extended` raises none on a client (2.4 step 2).

**As implemented.** `finish_player_damage`
(`crates/server/src/game/builtins/combat.rs`) raises the pair through
`combat::flesh_impacts` on a `weaponType bullet` weapon, and `bullet_fire`
raises no impact on a player.
`self->flags & 1` is not modelled.

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
   Section 11.3 reads that arithmetic out in full (not isotropic). Whether
   that origin is snapped is 5.5.
6. `BG_AnimScriptEvent(client, 1, 0, 1)`.
7. `G_AddEvent(self, 0xBD, 0)`. VERIFIED: `0xBD` is 189, `EV_DEATH`, and its
   event parm is the literal 0.
8. `Scr_PlayerKilled(self, inflictor, attacker, damage, mod, weapon, dir,
   hitLoc)`.
9. A walk over `level.maxclients` calling `Cmd_Score_f` for every connected
   client whose `sess.sessionState` is 2 and whose `spectatorClient`
   (`client+0x21D4`) is this entity's number. VERIFIED: the walk's stride over
   the client array is `0x22C4`, which is `sizeof(gclient_t)`.
   `cod11-spectator-follow.md` 4 has the compares' addresses and the retail
   run that measured the push.
10. `self->angles[2] = 0`; `self->takedamage = 1`; `self+0x118 = 0x4000000`.
    INFERRED: `self+0x118` is the entity's contents mask and `0x4000000` is
    the corpse contents, on the strength of `player_die` writing it between an
    unlink and a link and of Quake III Arena's `CONTENTS_CORPSE` having the
    same value.
11. `client->ps.stats[1]` (`ps+0xF8`) takes `(int)vectoyaw(attacker->origin -
    self->origin)`, or `(int)vectoyaw(inflictor->origin - self->origin)` when
    there is no usable attacker, or `(int)self->angles[1]` when there is
    neither. VERIFIED: the three stores. VERIFIED, 0x49c2c..0x49ca3: an
    attacker is usable when it is non-null and not `self`, an inflictor the
    same; `origin` is `r.currentOrigin` (`+0x134`) and `angles[1]`
    `r.currentAngles[1]` (`+0x144`), each truncated by a `fistp` under `|
    0xc00`. VERIFIED: the function called is `vectoyaw` (0x3dccc), not
    `vectoangles`: 0 for a vector with no horizontal part, else the
    `fpatan` stored as a double, times 180 and over pi in double (`.rodata
    0x72a88`, 0x72a90), stored as a float, and 360 (0x72a98) added on the
    stack when it is below 0. INFERRED: reading `stats[1]` as the dead
    yaw, which is the label `docs/protocol-1.1.md` carries for it; the
    store is the strongest evidence there is for that reading and is still
    a reading. INFERRED, with 4.4's inflictor rule: a grenade's own thrower
    (attacker and entity inflictor both `self`) reads his own yaw, a death
    whose attacker is the world faces the world's `r.currentOrigin`, which
    no spawn writes, so the map's origin, and `Cmd_Kill_f`, which passes
    `self` three times (0x49608..0x4960a), reads his own yaw.
    vcod: `builtins::combat::dead_yaw` on the entities, through
    `builtins::math::vectoyaw` (`a_death_faces_what_player_die_reads`).
12. `client->ps.viewangles` takes `self->angles`. INFERRED: those are
    (0, view yaw, 0), which `ClientThink_real`'s tail writes after every
    cmd (`cod11-gametypes-re-bel.md` 8.6), so the corpse's view loses its
    pitch. VERIFIED on UO (`player_die` 0x5f972, the same stores, and
    `coduo-vehicles.md` 11.7's capture, 24.999 to 0). vcod:
    `ClientSim::die`.
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

**The two suicides.** `Cmd_Kill_f` (`0x495c8`) and the `suicide` player
method (`0x45358`, entry 26 of `player_methods` at `0x733dc`, per
`tools/re/dump_builtins.py`) make the same `player_die` call.

- VERIFIED: `Cmd_Kill_f` compares `client->sess.sessionState`
  (`client+0x20D0`) against 0 at `0x495d7`. INFERRED: its `jne` to the
  epilogue means only a playing client can `kill`.
- VERIFIED: both clear bit 0 of `self+0x17C` (`0x495e0`, `0x453ba`) and store
  0 to `self+0x230` (`0x495e7`, `0x453c1`) and to `client+0xF4` (`0x495f1`,
  `0x453cb`). INFERRED: those are Quake III's `flags &= ~FL_GODMODE`,
  `health` and `ps.stats[STAT_HEALTH]`, written 0 where Quake III writes -999.
- VERIFIED: the call pushes `self` three times, `0x186A0` (100000), `0x16`
  and three zeros (`0x495fb`..`0x4960b`, `0x453d5`..`0x453e5`). INFERRED,
  off the argument names above: inflictor and attacker are the player,
  damage 100000, `mod` 22 (`MOD_SUICIDE`, 4.1), and weapon, `dir` and
  `hitLoc` all 0.
- VERIFIED: `player_die` tests the weapon argument at `0x49a89`. INFERRED:
  a weapon of 0 skips step 3's mounted-gun substitution.
- VERIFIED: `Scr_PlayerKilled` pushes `hitLoc` through
  `G_GetHitLocationString` (`0x5cb4c`), tests `dir` against 0 (`0x5cb5d`)
  with `Scr_AddUndefined` on one arm (`0x5cb70`), and pushes
  `BG_GetInfoForWeapon(weapon)->name` (`0x5cb7f`, `0x5cb84`). It ends in
  `Scr_ExecEntThread(self, g_scr_data+0x1C, 7)` (`0x5cbf6`). INFERRED: a
  suicide's `vDir` is undefined, the callback takes seven arguments, and
  there is no `psOffsetTime`.
- VERIFIED: the weapon loader behind `BG_SetupWeaponInfo`'s call at
  `0x368e2` (`0x35adc`) allocates a `0x41C`-byte def (`0x35b55`), stores it
  as `weaponDefs[0]` (`0x35b61`), and hands `def+4` and the `.rodata`
  string `"none"` at `0x70f6b` to `0x35a54` (`0x35bb5`). INFERRED: `0x35a54`
  copies the string in, so weapon 0's name is `"none"`.
- VERIFIED, run: retail logs both the same way. `probe_blastbody`'s
  `suicide()` on an sd player logged
  `K;0;allies;vcod;0;allies;vcod;none;100000;MOD_SUICIDE;none`
  (`crates/gsc/tests/fixtures/semantics/client-probes/README.md`,
  `probe_bomb and probe_blastbody`). The `probe_glass` run, whose
  `--probe-target` sends `kill` every 45 s, logged
  `K;0;;vcod;0;;vcod;none;100000;MOD_SUICIDE;none` at 0:22 and 1:07.

**A suicide that stays playing.** `player_die` writes no `pm_type` (step
list above) and `ClientEndFrame` writes 6 only off `sessionState == 1`
(0x41057..0x41079), with the contents, `takedamage` and `SVF_NOCLIENT`
beside it (5.4 below). INFERRED, off those stores: a killed callback that
leaves `sessionstate` at `playing` leaves a body at health 0 that the end
frame treats as alive. VERIFIED live, `client-probes/probe_suicide` on
retail mp_harbor (recipe in that directory's README): after
`suicide()` slot 1's own `pm_type` stayed 0 and its `viewHeightCurrent` 60,
and slot 0's snapshot kept carrying slot 1, `eFlags` unchanged, for the
3 s the session stayed `playing`; the frame the script wrote `dead`,
slot 1 read `pm_type` 6, its eye started down (51.18, 42, 33, ... 8), and
it left slot 0's snapshot. The `probe_compass` run saw the same
`pm_type` 0 then 6 (`cod11-hud-protocol.md` 9). INFERRED, UO's
`ClientEndFrame` (`game.mp.uo.i386.so`, Ghidra output): the same compares
of `sessionState` against 3, 2 and 1 pick the arm, and only the 1 arm
stores 6 (7 linked) with the same `svFlags` and `takedamage` writes, so UO
keeps a playing suicide at `pm_type` 0 too; not run live there.

vcod: the end frame keys the dead arm on the session, not the health
(`ClientSim::session`, set in the end-frame loop;
`a_suicide_that_stays_playing_keeps_pm_type_0`,
`crates/server/tests/suicide_playing.rs`): only
`playing` keeps the body on the wire with contents, `dead` alone writes
`pm_type` 6, and a sim no script client backs keeps the health rule. The
death's own relink as a corpse (step 13) clears `solid` until the body's
next link, so the suicide's frame goes out with `solid` 0 (INFERRED, off
step 13; the probe logged no `solid`). VERIFIED, ours after
the fix, the same probe pair against `vcod-server --gametype-script`:
`pm_type` 0 and slot 1 in slot 0's snapshot until the `dead` write, then
6 and gone on the same frame.

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

Measured, 1.1d `mp_carentan` tdm, a lone `--probe-target` that
sent `kill`: the clone (entity 64) first rode the snapshot at `serverTime`
396050 with `eFlags` `0x818` and read `0x18` from 396250 on, 200 ms later.
VERIFIED (capture). INFERRED: the `kill` ran inside a cmd, before the frame,
so the clone took the last frame's `level.time` (396000) and the think came
due at 396250, the first frame at or past `level.time + 250`. The `0x818`
also shows the clone's `eFlags` carry neither `EF_DEAD` (`0x1`) nor
`EF_ADS` (`0x200`): the table's source is `ps->eFlags`, and those two are
the bits `BG_PlayerStateToEntityState` adds to the entity's word only.
VERIFIED (capture).
The same probe against vcod-server: the clone appeared at 19200 and lost
`0x800` at 19400, the same 200 ms, with no `EF_DEAD` copied in. VERIFIED
(capture).

A death in the entity pass, `client-probes/probe_corpseblast` on
mp_carentan dm, one client on 1 health dropping a frag at his own feet
(`--probe-uo-weapon throw`; fixtures
`crates/server/tests/fixtures/dyingpose/mp_carentan-dm-corpseblast*.txt`
and the `net/*-corpseblast*.bin` pairs). VERIFIED (capture), on all three
retail servers: the killed callback (`MOD_GRENADE_SPLASH`) logged
`getTime()` 16250 on 1.1d, 16100 on 1.5b and 17200 on UO 1.51b, the corpse
at 64 first rode the snapshot of that same serverTime as `0x818`, and lost
`0x800` 250 ms later (16500, 16350, 17450). INFERRED: the blast runs in
`G_RunFrame`'s entity pass on this frame's `level.time`, so the think comes
due 250 ms after the first snapshot that carries the clone, where a
cmd-path death's comes due after 200. Ours matches on all three
(`dyingpose_ab::a_blast_corpse_keeps_its_fresh_flag_like_{retail,15,uo}`).

INFERRED: nothing frees a clone on a timer. It lives until the eighth
subsequent death reuses its slot, which is the eight-body behaviour the
project already observed live (`AGENTS.md`, "Netcode debugging").

**As implemented** (`crates/server/src/game/bodies.rs` and the `cloneplayer`
builtin in `crates/server/src/game/builtins/client.rs`). The clone is written
onto an empty entity state rather than a copy of the player's, and it carries
the table's `eType`, `clientNum`, `legsAnim`, `torsoAnim`, `eFlags` (the
entity's word less `EF_DEAD` and `EF_ADS`, which is `ps->eFlags`) with the
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
`Server::snapshots_begin` re-reads a body born this frame from that client's
sim once, at the snapshot build, after the frame's sim ops have been applied.
A body born on an earlier frame is never re-read.

**The corpse as a script entity.** VERIFIED, `game.mp.i386.so`:
`cloneplayer` (0x4450c) ends in `GScr_AddEntity(body)` (0x446d1), so the
method returns the corpse. `G_SpawnPlayerClone` (0x67888) names it through
`scr_const+0x56` (0x678e6), the constant `G_InitGentity` (0x6783c) writes,
which vcod reads as `noclass`. Nothing on the clone path writes
`s.eventSequence` or `s.events`, and `G_FreeEntity` zeroes a reused slot,
so a corpse is born with an empty ring. `G_PlaySoundAlias` (0x67b30)
appends `EV_SOUND_ALIAS` (172) to `s.events` of any entity with no client,
a corpse included. VERIFIED (capture), retail 1.1d mp_carentan, `client-probes/probe_dyingpose`, whose killed callback runs
`body = self cloneplayer(); body playsound("MP_bomb_plant");`: the killed
line named entity 64, and the client's first snapshot holding it
(`crates/common/tests/fixtures/net/snapshots-dyingpose.bin`, 11500) carried
`eventSequence` 1, events `[172, 0, 0, 0]`, parms `[4, 0, 0, 0]`, `eFlags`
0x818 and `apos` yaw 90. A client that first sees a fresh corpse replays its
ring from 0 (`cod11-events-and-fx.md` 1, "An entity enters the snapshot"),
so such a sound plays for a spectator who walks into view within the
250 ms. VERIFIED (capture): the stock 1.1 and UO captures' corpses all carry
`eventSequence` 0, and no stock gametype touches the body it clones. VERIFIED
(capture), the same probe on 1.5b and UO 1.51b
(`net/snapshots-dyingpose-1.5.bin`, `-uo.bin`): the first snapshot of 64
carried `eventSequence` 1, `eFlags` 0x818 and yaw 90 on both, its ring
`[172, 0, 0, 0]` parm 4 on 1.5b and `[177, 0, 0, 0]` parm 70 on UO, in
the wire's own numbering.

Ours: `cloneplayer` returns `ObjectTable::spawn_body`'s entity at the body's
number; the snapshot build writes that entity's ring into the queue's state
(`ScriptRuntime::body_entities`), `playSound` on it broadcasts it as on any
entity, and a `delete` frees the queue's slot (`BodyQueue::free`).
`dyingpose_ab::a_sound_played_on_a_corpse_rides_its_ring` compares the ring,
`eFlags` and yaw with the capture's.

**As implemented**, `dropItem` (same file). VERIFIED that retail's
`PlayerCmd_dropItem` (`.so` 0x43684) resolves the name through
`BG_GetWeaponIndexForName` and calls `Drop_Weapon` (0x4dd40), which takes the
weapon off the player with `BG_TakePlayerWeapon` and hands the entity to
`LaunchItem` (0x4db98). VERIFIED that `LaunchItem` stores `+0x4 = 3`
(`ET_ITEM`), ORs 0x10 into the entity state's `eFlags` byte at 0x4dc93 (the
`eFlags` 16 the placed-weapon traces carry), stores `pos.trType` 5 and
`pos.trTime = level.time`, and arms `DroppedItemClearOwner` at `level.time +
1000`. VERIFIED that `LaunchItem` stores 0x10 into `+0x17c` at 0x4dd24 and
that `Touch_Item` tests that bit at 0x4d9ad and 0x4da0e; `+0x17c` is not the
wire `eFlags`. INFERRED, from CoDExtended's `gentity_t` layout, that `+0x17c`
is the gentity `flags` field and 0x10 its dropped-item bit. VERIFIED that
`DroppedItemClearOwner` (0x4efb4) only writes 0x3fe into `s.clientNum`, and
that the two `0x7530` immediates in `.text` sit in `Cmd_CallVote_f` and
`fire_rocket`. INFERRED from `GetFreeCueSpot` (0x4da44) and `G_RunItem`
(0x4eb18): a drop lives until it is picked up, until a drop made while
all 32 ring slots are held evicts it from slot 0, which in play is the
slot evicted every time, or until it lands in `CONTENTS_NODROP`
(`docs/research/cod11-items.md`, section 8).

What vcod does with a drop is in `docs/research/cod11-items.md`, section 13.

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

Live (mp_carentan tdm, one probe shooting a teammate with
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

As implemented: `Server::snapshots_begin` builds no entity for a client whose
sim is dead, the same filter that already drops spectators. The corpse's
newborn re-read (5.2) takes its state straight off the sim, since the dead
client has no entity in that list any more. Before this, a retail client was
sent a dead player standing in its death pose under the corpse until the
respawn, and `--probe-sweep`'s live-target gate saw a target to shoot for the
whole dead wait.

The flag is set by the dead player's own end frame, so a player killed after
it, by a higher slot's turret round in that slot's turn, is still sent on the
kill snapshot, as its end frame copied it: VERIFIED, the `-victim` turret
captures (`cod11-turrets.md` 13.3), whose victim carries the pain of the
round before, the anim and trajectory its last cmds left and `solid` 0
(step 13's relink as a corpse). vcod builds every client's entity from the
copy its own end-frame turn made (`ClientSim::end_frame_entity`) and sends a
client killed after that turn on it, `solid` cleared.

### 5.5 The origin a death drops from

Both things a death drops leave from `r.currentOrigin` (`ent+0x134`), and
whether that origin is snapped depends on where the death runs.

VERIFIED, `player_die` (`0x49a48`): the live grenade's origin loads
`ent+0x134`, `+0x138` and `+0x13c` (`0x49b3e`, `0x49b47`, `0x49b50`) and adds
40.0 (`.rodata 0x743ec`) to z (`0x49b56`) before the `fire_grenade` call
(`0x49b6f`).

VERIFIED, `Drop_Weapon` (`0x4dd40`): the `LaunchItem` origin is `ent+0x134`
and `+0x138` (`0x4de7b`, `0x4de84`) and `ent+0x13c` plus
`(ent+0x114 - ent+0x108) * 0.5` (`0x4de8d..0x4de9f`, `.rodata 0x74d5c`). The
tag block's trace starts at `ent+0x134..0x13c` plus the box centre
`(mins + maxs) * 0.5` (`0x4e0ce..0x4e11a`). VERIFIED,
`G_DObjGetWorldTagMatrix` (`0x6746c`): it builds the tag's world matrix on
`AnglesToAxis(ent+0x140)` (`0x67502`) and `ent+0x134..0x13c`
(`0x67507..0x6751f`). INFERRED: every point the weapon drop is launched from
hangs off `r.currentOrigin`, the hand tag included.

VERIFIED, `maps/MP/gametypes/tdm.gsc` (`pak5.pk3`): `Callback_PlayerKilled`
runs from line 549 to its `self dropItem(self getcurrentweapon());` at 626
with no `wait` between. INFERRED: the weapon drop runs inside `player_die`'s
`Scr_PlayerKilled` call (5.1 step 8), so both drops read `r.currentOrigin` at
the same moment.

VERIFIED, `ClientThink_real` (`0x3fee0`): the snapped `s.pos.trBase` goes
into `r.currentOrigin` at `0x4051e..0x40533` (2.1), `ClientEvents` is called
at `0x40589`, `G_TouchTriggers` at `0x405b3`, `ps.origin` is written back at
`0x405c7..0x405dc`, and `Cmd_Activate_f` is called at `0x4064e`. VERIFIED:
`ClientEvents` (`0x3fd24`) calls `G_Damage` at `0x3fe0d` with 0x15 among the
pushed arguments (`0x3fde3`). INFERRED: 0x15 is the means of death, 21,
`MOD_FALLING` (4.1), and that call is the fall damage
(`cod11-player-clip.md` 8.8, measured in 8.10). VERIFIED:
`G_RunClient` (`0x40660`) writes `ent+0x134` only through `G_SetOrigin` on its
linked arm (`0x406e9`), and `ClientEndFrame` (`0x40e98`) has no store of its
own to `ent+0x134..0x13c`. INFERRED: between cmds a player's
`r.currentOrigin` is the unsnapped `ps.origin` its last cmd left, short of a
spawn, a script `setOrigin` or a link moving it, and only a death inside the
victim's own `ClientThink_real`, between `0x40533` and `0x405c7`, sees the
snapped one.

The death paths that follow from that:

- INFERRED, unsnapped: the `kill` command (`Cmd_Kill_f`, a client command,
  outside `ClientThink_real`); a bullet or melee hit, which runs in the
  shooter's cmd (16.1); a grenade blast, run from `G_RunFrame`'s missile pass
  (11.4); a turret round; a script's `suicide()` or `radiusDamage`.
- INFERRED, snapped: a `trigger_hurt`, whose touch runs from the victim's own
  `G_TouchTriggers`; a fall, `ClientEvents`' `MOD_FALLING` damage.

VERIFIED, `G_TouchTriggers` (`0x3f88c`): the `trap_EntitiesInBox` box is
`client+0x14..0x1c`, `ps.origin`, plus and minus the range at `.rodata
0x7dcdc` (`0x3f8b3..0x3f912`), and the box handed to the contact test is
`ps.origin` plus `ent+0x100..0x114`, the entity's mins and maxs
(`0x3f939..0x3f98e`). INFERRED: the touch is taken at the unsnapped origin
even while `r.currentOrigin` is snapped.

VERIFIED, `mp_carentan-tdm-grenade-death-shooter.txt` (a `kill` 500 ms into
the cook): the dying player's traces read `origin=1216.9,1296.0,-7.9`, and
the missile on the death frame reads `trBase` (1216.9, 1296.0, 32.1), where
a snapped origin would put it at (1216, 1296, 33). No capture holds a
`trigger_hurt` or falling death with a drop, or the origin of any death's
weapon drop.

**As implemented.** A client's `origin` field on the host is the
`r.currentOrigin` mirror. `Server::replay_moves` writes the mover's
truncated origin into it before the cmd's shots and its trigger pass, and
writes `ps.origin` back after the trigger pass, ahead of the item pass and
the use key; `ScriptRuntime::touch_triggers_at` and `trigger::touched` take
`ps.origin` for the touch. The same write is what a killing shot's dead yaw
now reads of the shooter: VERIFIED, `player_die`'s two `vectoyaw` arms
subtract `self+0x134..0x13c` from `ent+0x134..0x13c` of the inflictor or
attacker (`0x49c40..0x49c5e`, `0x49c74..0x49c98`). INFERRED: 5.1 step 11
takes the attacker's snapped origin when the attacker is inside its own cmd.
Both drops read the `origin` field (`drop_cooking_grenade` in
`crates/server/src/game/builtins/combat.rs`, and `item::launch_weapon`; the
mirrored entity state's `trBase` is `ps.origin`, unsnapped on the
`trigger_hurt` path). Pinned by `a_player_killed_mid_cook_drops_a_live_grenade`
(`crates/server/tests/combat.rs`), which stands the player off the unit grid
and holds the `kill` drop to the unsnapped origin plus 40, and
`a_trigger_hurt_death_drops_and_clones_at_the_snapped_origin`
(`crates/server/src/server.rs`), which holds a `trigger_hurt` death's grenade,
weapon and corpse to the truncated one. `cloneplayer` places the body at the
`origin` field, as retail's `G_SetOrigin(body, self->r.currentOrigin)` (5.2)
does, so a `trigger_hurt` corpse lies at the truncated origin and a bullet's
at the unsnapped one. Fall damage (`fall_damage`, `deliver_fall` in
`crates/server/src/server/cod1_think.rs`) and the weapon drop's tag sweep
(`item::launch_weapon`, `cod11-items.md` 11) are modelled; no capture checks
either against a snapped origin.

### 5.6 A spawn leaves `health` alone

VERIFIED, `game.mp.i386.so` `ClientSpawn` (`0x4268c`, annotated with
`tools/re/annotate_func.py`): the function holds no store to `gentity+0x230`
(`health`). Its stores to the entity are `takedamage` 0 (`0x4273c`) and the
`die` pointer, `player_die` (`0x42753`). It `__bzero`s the whole `gclient_t`
(`0x42804`, `0x22C4` bytes), copies `sess` back, and stores
`sess.maxHealth` (`client+0x2150`) into `ps.stats[2]` (`0x42849..0x4284f`).
INFERRED: a spawn in any of the three modes keeps whatever health the entity
held, and only the gametype's own `self.health = self.maxhealth` (`sd.gsc`'s
`spawnPlayer`) refills it; `spawnSpectator` writes none.

VERIFIED, `client-probes/probe_victims` on retail: a playing
client given `health` 37 read 37 after `sd::spawnSpectator()` and 37 again
half a second later; given 41 and spawned with `sessionstate` `"playing"`
through a bare `self spawn(origin, angles)`, it read 41 with `maxhealth` 100,
and 41 half a second later. `probe_blastloop`'s dead slot read -20 as a
spectator (14.5's rows).

**As implemented.** The `spawn` player method
(`crates/server/src/game/builtins/entity.rs`) writes no health; it clears
the host's `dead` mark, `player_die`'s `die` pointer coming back. The same
probe against `vcod-server` read 37, 37, 41 and 41.

### 5.7 A corpse's slide: `PM_DeadMove`

VERIFIED, `PmoveSingle` (`0x33dfc`): the `pm_type - 1` jump table at
`.rodata 0x70ce8` sends 6 to `0x34274`, the default arm, and 7 to `0x34220`,
the arm a linked live player takes. On the default arm `0x342e9` compares
`pm_type` against 6 and calls `0x2f700` when it is equal, after the first
ground trace (`0x342ce`) and ahead of the walk or air move. VERIFIED,
`0x2f700`: it returns at once when `pml+0x2C` is zero; otherwise it takes
the length of `ps->velocity` (`ps+0x20..0x28`, all three axes), subtracts
`20.0` (`.rodata 0x7090c`), stores zero to all three components when the
result is at or below 0 (`0x2f744..0x2f75c`), and otherwise scales the
normalized velocity by it. INFERRED: this is Quake III Arena's
`PM_DeadMove`: `pml+0x2C` is `pml.walking`, the same word the move dispatch
at `0x34312` picks the walk move by, and the drop is per frame, not per
second, so a corpse slides less the faster its client sends cmds.

VERIFIED, `client-probes/probe_victims` on retail: a flat 200
`radiusDamage` killed 100-health slot 0 at `(-226, 2424, -31.79)` and took
200 off 1000-health slot 1 at `(-269, 2381, -31.99)` behind it, on the same
line from the blast at `(-176.8, 2473.1, 7)`; both took the same 60-point
knockback (4.5). Per server frame after the blast, slot 0 read x -226.00,
-231.11, -238.91, -242.41, -243.68 and held there, 24.9 units of slide; slot
1 kept moving to frame 9 and held at `(-311.49, 2338.51)`, 60.1 units.
INFERRED: the difference between the two is the 20 a frame.

**As implemented.** `vcod_common::pmove::dead_move` calls `dead_friction`
after its first ground trace. The same probe against `vcod-server`, both
before the change and after: the corpse slid 56.4 and 21.8 units, the live
player 58.9 and 54.7. The probe clients send a cmd every 16 ms plus their
own loop time on either server, so the cmd count is the client's; the
residual 3 units are within what one run of each measures, which this pair
of runs does not settle. `pm_type` 7, a dead player still
linked, takes `0x34220` and does not move at all: vcod runs
`dead_linked_move` for it (`cod11-player-clip.md` 1.1 has the capture).

Where a corpse comes to rest also depends on its client's cmd rate.
VERIFIED, `client-probes/probe_pronecrouch` on 1.1d, 1.5b and UO
(`tests/fixtures/pronecrouch/`): a player held by `setOrigin`
at z -22.75 over the ground at (844 3480) is killed on the hold's last
frame. Under the net probe's 60 Hz cmds his body lands at -23.97739 and
then sinks about 0.0016 every few hundred ms, down to -23.9926 within
4.4 s. A live player let go there lands at -23.874989. Under one cmd every
50 ms (`--probe-cmd-ms 50`) the corpse rests at -23.875, and so does the
live player (a scratch rerun of that layout on 1.1d and UO). INFERRED: the dead arm drops the ground every cmd
(`PM_CheckLadderMove`, `dead_move`'s comment), so each cmd air-moves the
body into the ground again. How far one cmd's fall reaches depends on its
length. `pronecrouch_ab` replays the captured cmds and matches the 60 Hz
sink snapshot for snapshot on all three servers. A replay that sends one
cmd a frame after the death (`probe_istouching`'s first gate did) reads
-23.875.

### 5.8 A `trigger_hurt` kill and the corpse's fall

VERIFIED, `client-probes/probe_hurtfall` on retail 1.1d, three
runs (`crates/server/tests/fixtures/triggers/mp_carentan-dm-hurtfall.txt`,
`-hurtlow.txt`, `-hurtland.txt`, recipes in their headers). The probe lifts
mp_carentan's one `trigger_hurt` (entity 72, `dmg 1000`, model `*1`, the
kill slab at z -176..-160 under the whole map) by an origin write and drops
a `--probe-ride` client from z 362 over the street at (900 1930) through
it. The origin write moves the slab's touch volume: every run killed at the
lifted height.

- VERIFIED, all three: the damage callback gets the trigger as inflictor
  and attacker, `dflags` 0, `MOD_TRIGGER_HURT`, weapon `none`, an
  undefined point and an undefined direction, hitloc `none`; the killed
  callback's direction prints `(0.00, 0.00, 0.00)`. INFERRED: the undefined
  pair is `hurt_touch`'s (0x64dc4) null point and direction through
  `Scr_PlayerDamage` (0x5ca18), the shape the sine crush showed
  (`cod11-movers.md` 21), so stock `dm.gsc` adds `iDFLAGS_NO_KNOCKBACK`
  and `finishPlayerDamage` sets no knockback timer (0x43952).
- VERIFIED, `-hurtfall.txt` (slab at 224..240, kill at z 233): the corpse
  falls the remaining 276 on the client's cmds, lands at -43.19 at 3450 and
  then creeps south about 0.05 a frame for 2.5 s, the street's grade under
  `PM_DeadMove` (5.7).
- VERIFIED, `-hurtlow.txt` (slab moved to 27..43 at the drop, kill at z 24):
  the corpse lands 150 ms after the kill at y 1928.90 and creeps the same
  way. A 120 ms knockback timer on the kill would have slid it on to y
  1905 (vcod before the fix; `hurtfall_ab` fails on that).
- VERIFIED, `-hurtland.txt` (slab at -56..-40 from the first frame, its top
  3 over the street): the kill comes first, at origin -42.00 with the street
  at -43.17 under it, and the landing's fall damage callback follows in the
  same frame (both read 3350): 66 points on a body at health 0, which reads
  -66 from then on, with no second killed callback. INFERRED: the kill is
  the touch pass of a cmd still airborne and the fall damage the next cmd's
  `PM_CrashLand` through `ClientEvents` (0x3fda8), the dead player's landing
  of `cod11-player-clip.md` 8.8.

**As implemented.** `ScriptRuntime::deliver_world_hit` passes the callback
undefined vectors. `crates/server/tests/hurtfall_ab.rs` replays each
capture's cmds on vcod-server and holds every `PROBE` line exact.

### 5.9 The hurt's cadence, its flags and its sound

`hurt_touch` is 1.1d 0x64dc4, 1.5b 0x7a456 and UO 0x8672b, one body on
all three (INFERRED, read off the three
decompiles' control flow): a byte on the toucher, then the trigger's
timestamp against `level.time`, then the `"trigger"` notify (1.1 inline,
1.5 and UO through `G_Trigger`), the timestamp moved 1000 on under
spawnflag 0x10 and 100 otherwise, `G_PlaySoundAliasAtPoint` at the
toucher's origin unless spawnflag 0x4, `G_Damage` with the trigger's `dmg`
(gentity+568 on 1.1, +0x240 on 1.5, +0x248 on UO) and `dflags` 0x10 under
spawnflag 0x8, mod 0x17 (UO 0x16), and last the touch function cleared
under spawnflag 0x20. `dmg` and the spawnflags are read at each touch, not
kept from the spawn. `G_PlaySoundAliasAtPoint` (1.1d 0x67a2c, a `G_Spawn`
with `eType` 0xb8; 1.5b `G_TempEntity(point, 0xac)`) takes an entity
number on every touch (`cod11-sound-system.md`).

The captures are `client-probes/probe_hurtfall` on retail 1.1d, 1.5b and UO
1.51b, nine runs
(`crates/server/tests/fixtures/triggers/mp_carentan-dm-hurtrate.txt`,
`-hurtslow`, `-hurtonce`, each with a `-1.5` and a `-uo` twin, recipes in
their headers). Every run drops the client from z -30 on to the street at
(900 1930). VERIFIED: all three servers log the same lines but for the
times.

- VERIFIED, `-hurtrate`: the slab lifted to -56..-40 with `hurt.dmg = 30` written by
  script. Four touches 100 ms apart, 30 each, the fourth kills. A script
  write of `dmg` changes what the next touch takes.
- VERIFIED, `-hurtslow`: `spawn("trigger_hurt", (900, 1930, -20), 16)` with `dmg`
  written to 25. Touches 1000 ms apart, the first on the drop's frame, the
  fourth kills. The script spawn runs `SP_trigger_hurt`: before the write
  the new entity reads `dmg 5` (a separate 1.1d run; the probe logs it
  after the write).
- VERIFIED, `-hurtonce`: the same spawn with spawnflags 44 (4, 8, 0x20) and `dmg` 10.
  One touch, `dflags 16`, and never another in the 6 s that follow.
- VERIFIED, all nine: each damage callback spawns and deletes a `script_origin` and logs its
  number. Under the sound it reads two above the last (171, 173, 175, 177
  on the slab, 172, 174, ... next to the spawned trigger at 170): the
  touch's sound took the number before it. Under the silent flag it reads
  171, the next number after the trigger.
- VERIFIED, 1.1d: a script write of `spawnflags` is the runtime error
  `Tried to set a read only entity field`, so the slow and once flags are
  only reachable through a map's key or `spawn`'s third argument.

#### `START_OFF`, `hurt_use` and the read-only fields

`SP_trigger_hurt` (1.1d 0x64ef8, 1.5b 0x7a6ac, UO 0x86981) stores
`hurt_use` as the entity's `use` on every hurt and `hurt_touch` as its
touch only when spawnflag 1 is clear, then reads `atof` of the spawn key
`life` (default `"0"`; 1.1d `.rodata` 0x799a0 holds `life`) into a float
(gentity+0x274 on 1.1, +0x27c on 1.5, +0x284 on UO) (VERIFIED, three
decompiles). `hurt_use` (1.1d 0x65b94, 1.5b 0x7a605, UO 0x868da) sets the
touch when it is unset and clears it otherwise; when `life` is not 0 it
also sets `think = hurt_think`, `nextthink = level.time + 50` (1.1d
0x65bdd) and stores `(float)level.time + life * 1000.0` (1.1d `.rodata`
0x79ad4 holds 1000.0) (VERIFIED). `hurt_think` (1.1d 0x65b5c, 1.5b 0x7a5aa,
UO 0x8687f) re-arms 100 ms on and calls `G_FreeEntity` once that float is
below `(float)level.time` (VERIFIED). A map hurt with `life` so turns into
a timed hazard at its first use. INFERRED from the three: the toggle also
re-arms a hurt the once flag (0x20) spent.

The use reaches script through `useBy` (entity method 11, 1.1d 0x5d774;
1.5b 0x5d413, UO 0x68698 off `tools/re/dump_builtins.py`): `Scr_GetEntity(0)`,
a `"trigger"` notify on the receiver with that entity, then `ent->use(ent,
other, other)` when set (VERIFIED, 1.1d decompile).

The entity field table gives `classname`, `model` and `spawnflags` one
setter on all three (1.1d 0x6262c, 1.5b 0x65344, UO 0x71480), whose body
is `Scr_Error("Tried to set a read only entity field")`; UO gives its
`vehicletype` the same one (VERIFIED, `tools/re/dump_script_fields.py` and
the 1.1d decompile).

The captures are `client-probes/probe_hurtfall` under `probe_use` and
`probe_write`, ports 34733-34738
(`crates/server/tests/fixtures/triggers/mp_carentan-dm-hurtuse.txt` and
`-hurtwrite.txt`, each with a `-1.5` and a `-uo` twin):

- VERIFIED, `-hurtuse`, all three: a spawned hurt with spawnflags 17
  (`START_OFF` and slow) and `dmg` 25 under the body hurts nothing for
  the first second. The `useBy` at drop + 1000 logs its `"trigger"` with
  the player in the same frame, the next cmd's touch hurts (its callback
  reads the same `getTime()`) and plays its sound, and that touch's
  queued `"trigger"` comes a frame later. The second `useBy` 1000 later
  lands before the slow cadence's next touch: no more damage to the end
  of the run.
- VERIFIED, `-hurtwrite` on 1.1d and 1.5b at `developer 0`
  (`PRE_MAP_ARGS='+set developer 0'`): writes of `spawnflags`,
  `classname` and `model` each leave the field as it was (17,
  `trigger_hurt`, `""`) and their threads run on. At `developer 1` the
  first write ends the server with `script runtime error` (VERIFIED,
  1.1d). On UO at `developer 0` the first write aborts the server
  process (SIGABRT), as a builtin's error does there
  (`cod11-gsc-language.md`, "Runtime errors below developer 1").

#### The model-0 touch, `life`, and `useBy` on the other entities

The captures are on retail 1.1d, 1.5b and UO 1.51b, ports 34761-34770
(`crates/server/tests/fixtures/triggers/`, each with a `-1.5`
and a `-uo` twin, recipes in their headers). VERIFIED: all three servers
logged the same lines but for the times.

- VERIFIED, `-hurtfar` (`probe_hurtfall` under `probe_spawn_x/_y/_z`): a
  hurt spawned with spawnflags 16 at (0 0 8), 1900 units from the body,
  hurts it once a second from the drop's next frame and the fourth touch
  kills, as `-hurtslow`'s point next to the body does. The spawned trigger
  touches through its inline model 0, the world lifted 8: the body,
  dropped from z -30 on to the street at -43.13, is inside that copy's
  street (-35.16) from its 2500 frame on, and not at the drop's 2400,
  where its box still clears it. A point or box trigger would not touch
  at all.
- VERIFIED, `-hurtlife` (`client-probes/probe_useby` under `probe_life`):
  mp_carentan's slab with `"dmg" "1000"` replaced by `"life" "2.5"`, a
  same-length entity lump edit served from a `zzz_hurtlife.pk3`. Its
  `useBy` at 1550 notifies `"trigger"`, the slab reads defined through
  4100 and undefined from 4150: `hurt_think`, armed at 1600 and every 100
  ms after, frees it on the first think whose `level.time` is past
  1550 + 2500, 4100's, in that frame's entity pass.
- VERIFIED, `-useby` (`probe_useby`, no client): `useBy` with a
  `script_origin` on four script-spawned entities at the world origin.
  Each notifies `"trigger"` with the user, logged after the caller's next
  line (the notify's waiters run after its step). A `trigger_once`
  (`wait` -1) reads defined at 1550, 1600 and 1650 and undefined from
  1700: `Use_Multi` spent it and freed it 100 ms on, as its first touch
  would (`cod11-movers.md` 17). A `trigger_multiple` (`wait` 0.5) and a
  `trigger_damage` stay, and a second `useBy` 250 ms on notifies again. A
  `func_static` whose origin the script wrote to (0 0 8) stops a
  `bulletTrace` down at (900 1930) at -35.16 on the static; after one
  `useBy` the trace reaches the world's street at -43.16 and after a
  second the static again.

The use functions, VERIFIED off the three decompiles: `Use_Multi` (1.1d
0x6594c, 1.5b 0x7a2f2, UO 0x865c7) is `multi_trigger`, `Touch_Multi`'s arm
without its notify; `Use_trigger_damage` (1.1d 0x65d10, 1.5b 0x7ab3c, UO
0x86e77) calls `Activate_trigger_damage(self, other, accumulate + 1, -1)`,
whose mod -1 skips the `"trigger"` queue (14.8); `Use_Static` (1.1d
0x58070, 1.5b 0x697a3, UO 0x742e7) unlinks a linked entity and links an
unlinked one, and `SP_func_static` (1.1d 0x5820c, 1.5b 0x699ef, UO
0x74533) installs it on every `func_static`, not only one with
`takedamage` (the 1.1d store at 0x58228). VERIFIED,
`tools/re/annotate_func.py` on 1.1d: `G_SpawnTurret` stores `turret_use`
(0x52a9c) as the turret's `use` at 0x53009, as 1.5b's `G_SpawnTurret`
(0x632be) stores its `turret_use` (0x62fcc); `InitMover` stores
`Use_BinaryMover` (0x560ec) at 0x565d0 on a `func_bobbing` and
`func_pendulum`; `SP_misc_spawner` stores `misc_spawner_use` (0x53468,
which arms `misc_spawner_think` 100 ms on) at 0x534d8.

#### `useBy` on a gun, a sine mover and a `misc_spawner`

The captures are `client-probes/probe_useby` under `probe_sine`,
`probe_spawner` and `probe_turret`, on retail 1.1d, 1.5b and UO 1.51b,
ports 34771-34780 (`crates/server/tests/fixtures/triggers/`
`mp_carentan-dm-usesine.txt`, `-usespawner.txt` and `-useturret.txt`, each
with a `-1.5` and a `-uo` twin, recipes in their headers). Addresses below
are the modules' `nm` and `objdump` ones (1.1d `game.mp.i386.so`, 1.5b's,
UO's `game.mp.uo.i386.so`).

- VERIFIED, `-usesine`, all three servers the same lines and times: a
  script-spawned `func_bobbing` used at 1550 reads `origin` (0 0 0) on the
  line after the call and every frame after, through a second use at 2550,
  and the level runs past the 4000 where an unused one drops it (movers
  doc 18). A `func_pendulum` used at 4050 reads its old roll (15.77) on
  the line after the call, then -0.97, 0, 0.97, 0.97, 0.97, 1.94: its
  swing restarts from 0 off a `trTime` of 4100, then of 4150 and 4200,
  each roll the frame's own entity pass evaluated a frame before the line.
  The second use at 5050 repeats it.
- VERIFIED, 1.1d disassembly: `Use_BinaryMover` (0x560ec) at mover state 0
  stores the activator and calls `SetMoverState(ent, 3, level.time + 50)`
  for each team member (0x56247), then `G_PlaySoundAlias` of the start
  alias past level time 4000, `s.loopSound` 0 or the loop alias (0x562d6)
  and `trap_AdjustAreaPortalState(ent, 1)` for a team master (0x5630b). `SetMoverState` (0x55904) writes `moverState`, `pos.trTime`
  and `apos.trTime` both, `pos` from `pos1`/`pos2` with `trDuration` from
  `ent+0x1f4` (`+0x1f8` for the 2-to-1 leg past a `speed2`), then
  `BG_EvaluateTrajectory(&s.pos, level.time, r.currentOrigin)` and a link
  (0x55c5d..0x55c8f); it does not touch `r.currentAngles` for states 0..4.
  `InitMover` writes the `pos1`..`pos2` time, at least 1, to both
  `+0x1f4` and `+0x1f8` (0x566dd..0x566f3). `G_MoverTeam` (0x558a0..0x558e9)
  calls `reached` for `pos`, then for `apos` as that call left it.
  `Reached_BinaryMover` (0x55c9c) moves state 3 to 1 (`pos2`) and puts
  `ReturnToPos1` (0x57b28) `wait` on unless `wait` is -1000 (0x75c84), and
  state 4 to 0. `ReturnToPos1` runs `SetMoverState(ent, 4, level.time)`.
  `G_PlaySoundAlias` (0x67b30) returns on alias 0, which is all a sine
  mover holds. INFERRED from those and the capture: a use at `pos1` runs a
  sine mover through a 1 ms leg out to `pos2`, the world origin, and a 1
  ms leg back over the next three entity passes (`ReturnToPos1` in the
  pass that reached `pos2`, since `wait` is 0 and `G_RunThink` follows
  `G_MoverTeam`), and leaves it stationary there; a pendulum keeps its
  `apos` swing, its `trTime` set at each of the four state changes, and
  drops the level one period past the last (4200 + 9711) as an unused one
  does. `Use_BinaryMover` at state 3 or 4 (a use inside 100 ms of the
  last) runs `Blocked_Door` (INFERRED, unmeasured).
- VERIFIED, `-usespawner`, all three the same lines but for the drops'
  heights: a script `spawn("misc_spawner", ...)` with `spawnitem` written
  after it is used twice and drops nothing; `SP_misc_spawner` (1.1d
  0x53498) prints `has no spawnitem!` at the spawn and returns before the
  `use` store (0x534aa, VERIFIED, console and disassembly). The map's
  spawner (an `mp_retrieval_spawn_axis` block rewritten, header) used at
  4050 shows its `item_health` (entity 172) from 4200, at the spawner's
  origin, then 7.5 units a frame along +x, its yaw 0: `misc_spawner_think`
  ran in the 4150 entity pass, 100 ms after the use, and the item's own
  turn (172) had passed. A second use 1 s on drops a second item (173).
  The climbs differ run to run: 1.1d's two read 131.5 and 86, (-3, -1] in
  `crandom()` (items doc 14.2), 1.5b's and UO's [150, 250).
- VERIFIED, 1.1d disassembly: `misc_spawner_think` (0x533fc) is
  `Drop_Item(ent, BG_FindItem(SL_ConvertToString(spawnitem)), 0, 0)`
  (0x53428) and prints `failed to drop!` on a 0. `Drop_Item` (0x4ed30)
  launches along `(0, r.currentAngles[1] + angle, 0)` at 150 (0x74e5c), the
  climb `200 + 50 * crandom()` with one `rand()` (0x4eda6), from
  `r.currentOrigin` raised by half `r.maxs[2] - r.mins[2]` (0x4ede8), 0 on
  the point entity, through `LaunchItem` with the spawner's number as the
  owner (0x4ee05). VERIFIED, the 1.5b and UO `misc_spawner_use` (0x538f3,
  0x5d265) add 100 to the level time and link, as 1.1d's does.
- VERIFIED, `-useturret`, all three the same lines but for the times: a
  player placed 40 behind the gun at (1712 1830 8, yaw 229) looking at yaw
  289 is mounted by the gun's `useBy` with him, with no use key and no
  reach test: his `angles` read (0, 274, 0) on the line after the call,
  the yaw clamped into the 45-degree arc, and (0, 0, 0) from the next
  frame; his origin moves from (1738.24, 1860.19) to the mount point
  (1715.42, 1877.25) two frames after the call; his `"trigger"` notify
  carries his number. VERIFIED, 1.1d decompile: `turret_use` (0x52a9c)
  writes through `other->client` (`+0x158`) with no test and ends in
  `SetClientViewAngle` (0x41e30, called at 0x52c72), which stores the view
  into `r.currentAngles` and `ps.viewangles`. VERIFIED: with the
  `script_origin` as the user (`+set probe_turret 2`) each server
  segfaults inside the call, after `PROBE use turret` and before the
  notify's waiter logs.

**As implemented.** `Triggers::arm_hurt` takes the cadence and the once
flag off the `spawnflags` field at each touch, the touch pass reads `dmg`
there too, and the hurt plays its alias on a PVS temp entity at the
toucher's `origin` field before the damage (`ScriptRuntime::add_sound_at`).
`trigger::spawn_scripted_hurt` is the script spawn's `SP_trigger_hurt`, and
both spawns write `dmg` 5 over a 0 (`default_hurt_dmg`).
`Triggers::register_hurt` leaves a `START_OFF` hurt without its touch
and keeps its `life`; the `useby` builtin notifies and runs
`Triggers::use_hurt`, which toggles the touch and returns the free time
`ThinkFn::HurtLife` runs to. `GameHost::store_field` refuses a script
write of a read-only field with retail's error, which vcod's VM recovers
from on UO as on 1.1d: UO's abort is a crash vcod keeps out on purpose.
The touch pass tests a model-0 trigger's box (the client's) by a position
test against its world copy's clip (`Pusher::Copy`, mask -1). The `useby`
builtin then runs `trigger::use_multi` (the wait arm, or the spend and the
free 100 ms on), `damage_trigger::use_trigger`, `mover::toggle_static`,
`mover::use_binary`, `turret::use_by` or `item::misc_spawner_use` by what
the entity is. `mover::use_binary` is `Use_BinaryMover` at `pos1` and
`pos2` on a sine mover's `Binary` state, `mover::reached` its
`Reached_BinaryMover` legs and `ReturnToPos1` (pos then apos, the
`G_Error` on any other state kept), and `set_mover_state` its
`SetMoverState`; the start and stop sounds (alias 0) and the area portal
are not modelled, nor a use during a leg. `turret::use_by` runs
`turret_use`'s record half on the spot from the host's mirrors (origin,
stance, `client_aim`), writes the player's `angles` and queues
`SimOp::Mount` for the sim half; a user that is no player notifies alone,
where retail segfaults, a crash vcod keeps out on purpose.
`item::spawn_misc_spawner` gives a map spawner with a `spawnitem` its use,
`item::misc_spawner_use` arms `ThinkFn::MiscSpawner` 100 ms on, and the
entity pass runs `item::misc_spawner_think`'s `Drop_Item` on its turn. A
script `spawn` of a `trigger_multiple` or
`trigger_once` is `trigger::spawn_scripted_multi`, a world copy with
`wait` 500 or -1000. `crates/server/tests/hurtfall_ab.rs` replays the
thirty-six captures (on UO's write, the lines before the abort; the
spawner's climbs pinned to the draws its frames read back to, its
landings rested where retail's did; the 1.5b write capture is a rerun on
port 34770, as an earlier take drifted 0.16 sideways in its first 50 ms of
fall). No stock
MP map gives a hurt spawnflag 1 or a `life` key (VERIFIED, entity lumps).

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
`0x31778`, `0x31826` and `0x31fc6` whose base register is not identified, and
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

**The angles are `vectoangles`'.** VERIFIED, `P_DamageFeedback`
(0x3f500): step 8's angles come from the module's `vectoangles` (0x3de94),
whose pitch is `-atan2(z, |xy|)` and whose two angles are both wrapped
into [0, 360) (14.8, "`vectorToAngles`"), so a direction a little up reads a
pitch just under 360. VERIFIED, `mp_carentan-tdm-grenade-target.txt`
(ms 9557): a frag going off under the standing target knocked him
(-86.9, -0.6, 14.0) and wrote `damageYaw` 128 and `damagePitch` 249, 350.9
degrees, about 9 up; Q3's `vectoangles` (pitch `+atan2`) would have written
6. VERIFIED, `mp_carentan-tdm-hit-target.txt`: a level carbine hit with
knockback z 0.1 wrote `damagePitch` 255. vcod:
`a_blast_from_below_writes_the_capture_s_pitch_byte` pins the grenade line
(Q3's sign is the trap).

**As implemented** (`ClientSim::take_damage` and `end_frame`,
`crates/server/src/spectate.rs`): steps 2 to 5 and 7 to 13 as listed,
the angles through the same `vectoangles` port as the `vectorToAngles`
builtin (`game::builtins::math::vectoangles`), scaled in double, with
step 1 read as the sim's `dead` flag, which the killing hit sets before the
end-frame runs. Step 6's view kick is kept on the sim (`DamageKick`, the
`client+0x2270`/`+0x2274` pair) and played out by the aim block (15); nothing
on the wire carries it. `EV_PAIN` is raised whatever the stance: nothing above names
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
event entities (`event=201`): the probe read the playerstate and entity rings
one slot high when these were taken, so no `EV_DEATH` or `EV_PAIN` reached
the `!event` list. The `events=` columns on the `!trace` lines are the source
for both. VERIFIED, an uncommitted run with the ring read right
(`crates/common/src/net/events.rs`): the same death drains as `ev 189 parm 0`
on the playerstate ring.

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
item 11's third arm, `(int)self->angles[1]`: `Cmd_Kill_f` names the victim
as both inflictor and attacker, so neither is usable, and this probe's
`angles` yaw read 0 at each of those deaths. 8.4 has the case where the
attacker is usable.

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

Section 8's two probes pointed at `vcod-server` instead of retail, plus three
hand checks with a retail 1.1 client (under Wine) on `vcod-server`. Every
claim here is evidence about vcod; the retail column is section 8's committed
fixtures. The vcod runs are not committed (the fixture names are retail's
evidence): `mp_carentan` `dm` (200 s, settled state and weapon channel),
`mp_ship` `dm` (155 s; carentan is too large for the shooter's walk, as for
retail's `dm` capture, and mp_ship closed to 572 units with a line of
sight), and a `mp_carentan` `dm` retake of the death half once 9.2's
divergences were closed (the capture 9.1 quotes). The hand check covered the
suicide corpse's death anim, the respawn text and key, a head shot on the
probe logging `D;...;head` and `MOD_HEAD_SHOT`, and ADS and reload by eye.

### 9.1 What matches

VERIFIED, `mp_carentan` `dm`, field for field against
`mp_carentan-dm-hit-target.txt`:

| frame | fields that match |
|---|---|
| settled standing | `health` 100, `pm_type` 0, `eFlags` 16, `deadViewHeight` 8, `viewHeightCurrent` 60.0, `legsAnim` 634, `torsoAnim` 0, `weaponstate` 0, `stats[1]` 0, the four damage fields 0, `eventSequence` 0 with an empty ring, `clip=3:7,6:3,10:15 ammo=3:56,10:400` |
| `kill` death | `health` 0, damage fields 0, `pm_type` 6, `eFlags` 16, `deadViewHeight` 8, `viewHeightCurrent` 60.0, `legsAnim` 18, `torsoAnim` 512, `weaponstate` 0, `stats[1]` 0, `clip=3:7,6:3`, `ammo=3:56` (index 10 gone from both), zero velocity; `eventSequence` 2 since 9.2's fix |
| respawn | `health` 100, `pm_type` 0, `eFlags` **24**, `eventSequence` 0 with an empty ring, full arrays; `legsAnim` 634 since 9.2's fix |

VERIFIED: over 130 s of three deaths and respawns `eFlags` alternates 16 and
24 as retail's does (38 samples against 51 here, 115 against 101 in retail's
`dm` capture). The three corpses take entities **64**, **65** and **66** in
order, each on its death frame with `trType` 5, and none vanishes (5.2's
rule; retail's 64/65/66/67; no lifetime timer). The obituary reaches the
probe on all three as `victim=0 attacker=0 parm=150`, retail's own line
(`!obituary ms=10037 victim=0 attacker=0 parm=150`): `MOD_SUICIDE`, index 22,
one of the seven the `0x80` flag covers.

VERIFIED, the shooter's single shot on `mp_carentan`: `weaponstate` 3,
`weapAnim` 514, one `EV_FIRE_WEAPON` (159), clip index 10 counting 15 to 14,
as retail's shooter capture. `torsoAnim` reads 765 (512 | 253), the number
the retaken `combat` fixtures measure; the retail `dm` shooter fixture's 0
predates the `cmd.weapon` fix (`player-model-anim-system.md`, "The
`jump_takeoff` torso index is gone") and is not evidence.

VERIFIED, `mp_ship` `dm`, the kill by a bullet: `health` 0, `pm_type` 6,
`torsoAnim` 512, the dead view height lerping 60 -> 50.8 -> 42 -> 32.8 -> 24
-> 12.1 -> 8 against retail's 60 -> 51 -> 42 -> 33 -> 24 -> 15 -> 8.
`legsAnim` 22 where retail's two read 17 and 18, all three indices of the
standing `death` clause (a random draw, `player-model-anim-system.md`).
`stats[1]` 294 with the shooter at bearing 294.42 (5.1 item 11's `vectoyaw`
truncated). `damageEvent` stays 0 through the fatal hit (INFERRED: 6's step
1). The obituary reaches both probes as `victim=0 attacker=1 parm=5`
(INFERRED: 5 is the killer's configstring 7 weapon index, 4.1's rule for
`MOD_RIFLE_BULLET`, which the `0x80` flag does not cover). The victim
respawns on use with the full loadout, and the killer's score goes 0 to 1 the
frame after the kill.

### 9.2 What differs

Every divergence these runs found is closed. Retail facts and the traps they
set, each with its pin:

- **Bullet death frame `[187, 189, 155]`** (8.4). INFERRED: the 155 comes
  from cmds of the victim's own that reached retail behind the shooter's in
  the same frame (16.1). The tick runs packets in execution order and fires
  each cmd's rounds inside it (16.2), so a player one round killed also stops
  no later round of the tick. Pinned by `a_bullet_deaths_frame_is_retails` and
  `a_player_killed_earlier_in_the_frame_stops_no_later_round`
  (`tests/combat.rs`).
- **The killed callback on a suicide.** Retail's `K;` ends
  `none;100000;MOD_SUICIDE;none` (5.1, "The two suicides"), `vDir`
  undefined. Both `kill` and `suicide()` pass damage 100000, `"none"` and an
  undefined `vDir`; the sim's damage stays 0 (VERIFIED, 8.1: damage fields 0
  across every `kill` death). Pinned by the `K;` check in
  `the_kill_command_suicides_a_player` (`tests/combat.rs`) and
  `suicide_kills_the_player_and_runs_the_killed_callback`.
- **The `kill` death frame**: 189 then 155, `eventSequence` 2; the
  three-probe follow run (`cod11-spectator-follow.md` 9) reads `weapon` 0 and
  `viewHeightTarget` 60 there. VERIFIED, `cod_lnxded`:
  `SV_ExecuteClientMessage` (0x80872ec) reads the 2-bit op, calls
  `SV_ClientCommand` (0x8086e08) while it reads 2, and only then
  `SV_UserMove` (0x8086fa4) for an op of 0 or 1; `SV_ClientCommand` hands the
  command to `SV_ExecuteClientCommand` (0x8086d58), whose game call
  (0x8092158) passes 6, and `SV_UserMove` makes the same call with 7 per cmd.
  INFERRED, off Q3's export numbering: 6 is `ClientCommand`, 7
  `ClientThink`. VERIFIED, `game.mp.i386.so`: `ClientEndFrame` stores
  `pm_type` 6 (7 linked) off a `sessionState` of 1 (0x41057..0x41079) and 0
  or 1 otherwise (0x410a7); `ClientThink_real` (0x3fee0) holds no store to
  `ps+0x4`. INFERRED: a packet's client commands run ahead of its usercmds,
  and the cmds behind a `kill` still read the `pm_type` 0 the last end frame
  wrote, so they run the live move: `PM_Weapon` finds the carbine gone and
  goes down to empty hands with `EV_RAISE_WEAPON` (1.8), and `PM_CheckDuck`
  still targets the standing eye. vcod: `client_command` marks where in the
  cmd stream the `kill` arrived, `replay_moves` runs it there, and the sim's
  `pm_type` is the one its end frame wrote (`ClientSim::end_frame`), with the
  wire's `viewHeightTarget` off the last move's arm. Pinned by
  `the_kill_commands_death_frame_is_retails` (`tests/combat.rs`).
- **The death frame's `weaponstate`.** VERIFIED, run: `--probe-kill-cmds
  1,3` sends each `kill` with exactly one or three cmds behind it, one
  message each, then no cmd for 150 ms. On 1.1d, 1.5b and UO 1.51b
  (`mp_carentan-dm-hit-target-killcmds{,-1.5,-uo}.txt`, 7, 10 and 10 kills) a
  kill with one cmd reads `weaponstate` 1 and one with three reads 0, kept
  until the respawn; the events are `EV_DEATH`, `EV_RAISE_WEAPON` (UO's
  `EV_REMOVE_WEAPON_ATTACHMENTS` between them) either way. VERIFIED:
  `PM_Weapon`'s dead arm stores only `ps->weapon` 0: 1.1d `.so` 0x390fe, 1.5b
  `game.mp.i386.so` 0x303fc (behind `cmpl $0x5,0x4(%eax)` at 0x303ec), UO
  `game.mp.uo.i386.so` 0x372bb (`ps+0xd8`, behind 0x372ab); `player_die`
  stores no `weaponstate` on any. INFERRED: the first cmd's putaway to empty
  hands raises weapon 0 (state 1); a second live cmd before `ClientEndFrame`
  ends that raise (weapon 0's `raiseTime` is 0) and reads 0. The unscripted
  captures' mix (the `tdm` hit target: one death in five reads 1) is where
  the kill fell in the frame. Pinned by `tests/kill_cmds_ab.rs` at each
  install.
- **The respawn frame's `legsAnim` 634.** VERIFIED, `game.mp.i386.so`:
  `ClientSpawn` ends in `ClientThink_real` (0x42a82) on a zeroed cmd stamped
  `level.time` (0x42a2f, 0x42a42), 100 ms past the `commandTime` it set
  (0x42a48, 0x42a6f), with angles the negated `delta_angles`
  (0x42a4e..0x42a69). INFERRED: that think runs 100 ms of live pmove, whose
  animscript pick puts the standing idle on the spawn frame. VERIFIED: the
  capture's respawn frames read velocities of (5, 5, -1) and (0, 1, 0).
  INFERRED: that is the think's 100 ms fall. vcod: `ClientSim::spawn_move` runs that
  cmd and the tick picks anims straight after; pinned by the respawn half of
  `the_kill_commands_death_frame_is_retails`. A spawn point above a slope
  lands moving and plays a run on its spawn frame, so `turret_ab` compares
  the legs toggle as its change from the first sample.
- **A walker's ground under the sight.** `mp_carentan-tdm-ads-sniper`
  `ads_walk` reads `groundEntityNum` 1022 on all 30 samples on retail. A
  dropped ground frame clears the sight flag (1.13) and reverses the ramp
  for a cmd, which on `adsZoomFov` 16 reads as a twitching scope. The cause
  was `PM_StepSlideMove`'s ground snap and the collider on a terrain seam,
  not `pmove::ground_trace` (`cod11-mantle.md`, "The ground snap" and "What
  the collider does to a walker on a terrain seam"); pinned by
  `walking_a_slope_never_leaves_the_ground` and
  `standing_on_a_slope_holds_its_height`.
- **A segmented reload's start exit.** `ads_release` reads `weaponstate` 7,
  then 9 with `weaponDelay` 0 throughout on retail. The rifle ships `clipSize`
  5, `reloadStartAdd` 1 and `noPartialReload` 1 and fired one round, so the
  start's add fills the clip and 1.7's start exit (`0x30010c50`) refuses the
  loop. vcod's start exit runs the reload test, a state 8 with rounds in the
  clip ends the reload too, and the start's add reads `reloadStartAdd`
  (`vcod_common::pmove::weapon`).
- **A dead player's entity** leaves the wire the frame after the kill and
  returns on the respawn (5.4): the snapshot build makes no entity for a dead
  sim.
- **No pain anim on the server.** Retail's captures keep `legsAnim` 634
  through a surviving hit; the stock `pain` clause
  (`pb_crouch_pain_holdStomach`, 1.35 s) would double the victim over under
  the bone trace (3.4).
- Also closed: the frag's reserve entry (`clipOnly 1`: a give writes the clip
  and no reserve), the held weapon's rounds going with the death drop, the
  `eFlags` `0x8` toggle, the event ring cleared by a respawn, the `kill`
  client command, and the `dm` scoreboard's two team totals (retail sends
  `0 0`, not the `-9999` sentinel).

### 9.3 What this run could not reach

The one-shot kill on mp_ship means no non-fatal hit was observed live against
vcod: `damageEvent`'s increment, `damageCount`, `damageYaw`/`damagePitch`,
the `EV_PAIN` parm, the knockback and the hit-location multiplier are pinned
in process by `crates/server/tests/combat.rs`, which reproduces 8.4's 33
health, 67 `damageCount`, `EV_PAIN` parm 33 and `MOD_HEAD_SHOT` from two
clients on one server.

PENDING, by eye on a retail client: the shooter's own muzzle flash and fire
sound, the damage direction indicator, and a kill by another player seen from
the victim's side.

### 9.4 What is not modelled at all

- `EV_CROUCH_PAIN` (188): `EV_PAIN` is raised whatever the stance.
- The direct-hit `MOD_GRENADE` arm of a thrown grenade (13.1), which a stock
  frag cannot reach (`damage 0`); a shell's direct hit is modelled
  (`Explosion::direct`).
- `pm_flags 0x400`, and the `pm_time` and `eFlags 0xC000` arms the sight
  fraction shares with the rest of `PM_Weapon`.

Modelled since this list was first written, with their sections: the view
kick of 6's step 6 (`ClientSim`'s `DamageKick`, played out by the aim block,
15), the water mask and splash of 12.1 (`missile.rs`), melee and grenades
(1.10, 2.5, 1.11, 11 to 14, measured in 9.5), 14.1's falloff, `CanDamage`'s
line of sight (4.6, 14.3), `setPlayerIgnoreRadiusDamage` (14.2), item pickup
(`cod11-items.md`), and the sight fraction and spread scale (1.13, 2.1;
`spread_deg` in `crates/server/src/game/combat.rs` and 1.2's and 1.9's
`fWeaponPosFrac > 0.75` anim tests read the fraction, not the sight bit).
`playerstate_combat_ab` replays the `--save-ads` captures on retail's cmds
and matches every sample to the print. The turn term is unpinned: both
fixture weapons spell `hipSpreadTurnAdd 0`, and the BAR, the one stock MP
weapon with one, needs a probe that answers the weapon menu with it.

### 9.5 Melee and grenades, measured the same way

Four vcod runs, `mp_carentan` `tdm` with `scr_friendlyfire 1`: the lone
`--save-grenade` script, and target-plus-shooter pairs under
`--probe-melee`, `--probe-grenade` and `--probe-grenade-death`. The retail
column is section 8's family of seven committed fixtures. None of the vcod
captures is committed.

**What matches.** VERIFIED:

- The melee triple, event for event: `EV_MELEE_SWIPE` (164) on the
  attacker's ring, then `EV_MELEE_HIT` (166) or `EV_MELEE_MISS` (167) on a
  broadcast temp entity, then `EV_FIRE_MELEE` (165) on the attacker's ring,
  the last two on one frame about `meleeTime` after the swipe (retail 146 to
  165 ms over ten swings, vcod 98 to 166; the spread is the probe's snapshot
  timing). Eleven swings each: retail two hits and nine misses, vcod three
  and eight.
- Melee damage: retail's target 100 to 44, vcod's 100 to 53 to 5, all inside
  `meleeDamage + rand(0..5)` times a hit-location multiplier (3.5): 56 at
  `torso_upper`'s 1.1, 47 and 48 at a leg's 0.9. Different approaches, so
  different locations.
- The grenade event chain and explode frame: `EV_PULLBACK_WEAPON` (158) on
  the thrower's ring, one `EV_GRENADE_BOUNCE` (177) per bounce and
  `EV_GRENADE_EXPLODE` (178) with parm 5 on the missile's ring, on a frame
  reading `eType` 0, `eFlags` 256, `pos` and `apos` `TR_STATIONARY` with zero
  delta, origin truncated to whole units; the explode lands in the slot below
  the sequence on both.
- `grenadeTimeLeft` takes 0 or 4000 and nothing between (the two
  `-grenade-shooter.txt` captures: 499 vcod traces, 233 retail). No cook
  countdown in 1.1 MP (1.14).
- The fuse: pullback to explode 4568 ms on vcod against retail's 4759 and
  4549, on the same 1000 ms cook and `fuseTime` 4.
- The switch to the frag against `to_frag`: putaway 14 frames against
  retail's 13, raise 5 against 6 (retail's own takes read 14/5 and 13/6; the
  cmd-logged replay matches sample for sample). The cook holds `weaponstate`
  3 for 41 frames against 40.

**Traps these runs found**, fixed:

- The tumble. Retail draws `apos.trDelta` per throw: roll rates 354.4, 358.7,
  367.4, 367.9, 387.1 and 402.6 deg/s (inside `360 +/- 45`, constant over a
  throw's life), launch pitch rates 687.7, 692.7, 704.1, 726.9, 762.1 and
  763.5 (inside `720 +/- 45`; older takes' rolls 323.0, 335.9, 368.4, 316.3,
  318.0, 344.8 and pitch rates 684.9, 703.5, 729.5), redrawn at each bounce
  (12.7's `G_MissileLandAngles`). A zeroed `flrand` state was `xorshift`'s
  fixed point and read `675.0, 0.0, 315.0` on every throw. The tumble now
  draws the level's `holdrand`, the script's own stream (map-cycle doc
  4.8).
- An absent `semiAuto` is off. `mp_carentan-tdm-grenade-death-shooter.txt`
  raises the frag under `buttons=1` for five frames (ms 10734 to 10930,
  `raiseTime` 0.25), then `weaponstate` 3 with `EV_PULLBACK_WEAPON` and
  `grenadeTimeLeft` 4000 at ms 10995. Of the 32 stock `weapons/mp` files the
  eight that omit the key (four grenades, three bipod mg42s, the PTRS41) do
  not latch; the carbine, which spells it, does (`cancel`: `weaponstate` 1
  for 54 frames under a held bit). `crates/common/src/weapon.rs` defaults it
  off.
- A bounce writes the material into the event parm only and leaves
  `s.surfType` to the explode (13.1 step 5): the client reads
  `grenade_bounce_<es.surfType>` (`cod11-sound-system.md` 7a) and
  `iw_sound.csv` has only `grenade_bounce_default`, so a tagged `surfType`
  silences the bounce.
- `RegisterItem` precaches `projectileModel` verbatim, prefix and all
  (retail's table: `394 xmodel/projectile_GermanGrenade`, `386
  xmodel/projectile_USGrenade`, `configstrings/mp_carentan-dm.txt`;
  `configstrings_ab` pins ours), so a lookup with `WeaponDef`'s stripped name
  misses (`crates/server/src/configstrings.rs` puts the prefix back).
- An empty frag leaves the weapon list: `throw_down` ends on `weapons[0]`
  4112, `weaponslots[4]` 0, `weapon` 0 (the step before reads 4368, 8, 8).
  INFERRED: 1.5 step 9's `BG_TakePlayerWeapon`, then 1.8's switch path
  finding the player no longer owns what he holds. vcod's last shot of a
  `clipOnly` weapon with no reserve queues a take beside the weapon change
  (`crates/server/src/server.rs`).

**Open.** The blast on a player is unmeasured live against vcod: vcod's
`tdm` picker put the pair 280 units apart on two floors (`z` -143.9 against
-23.9), and the shooter threw from 296 units with no line of sight; retail
spawns same-team clients 41 to 367 units apart. Retail's target drops 100 to
26 at 137 units, which 14.1's falloff reproduces to the unit in process
(`crates/server/tests/combat.rs`). The lone `--save-grenade` script also
blew up its own thrower against vcod (at 57 s and 23 s; the mode has no
respawn), where retail's survived from a spawn 144 units up and the later
takes' spawn at z -4.5 take no damage. INFERRED: the spawn geometry, not the
damage.

Not divergences: temp entity numbers come from `G_Spawn`'s free list on both
(`crates/server/src/game/temp_entity.rs`, 14.7 "Entity numbers"), so they
match only when the entity history does; and the bounce parm on a prop is
13.4's prop-material rule.

PENDING, by eye on a 1.1 client against `vcod-server`: a throw's arc, a
bounce off a wall and a floor, the explosion's decal and sound, damage at
three ranges against the server's `D;` records, a grenade kill and its
killfeed line, a melee kill and its icon, a death while cooking and the
grenade the body drops, and a wall between the eye and the blast.

### 9.6 What the third hand check found

Three traps, fixed in `crates/common/src/pmove/weapon.rs` and
`crates/common/src/pmove.rs`:

- A missing def must not end `pm_weapon` before the switch check. INFERRED,
  1.12: retail ends `PM_Weapon` on three tests, none a missing def, so the
  switch path runs with weapon 0 in hand (no melee, reload or shot). After a
  last frag's take, a putaway from weapon 0 leaves no drop time, so 1.8's
  pickup raises the new weapon on the same frame. Pinned by
  `a_player_who_lost_its_last_grenade_can_switch_back` and
  `a_client_out_of_grenades_gets_its_rifle_back` (`two_clients.rs`).
- 1.8's pickup forces the new weapon to 0 when `pm_flags & 0x10` is set (dll
  0x300107c0), so a climber holding nothing raises nothing:
  `a_climber_with_nothing_in_hand_raises_nothing`.
- `step_slide_move` is retail's `PM_StepSlideMove`, with the extra half step
  under the feet (`cod11-mantle.md`, "The ground snap"); Q3's leaves a walker
  airborne at every crest. VERIFIED, `mp_carentan` walked from (412, -1704):
  6 of 80 frames airborne with the origin jumping 4.38 to 4.04 to 3.99 across
  one crest under Q3's form, 0 of 80 with the snap. Pinned by
  `walking_a_slope_never_leaves_the_ground` and
  `standing_on_a_slope_holds_its_height`.

### 9.7 The ladder and the weapon byte

VERIFIED, cgame `0x30033ed5`..`0x30033ede`: the usercmd's weapon byte is
`cg_weaponSelect`, handed to trap `0x55` with the sensitivity. VERIFIED, the
six writes of `"cg_weaponSelect"`: the respawn reset `0x30028a70` (from the
snapshot's `ps.weapon`), the pickup `0x3001da80` (only while it reads 0), the
out-of-range clamp in the frame, the select `0x30037f20` (the binds and the
`a` command), and the cycle `0x30038570`, which writes 0 when the selection
is no longer owned. INFERRED: nothing writes it when the ladder takes
`ps.weapon` to 0, so a retail client keeps asking for the weapon it climbed
with, and 1.8's pickup half raises it on the first frame off the rungs.

VERIFIED, `client-probes/probe_ladder` on `tools/run_probe.sh` `mp_ship`,
the `--probe-ladder 90` client building its cmds through `PlayInput` up the
hold ladder (4332, -94, 56): `pm_flags` reads 0x40010 on the rungs and
0x40000 off them; `weapon` 5 goes to 0 through a `weaponstate` 2 of 383 ms;
with the byte kept at 5 the first snapshot off the rungs reads `weapon` 5,
`weaponstate` 1, `weaponTime` 467 to 500, `weapAnim` 0xa. A client sending
`ps.weapon` as the byte sends 0 from the putaway on, and stays empty-handed
(the run sat 7 s at 615 until the slot 1 bind sent 5). At the lip the
probe's held forward bobs on and off the rungs, and each bob holsters and
raises the rifle again: retail's. Retail holds `weapAnim` 0x200 for the
whole climb with the byte 5 (no toggle flips with empty hands on a ladder).

vcod: `PlayInput` keeps 9.8's selection and sends it whatever `ps.weapon`
reads (`empty_hands_keep_asking_for_the_weapon_held_before`); every vcod
sender carries the byte (the play client, bots' `armed`, the A/B replays'
logged bytes, the net probe's echo); pmove reads a 0 as retail does (1.8);
a climber is re-armed only by the byte, through the putaway's short branch
and the pickup on the same frame. `begin_change` keeps the idle toggle still
on a ladder (`a_climber_with_nothing_in_hand_raises_nothing` checks
`weapAnim` every frame). `ClientSim::to_wire` sends `PMF_LADDER`
(`a_climber_is_sent_pmf_ladder`).

VERIFIED, the climb rate: retail climbs the hold ladder at a level view at
59.4 units/s (64.5 at 9450 ms to 578.0 at 18100 ms), `pm_ladderScale` 0.5 x
upscale 0.625 x 190. VERIFIED, `game.mp.i386.so` `PM_LadderMove`
(`0x33944`): its calls are `PM_CheckJump`, the air move, `VectorNormalize`,
`VectorNormalize2`, `ProjectPointOnPlane`, `PM_CmdScale`, `VectorNormalize2`,
`VectorNormalize2D`, `PM_StepSlideMove`, `vectoyaw` and `AngleDelta`, with no
`PM_Friction` (`0x2e460`); the accelerate is inline (9.0 at `0x70cc8`, the
100.0 cap at `0x70ccc`), and the one friction is 16.0 (`0x70cd0`, `0x33c81`)
on the velocity along the flattened `pml.right` (`VectorNormalize2D`,
`0x33c23`), only when `cmd.rightmove` is 0 (`0x33c00`). VERIFIED, the cgame
copy `0x3000c060` reads the same, and its `PM_Friction` (`0x30007430`) has
no ladder term. Trap: `friction` with `pm_ladderfriction` 16 on the whole
velocity before the accelerate climbs at 32, 9/16 of retail. vcod:
`pmove::ladder_move`, pinned by `ladder_ab.rs` against a capture at five
pitches (`cod11-mantle.md`, "Ladders"; the bots' numbers in
`bot-navigation.md`).

### 9.8 The client's weapon selection

What the cmd's weapon byte (9.7) is, who moves it and who reads it, out of
`cgame_mp_x86.dll` (1.1). `cg_weaponSelect`'s integer is `0x301cbb0c`; every
write below goes through the cvar-set trap (`9`) with `"%i"` (`0x3006567c`).

VERIFIED: the six writes of `"cg_weaponSelect"` sit in `0x30028a70`,
`0x3001da80`, the frame at `0x30033e73` and `0x30033e8e`, `0x30037f20` and
`0x30038570`; nothing else stores `0x301cbb0c`.

- VERIFIED, `0x30028a70`: the selection takes the snapshot playerstate's
  `weapon` (`cg.snap + 0xbc`), and `cl_run` is set to `"1"` and `cl_stance`
  to `"0"` beside it. INFERRED, off its callers (`cod11-hud-protocol.md`,
  "Weapon name and ammo"): that runs on the first snapshot of a gamestate
  and on a new `stats[5]` or `clientNum`, a respawn or a new followed body.
- VERIFIED, `0x3001da80`: a pickup of a weapon row (type 1 at
  `0x30076310`) sets the selection to the row's weapon only while the
  selection reads 0, and writes no `cl_run`. VERIFIED: its one caller is
  the arm for events 146..148 (`0x3001e0ff`), gated on the snapshot's
  `pm_flags & 0x50000` and the event's entity being its `clientNum`.
- VERIFIED, `0x30033e2a`..`0x30033e94`: a selection below 0 or above the
  weapon count (`0x300eef38`) prints `Invalid cg_weaponSelect setting`,
  writes each non-zero slot of 1..5 in turn and then `"0"`, so the value
  that stands is 0.
- VERIFIED, `0x30037f20`, the select (the old selection in `ecx`, the new
  weapon in `eax`, which `0x30037f4f` formats into the cvar): it stamps the
  weapon name whatever it is handed, and for a new weapon writes it and
  sets `cl_run` to `"1"` (`0x30064a28`) unless the new weapon is non-zero
  and is the old selection's `altWeapon` (def `+0x2fc`, `0x30037f3e`). vcod
  follows the instruction (the stock modes link both ways, so the direction
  only matters on a one-way chain).
- VERIFIED, the command table at `.rdata` `0x30061000`: `weapnext`
  `0x30038040`, `weapprev` `0x300380b0`, `weapalt` `0x30037f90`, `weapon`
  `0x30038130`, `weaponslot` `0x300382f0`. VERIFIED: each returns at once
  unless the snapshot's `pm_flags` has `0x40000` and the predicted
  `pm_flags` lacks `0x4000`; all but `weapon` also need `cg.time` less the
  name's stamp to reach `cg_weaponCycleDelay`. VERIFIED: `weaponslot` takes
  slot 1..5 by name or number (UO 1..7; `cod11-hud-protocol.md`, "Weapon
  slots") and selects the slot's weapon when it is non-zero and carried
  (`0x30037f00`); `weapalt` refuses on `eFlags & 0xc000` (a mounted gun) and
  prints `CGAME_THIS_WEAPON_HAS_NO_ALTERNATE` for a selection with no alt;
  `weapnext` calls the cycle `0x30038570(1, 0)`, `weapprev` `(0, 0)`.
- INFERRED, off `0x30038040` and `0x300380b0`'s first branch: ahead of those
  gates, while `+scores` holds the board (`0x3020c030`, set by `+scores`
  0x30013d1f and cleared by `-scores` 0x30013cce), `weapnext` scrolls it up
  (`0x3020c03c` less `cg_scoreboardScrollStep` `0x301ad4ec`, floored at 0,
  only from above 0) and `weapprev` down (only while `0x3020c040`, the board
  ran out of room, at most to the last row), the same moves as the wheel
  (`0x30027180`, `0x3002b720`); neither cycles nor stamps. VERIFIED: 1.5's
  `0x300399d0` / `0x30039a40` and UO's `0x300474c0` / `0x30047550` open with
  the same branch, UO's ahead of its seat test. vcod:
  `Scoreboard::take_cycle`, ahead of the seat and view gates in `main.rs`.
- VERIFIED: the stamp (`0x3020c920 = cg.time`) is written by `weapnext` and
  `weapprev` before the cycle (`0x300380a2`, `0x3003811c`), by `weaponslot`
  before it parses its argument (`0x30038342`), and by the select
  `0x30037f20` (`weapalt`, `weapon`, `a`). INFERRED, off that order: every
  cycle bind that passes the gates and the delay stamps, an empty slot or a
  fruitless cycle included; only `weapalt` and `weapon` stamp on a select
  alone. VERIFIED: 1.5 (`0x30039a32`, `0x30039aac`, `0x30039c52`) and UO
  (`0x3004753f`, `0x300475dd`, `0x300477b5`) store it at the same points.
  vcod stamps the same way (`main.rs`, the `Effect::Impulse` arm).
- VERIFIED, the gates' bits: the snapshot's `0x40000` is 1.5's `0x20000`
  (`0x30039a0e`) and UO's `0x80000` (`0x3004751b`), each version's own-view
  bit, which vcod reads off `cg.snap` (`Held::cg_snap`). The predicted
  `0x4000` is never set by any server (`cod11-mantle.md`, "`pm_flags`
  0x4000"; `coduo-diff.md`), so vcod models no arm for it.
- VERIFIED, `0x3002e139`: the server's `a <n>` selects `atol(n)` through
  `0x30037f20`, unchecked.
- VERIFIED, the cycle `0x30038570(forward, need_ammo)`, gated on the
  snapshot's `pm_flags & 0x40000`: the selection's slot comes from
  `0x3000f5c0` with the alt chain followed. INFERRED, off its branches: from
  a slot, the slots past it toward 5 (forward) or toward 1, without
  wrapping; then `0x30038430`, the carried weapons in no slot, from 1 up
  (stopping short of the last number) or from the count down; then
  `0x300383b0`, the first non-empty slot from 1 up or 5 down. From a
  selection in no slot, the weapon numbers past it, wrapping, skipping the
  selection's own alt modes and every slotted weapon, then `0x300383b0`,
  then `0x30038430`. `need_ammo` skips a weapon whose `ammo[ammoIndex] +
  ammoclip[clipIndex]` (def `+0x1a0`, `+0x1a8`) is 0. When the fallbacks
  found nothing and the selection is no longer carried, it is set to 0 with
  `cl_run` `"1"` (`0x300387d3`).
- VERIFIED, `0x300381b0`, reached from `EV_NOAMMO` (arm `0x3001e1d4`) and
  `EV_DROPWEAPON` (arm `0x3001e1f9`) through the same gate as the pickup
  (`0x3001e21b`, jump `0x3001e241`): with the held weapon in a slot, the
  first slot from 1 whose weapon has a round; otherwise the cycle `(1, 1)`.
  It reads the predicted playerstate (`0x3020715c` handed to `0x3000f5c0` in
  `edi`; ammo pairs at `0x30207268` and `0x30207368`, its `+0x10c` and
  `+0x20c`). INFERRED, off the def's `+0x7c` test: the arm ahead of that,
  same-slot weapons first, is `slotStackable`'s, which every stock
  `weapons/mp` file leaves 0 or absent; vcod leaves it out.
- VERIFIED, `0x30037f90`, `weapalt`: past the gates above (snapshot
  `pm_flags & 0x40000`, predicted `pm_flags` without `0x4000`, predicted
  `eFlags & 0xc000` clear, `cg_weaponCycleDelay` from the stamp) it reads the
  selection's `altWeapon`; 0 prints `CGAME_THIS_WEAPON_HAS_NO_ALTERNATE`
  (trap `0x39` with `"game message"`) through `0x300207b0`, the window the
  `e` and `f` commands print to (`0x3002e0d0` cases `0x65`, `0x66`); a
  carried mode is selected through `0x30037f20`; an uncarried one does
  nothing.
- VERIFIED, `0x30038130`, `weapon`: the same first two gates and no cycle
  delay; `0x3000f260` looks argument 1 up against each def's name (`+4`,
  case-folded by `0x3003e8f0`), printing `Couldn't find weapon "%s"` on a
  miss through `0x300208f0`, which prints only while `0x30298d8c` is set
  (INFERRED: `developer`); a 0 there falls to `atol` (`0x3004afde`), and a
  non-zero result is selected unchecked, carried or not, like `a`.

The readers. VERIFIED: the cmd byte (9.7). VERIFIED, `0x30023c30`,
`0x30023d50` and `0x30023e90`: the weapon name, its backdrop and the mode
icon draw the selection's def when the selection is at least 0, below the
count and carried, else the predicted `ps.weapon`'s (`0x30209484`, set from
`0x3020720c`). VERIFIED, `0x30025ab0`: the ammo count draws nothing while
`ps.weapon` is 0, and reads the selection while the snapshot's `pm_flags` has
`0x40000` and the selection is at least 0 and below the count, carried or
not, else the entity's weapon; a 0 there draws nothing. VERIFIED, the ammo
backdrop (ownerdraw 6, `0x30026c01` to `0x30023be0`): drawn while
`ps.weapon` is not 0. INFERRED: the count is the highest weapon number, so
the last weapon of configstring 7 never passes the `< count` tests.

What the server sends. VERIFIED, `game.mp.i386.so` `PlayerCmd_switchToWeapon`
(`0x44c34`): for a weapon `Com_BitCheck` finds in `ps.weapons` it sends
`trap_SendServerCommand(client, 1, "a %i")` (`.rodata` `0x731c5`, `0x44cea`)
and returns `Scr_AddBool(1)`; otherwise `Scr_AddBool(0)` (`0x44d05`). It
writes nothing to the playerstate. VERIFIED: `Add_Ammo`'s caller sends the
same string on a weapon pickup (`0x4d1e5`, items doc 4.2).

VERIFIED, live: `client-probes/probe_weapsel` on `tools/run_probe.sh
mp_carentan` against a `--probe-weapsel` client that routes `a` and its own
events into `PlayInput`. The server log reads `before_pistol 14750 current
m1carbine_mp`, `before_primary 18750 current colt_mp`, `before_dry 22750
current m1carbine_mp` and `after_dry 30750 current colt_mp`. On the wire:
`a 4` arrives, the byte goes 12 to 4, `EV_PUTAWAY_WEAPON` (parm 0) at 14800
and the colt raises at 15450; `a 12` brings the carbine back by 19100; the
first dry pull at 29150 carries `EV_NOAMMO` (`weaponstate` 3, `weaponTime`
618, `weapAnim` 0), the selection goes to the colt, and the colt raises at
30500.

vcod: `play::input::PlayInput` (the selection, `a`, the out-of-ammo switch
off the predicted ps, `PlayInput::predict_held`,
`out_of_ammo_reads_the_predicted_ammo`; `alt_weapon` and `weapon_command`,
pinned by `weapalt_selects_the_alt_mode` and
`the_weapon_command_takes_a_name_or_a_number`). The server's
`switchToWeapon` sends `a` and returns the bool, writing no playerstate (a
putaway started on the server is undone by the client's unchanged byte); a
bot, which has no cgame, moves its own byte (`Bot::observe`). `weapsel_ab`
pins the sequence, the putaway's parm 0 and the dry pull (1.8, 1.5 step 2).

### 9.9 The select-fire switch

VERIFIED, `game.mp.i386.so` `BG_GivePlayerWeapon` (`0x36a38`), which
`giveWeapon` and `setWeaponSlotWeapon` (`0x43fb9`) call: a new weapon is held
and slotted, and every mode on its `altWeapon` chain is held beside it with
no slot; `BG_TakePlayerWeapon` (`0x36b78`) empties the slot the weapon or one
of its modes stands in and drops the whole chain. vcod:
`PlayerWeapons::give_chain` / `take_chain`, with `WeaponTable::load` linking
`altWeapon`.

VERIFIED, live, `mp_carentan-dm-weapsel.txt` (local 1.1d): `probe_weapsel`
puts a loaded thompson in the primary slot and switches to it; the client's
`weapalt` sends `thompson_semi_mp` (32) from the thompson (31). The next
snapshot reads `EV_WEAPON_ALT` (157) with parm 0, `weaponstate` 2,
`weaponTime` 682 (`altDropTime` 0.7), `weapAnim` 527 (`WEAP_ALTSWITCHFROM`
with the toggle) and the slot word's primary byte 31 to 32 on that frame;
700 ms on, `weapon` 32 with `weaponstate` 0 and the idle anim, no
`EV_RAISE_WEAPON` (`altRaiseTime` 0, so the raise ends on the next cmd). The
second `weapalt` runs the same back to 31. The server log reads `mode_2 slot
thompson_semi_mp ... current thompson_semi_mp`. `weapon colt_mp` then runs
the ordinary putaway (156, parm 0) and raise (155), and a `weapalt` on the
colt prints the no-alternate message and sends nothing. vcod:
`pmove::weapon::putaway`, `alt_slot_swap` and `pickup`, with the moved slots
written back to the script host (`ScriptRuntime::set_client_slots`).
`weapsel_ab` replays the capture's cmds verbatim and matches every
transition.

---

## 10. Open cells

- UNRESOLVED: retail labels `head` at 44 to 49 units above the origin on a
  victim hit 200 to 400 ms earlier, on a victim whose `legsAnim` is back at
  634 (3.4, second table), where vcod reads `torso_upper` on an idle victim.
  Candidates: a server-side cross-fade out of the knockback's one-frame run,
  or the echoed `viewangles` lagging the tap by more than one snapshot. A
  sweep against a target with more than two hits of health at a fixed short
  range, with the pitch read off the cmd rather than the echo, would settle
  it.
- UNVERIFIED: what sets `pm_flags 0x800`, which stops `PM_Weapon` outright,
  and what `pm_flags 0x400` and `0x4000` mean. INFERRED for `0x400`: set
  whenever a fire, a melee finish or a weapon change happens while
  `pm_flags & 1`, and ORed in by the ADS flag update's prone arm (1.13), so
  it is prone-specific.
- UNVERIFIED: the meaning of bit `0x10` of the trace word at offset 32, which
  makes a bullet continue through a surface at full damage.
- UNVERIFIED: what `Bullet_Endpos` (`0x69624`) is for; nothing on this path
  calls it.
- UNVERIFIED: the hit-location partition itself. It is decided inside
  `trap_LocationalTrace`, in the engine binary, so nothing above pins how a
  point on a player maps to one of the 19 names.
- Settled: every jump adds 64 to `ps.aimSpreadScale`, capped at 255
  (`PM_CheckJump` 0x2edc6-0x2edf3, `cod11-mantle.md`, "Jumps"; the 255 once
  attributed to the ground jump was the prone dive's block). VERIFIED: both
  motion fixtures sample 16 ms after the takeoff and read 66.28 (carentan)
  and 67.81 (pavlov) from 0. INFERRED: 64 plus one frame of 2.1's airborne
  climb; vcod's replay climbs 2.4 a cmd after the 64. `pmove::check_jump`
  adds it.
- Settled: the putaways once read at the reload key, a stance change and a
  jump were the probe sending `cmd.weapon` 0 (1.8; `player-model-anim-system.md`,
  "The weapon channel").
- Settled: `client+0x220C` and `client+0x2210` are the aim `ClientThink_real`
  computes ahead of `Pmove`, the view rotated by the gun's own angles down a
  sight. Section 15.

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
  CoDExtended's `src/shared.h:449`, and the entity runner that
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
- `clipmask` (`+0x190`) takes `0x02802091`. VERIFIED, the disassembly:
  1.1d `fire_grenade` stores it at 0x544a0 and `fire_rocket` at 0x546ef;
  1.5b's store it at `+0x198` (0x5539d, 0x556bd); `G_RunMissile` loads
  `+0x190` at 0x5409a and 0x540ac with no fallback, so every 1.1 missile
  moves under it (vcod's `MASK_MISSILE_MOVE`). VERIFIED: the bullet's mask (2.3)
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

What the client draws it with is the weapon, not `s.index`. VERIFIED: every
`!missile` line of the lone grenade capture
(`crates/server/tests/fixtures/playerstate/mp_carentan-tdm-grenade.txt`)
reads `weapon=8 index=0`. VERIFIED: `cgame_mp_x86.dll` (1.1)'s weapon field
table names `projectileModel` at def offset `0x31C` (the table entry at
`0x30075D2C`), the weapon setup registers that string as a model (trap `7`)
into the per-weapon slot at `0x301A6A64`, and the only reader of that slot is
the function at `0x3001B3F0`, which indexes the per-weapon block by the
entity's `+0xC8`, `s.weapon`. INFERRED: that function is the missile's draw,
and it draws nothing once `s.eFlags` carries `0x100`, which is the explode's
flag (13). vcod's client draws an `eType` 4 the same way
(`EntityVisual::Missile`, `crates/client/src/entities.rs`); a draw off
`s.index` leaves every grenade invisible.

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

VERIFIED, `fire_grenade` 0x544cd-0x54589: `vectoangles` reads the
`velocity` argument (`edi`), not the truncated `trDelta`, and
`AngleNormalize360` (0x3eb30) scales by 182.04445 (.rodata 0x72ba4),
truncates under a `| 0xc00` control word, masks to 16 bits and scales back
by 0.005493164 (0x72ba8), so the launch pitch is a whole number of
65536ths. VERIFIED, `net/snapshots-skyfrag.bin`
(`client-probes/probe_skyshot` sky part with `fraggrenade_mp`, the recipe in
the probe's header with `--probe-uo-weapon throw --save-snapshots
--capture-tag skyfrag`): a frag thrown 80 degrees up left with `trDelta`
(117, 117, 1065) and `apos.trBase` (158.88977, 45, 0), the pitch exactly
28925 65536ths. INFERRED: 158.88977 is 278.88977 less 120, a pitch of
81.11 degrees up, which is the unrounded throw (117.8, 117.8, 1065.x); the
truncated delta reads 81.17 (angles off the truncated velocity, wrapped
without the 65536ths, read 158.82935).
`missile_edge_ab::launch_angles_match_retail_on_1_1` matches the capture bit
for bit.

### 11.3 Where the throw's origin and velocity come from

VERIFIED, `FireWeapon` `0x68D68`: it takes the client's view angles from
`client+0xC0`, then overwrites pitch with `client+0x220C` and yaw with
`client+0x2210`, calls `AngleVectors` for a forward/right/up basis, sets the
start point to `self->r.currentOrigin` with `client+0xD0` added to z, calls
`G_AddLean` on it, and truncates each of its three components toward zero.
INFERRED: `client+0xD0` is the view height, since `CanDamage` adds the same
field to the same origin for the same purpose (14.3). INFERRED: that start
point is the muzzle, since the same local is what `FireWeapon` hands
`Bullet_Fire_Extended` as the trace start (2.3). INFERRED: inside a cmd
`r.currentOrigin` is the snapped `s.pos.trBase`, so the throw leaves from a
whole-unit origin plus the view height (2.1).

VERIFIED, `FireWeapon` `0x69003`-`0x6909F`: on the `weaponType == 1` arm it
builds `velocity = forward * (float)weapDef->projectileSpeed (0x314)` with
`(float)weapDef->projectileSpeedUp (0x318)` added to z, calls `fire_grenade`
with that start point and that velocity and `self->s.weapon`, then normalizes
the velocity in place and adds `n * dot(client->ps.velocity, n)` to the
returned missile's `s.pos.trDelta`, where `n` is the normalized velocity.
INFERRED: the thrower's own motion along the throw direction is therefore
added to the grenade after `fire_grenade` has truncated the delta, so the
final `trDelta` is not whole-numbered. VERIFIED, 0x6900c-0x6909c: z is
`projectileSpeed * forward.z` plus `projectileSpeedUp` on the stack, stored
once; `VectorNormalize` runs on the unrounded velocity, and the dot and
each `n * dot` stay on the stack until each `trDelta` component is stored.
VERIFIED, UO's `weapon_grenadelauncher_fire` (0x7b2c4) has the same shape
with the dot stored as a float. vcod: `missile::throw_velocity` returns
the unrounded velocity and `Missiles::carry` does the truncation and the
add, so `fire_grenade` gets what retail's does.

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

### 11.4 When a throw spawns, and when it first moves

VERIFIED, `fire_grenade` (`0x543AC`): it stamps `s.pos.trTime` (`0x544b6`)
and `s.apos.trTime` (`0x54565`) with `level+0x1E8`, and `nextthink` with
`level+0x1E8` plus `grenadeTimeLeft` (`0x543d3`) or plus `0x9C4`, 2500
(`0x543e0..0x543e5`). VERIFIED: `G_RunFrame` (`0x50478`) stores
`level+0x1E8` into `level+0x1EC` and its argument into `level+0x1E8`
(`0x50487..0x50499`), and `gettime` answers `level+0x1E8`
(`cod11-gsc-object-model.md` 23.1). INFERRED: `level+0x1E8` is
`level.time` and `level+0x1EC` `level.previousTime`.

VERIFIED: the calls `fire_grenade` makes are `G_Spawn`,
`BG_GetInfoForWeapon`, `Scr_SetString`, `vectoangles`,
`AngleNormalize360` and `flrand` twice, and the calls `FireWeapon`'s
grenade arm makes are `fire_grenade` (`0x6904d`) and `VectorNormalize`
(`0x69058`): neither links the entity or calls `G_RunMissile`. VERIFIED:
`G_RunMissile`'s one caller is `G_RunEntity` (`0x502bc`, call at `0x50375`
on `s.eType == 4`), which `G_RunFrame` calls for every `inuse` entity from
the loop at `0x50930` (`0x50949`, `0x50955`), past its clock stores.

INFERRED, off 16.1's packet order and the above: a throw runs inside its
release cmd, between frames, on the previous frame's `level.time`; the
missile does not move in that cmd, and its first `G_RunMissile` is the next
`G_RunFrame`'s, which evaluates the arc one frame past `trTime`. The fuse
counts from the same previous-frame `level.time`.

VERIFIED, the pair capture `mp_carentan-tdm-grenade-shooter.txt`: the first
throw's missile first reaches the wire on the snapshot at `serverTime`
1193050 reading `trTime` 1193000, `trBase` (1443, 3394, 37) and `trDelta`
(-873, -342, -85), and its `origin` there, 1399.3 on x, is the arc evaluated
50 ms past `trTime`; the second reads `trTime` 1202250 on the snapshot at
1202300 and `trBase` (1449, 3399, 37). The thrower's feet read
(1443.8, 3394.7, -23.9) and (1449.0, 3399.2, -23.9) on those snapshots and
its velocity is zero, which is why both deltas are whole (11.3).

**As implemented.** `Server::replay_moves` takes the grenade's fire event
off the cmd like a shot's and `Server::throw`, beside `fire`, runs
`throw_velocity` on the thrower's sim as that cmd left it, before the cmd's
touch pass, and spawns the missile stamped with the frame before the one
being built; the missile pass in `tick` then flies it on the new frame's
clock, as it does every other missile. A thrower killed by a later cmd of the
same tick has thrown already. `combat::muzzle_point` truncates the origin
before adding the eye height and lean (2.1), which moves every shot and
swing as well as the throw. Pinned by
`a_throw_s_first_frame_matches_retail_s_capture`
(`crates/server/tests/combat.rs`), which puts a thrower where each of the
capture's two throws stood and holds `trTime`, `trBase` exactly and
`trDelta` to within two units of it, and
`a_throw_leaves_from_its_own_cmd_not_the_tick_s_last`
(`crates/server/src/server.rs`), a run queued behind the release in the same
tick.

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
- VERIFIED, the disassembly: 1.1d ORs the 0x20 in at 0x540a0 and pushes
  `0xAD` at 0x54115; 1.5b ORs it at 0x54ccb and stores `0xAD` at 0x54d8d,
  the same arm. United Offensive's `G_RunMissile` ORs it at 0x5f08b and
  stores `0xB2` (178, its `EV_BULLET_HIT`) at 0x5f14d, and also writes the
  missile's `s.weapon` into the temp entity's (`+0xCC`); when the weapon's
  `projExplosionType` (`+0x390`, 0x5f1f3) is 1, `smoke`, it then calls
  `G_FreeEntity` (0x5f202) and returns, before the re-trace at 0x5f23f.
  INFERRED, off the decompiles: in all three the `d[2]` fix multiplies a
  negative z by a `.rodata` constant, which the captures below read as -1.
  VERIFIED: the `+216` (`+0xDC` on UO) field is `_union.scale` in the
  entity netfields (`crates/common/src/net/cod1/fields_v1.rs`, `fields_v22.rs`).
- VERIFIED, live (`crates/server/tests/fixtures/missiles/
  mp_harbor-water-*.txt`, `client-probes/probe_skyshot` spot part on
  mp_harbor's south sea wall over the harbor; the water's top is z -120,
  material `textures/common/water`, surface flags 0x14040a0): a 1.1d
  panzerfaust round 45 degrees down raises temp entity 74, event 173, parm
  5 (straight up), `otherEntityNum` the rocket, `surfType` 20, `scale` 7,
  at -90, its origin on the frame before the move that crosses -120, and
  explodes on the harbor floor the next move as if the water were not
  there. A frag thrown flat raises the same at -103 (`trDelta` z is the
  throw's 120, past 30, though the grenade is falling by then) and then
  bounces along the floor; its flight matches ours to the hundredth. On
  retail UO 1.51b the same throws raise 178 with `weapon` set (5 the
  bazooka, 18 the frag, 52 the smoke grenade), and the smoke grenade
  leaves the wire on the snapshot that carries its splash, never popped.
  VERIFIED, live (retail 1.5b, `mp_harbor-water-{rocket,frag}
  -15.txt`, same spots): 173 off the same arm, parm 5, `surfType` 20, the
  frag's at -102 with `scale` 32 and its whole flight ours to the
  hundredth; that rocket's splash move, from -93.28, also reaches the
  floor, so its splash and its explode go out on one snapshot.
  `a_rocket_into_water_splashes` flies each server's rocket on retail's
  `trDelta` (off the script's first two positions), so the splash, its
  `scale` and the explode are held exactly on all three.
  vcod: `CollisionWorld::water_hit` (the water brushes with each side's
  surface flags) and `Missile::step`'s `wet` arm, the temp entity spawned
  on the missile's turn through `Missiles::take_splashes`; gated by
  `crates/server/tests/missile_edge_ab.rs` (`a_frag_into_water_splashes`,
  `a_rocket_into_water_splashes`, `a_uo_smoke_grenade_into_water_is_freed`)
  and `a_missile_entering_water_fast_splashes`.
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
- VERIFIED, `game.mp.i386.so` read with `tools/re/annotate_func.py`: the
  snap's trace (the call at 0x542a4) pushes `bulletPriorityMap` (0x54285),
  `r.ownerNum` and `ent->clipmask` like the move trace, so it scores a
  player's posed bones too. VERIFIED, live (12.4, "Resting on a player"):
  a frag on a prone player's back snaps to his bones, not past them to the
  floor. vcod runs the snap through the same body, script model and world
  trace as the move (`Missile::full_trace`).
- VERIFIED, `game.mp.i386.so` 1.5b: after the snap's `G_MissileTrace`
  (call at 0x54fe0) the snap applies only when the fraction is below 1.0
  and `tr.entityNum` is 0x3fe (`cmpw $0x3fe` at 0x54ffe); VERIFIED, UO
  `game.mp.uo.i386.so`: the same compare at 0x5f44d. VERIFIED, 1.1d:
  nothing between the fraction test after the call at 0x542a4 and the
  store. So 1.5 and UO snap a missile down onto the world only, never onto
  a body, a script model or a brush entity. VERIFIED, live (retail 1.5b, `mp_carentan-scriptmodel-prone-rest-1.5.txt`): the frag that
  strikes the prone target's back takes the strike point itself as its
  base, z -10.667, on the move's chord. INFERRED, from the frames of
  `mp_carentan-scriptmodel-prone.txt` (1.1d): its strike at z -4.74 sits
  1.49 above the chord between the frames either side. vcod keys the test on `Missile::falls`
  (`the_15_ground_snap_takes_the_world_only`); a body snap on 1.5 sets the
  frag 1.5 above his back.
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
  VERIFIED, `game.mp.i386.so` read with `tools/re/annotate_func.py`: the
  test is `test [ebp-0x2c], 0x10` at 0x5432f ahead of the `G_FreeEntity`
  call at 0x54339; 1.5b's is `and eax, 0x10` at 0x550e4 before its free at
  0x550f1. VERIFIED, the material lumps of every stock 1.1 and UO map
  (`crates/common/examples/flag_census.rs`): the only clip-brush materials
  carrying 0x10 or 0x4 are the `textures/sky/*` ones, every one surface
  0x434 and contents 0x801 (0x8000801 for `sky/sewer`). VERIFIED, live
  (retail 1.1d, `fixtures/missiles/mp_carentan-skyshot-rocket.txt`,
  `client-probes/probe_skyshot`): a panzerfaust round fired 80 degrees up
  climbs 137 a frame to z 2225.8 and is gone from the script's entity list
  and the wire on the frame its move meets the sky at 2304, with no explode.
- **United Offensive.** VERIFIED, `game.mp.uo.i386.so`: right after the
  move trace `G_RunMissile` tests `surfaceFlags & 0x4` (0x5f247) and on it
  adds -2.0 (`.rodata 0xa180c`) times the plane's normal to the end point,
  before the trigger re-trace and the store into `r.currentOrigin`; at the
  end of the move the same 0x4 test (0x5f533) jumps past both the 0x10 free
  (0x5f541) and `G_MissileImpact` to `G_RunThink`. VERIFIED: its
  `G_MissileTrace` (0x5edae) clears a start-solid (`tr+0x2f`) and sets the
  fraction to 1.0 when the trace's contents carry 0x800 (0x5edf4..0x5ee13),
  and otherwise zeroes the fraction of a start-solid trace. INFERRED, off
  the stores: a UO missile that meets the sky is put 2 units inside the sky
  brush, and the next frame's trace, starting inside it, reads clear, so the
  missile flies on above the map. VERIFIED, live (retail UO 1.51b, `mp_carentan-skyshot-rocket-uo.txt`): the bazooka round's
  origin reads z 2305.88 on the contact frame, 2 past the face at 2303.88,
  climbs on at 137 a frame on its line, and goes off on its 15 s think (13.6)
  with 185 and parm 0 at z 41318, out past the map. vcod: `Missile::step`
  (`SURF_SKY`, `SURF_NOIMPACT`, `UO_SKY_PUSH`, `CONTENTS_SKY`), pinned by
  `a_missile_at_the_sky_is_freed_silently`,
  `a_uo_missile_flies_through_the_sky` and
  `crates/server/tests/missile_edge_ab.rs`.
- VERIFIED, the disassembly: on any other start-solid UO's `G_MissileTrace`
  stores a fraction of 0 (0x5ee1e, 0x5ee27) and `VectorNormalize2(start -
  end)` into the normal; 1.5b's (0x54a18) does the same at 0x54a5e with no
  sky arm. 1.1d has no `G_MissileTrace`: its `G_RunMissile` zeroes only the
  fraction, after the store into `r.currentOrigin` (0x5420c), and UO's
  repeats that test at 0x5f367. INFERRED: a move that starts inside a brush
  is a contact at its end point, which 1.5 and UO bounce straight back
  along the move. vcod: `Missile::full_trace` under `falls` and the zeroing
  in `Missile::step`, pinned by `a_start_solid_move_is_a_contact`.
  VERIFIED, live (retail 1.1d and 1.5b, mp_carentan,
  `fixtures/missiles/mp_carentan-kerb-rocket{,-15}.txt`,
  `client-probes/probe_skyshot`'s spot part at (1192, -536, 340)): with the
  shooter's eye inside the 0x2080 caulk brush (1164..1220, -544..-528,
  366..430), a panzerfaust round fired flat along +y goes off at the end of
  its first move, 279 units out in open air, and reaches the wire already
  exploded (`eType` 0, `EV_ROCKET_EXPLODE`) on the launch snapshot. 1.1d's
  explode packs parm 0, the zero normal of a trace that started solid and
  met nothing after; 1.5b's packs 104, the byte of the reversed flight.
  INFERRED: 1.1's brush would not stop a 0x11 move, so the contact also
  confirms 11.1's clipmask. `missile_edge_ab`'s
  `a_rocket_fired_inside_a_kerb_brush_goes_off_at_once` replays both, the
  round flying retail's `trDelta` (the one integer delta whose
  `vectoangles` is the capture's `apos`); under the old 0x11 mask it
  fails on both.
- Otherwise `G_MissileImpact(ent, &tr, dir)` runs, and `G_RunThink(ent)`
  follows only when `s.eType` is still 4. VERIFIED: `G_MissileImpact`'s
  explode path sets `s.eType` to 0 (13.1), so the test is what stops a
  detonated missile from thinking again in the same frame.

### 12.3a What a script reads as a missile's `origin`

VERIFIED: the entity field table's `origin` sits at gentity offset 308
(0x134, `crate::game::fields`), so `ent+0x13c` is its z, and
`G_RunMissile` (0x53fcc) stores to `ent+0x13c` at 0x54206 and 0x542e6 ahead
of its `trap_LinkEntity` (0x542f0). INFERRED: a script reading a grenade's
`origin` reads `r.currentOrigin` as this frame's move left it. vcod copies
each live missile's origin into the entity's script `origin` at the throw
and after every missile pass (`Missiles::sync_origins`).

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

**1.5 and United Offensive.** VERIFIED, `game.mp.i386.so` 1.5b
(`G_BounceMissile` 0x53d71, GOT 0x88d98): with `s.eFlags & 0x02000000`
set and neither the water nor the body test true, both arms of the
`0x01000000` test scale the whole reflected `s.pos.trDelta` by 0.5
(`flds -0xdbec(%ebx)`, .rodata 0x7b1ac, at 0x53f31 and 0x53f6f); the soft
arm keeps 1.1's 0.125 (0x7b1a8), and the reflection's -2.0 (0x7b198), the
0.7 ground test (0x7b1a0, a double), the rest speed 20.0 (0x7b1b0), the
0.1 nudge (0x7b1b4) and the 100.0 event gate (0x7b1bc) are 1.1's. 1.5
has no tangential and normal split. United Offensive scales the same way
by 0.4 and 0.1 (coduo-diff.md 21, "Missiles"). VERIFIED, live (retail
1.5b, mp_carentan, `mp_carentan-scriptmodel-prone-rest-1.5.txt`
and `mp_carentan-scriptmodel-prone-1.5.txt`): the frag that falls off the
prone target (12.6) strikes the floor at z -22.375 and is airborne two
snapshots before it bounces again and rests, a rise of 1.0 at the first
(40 units/s off the 80 it struck at); the second frag's floor and wall
bounces land on the same snapshots. `Missile::bounce` scales by
`BOUNCE_15` (1.1's split here makes the fallen frag rise 0.21 and rest a
snapshot early, and the wall rebound reach z -16.84 against -2.64);
`a_frag_rests_on_a_prone_players_back_on_15` holds every snapshot of both
frags to retail's `trType` and ground, each base within 0.3.

INFERRED: the body test reads the struck entity's contents, not whether
it is a player, so a script model a script `setContents`es to 0x2000000
takes the soft scale too. VERIFIED, `tools/re/dump_builtins.py`: the
method exists on all three (1.1d 0x5dcec, 1.5b 0x5dc3c, UO 0x696b9); no
stock script passes it more than 1 (coduo-diff.md 21). vcod reads the
struck model's link contents (`PlacedModel::body`); its bounce stays
silent only in water and off a player, as before.

**The bounce off a player.** VERIFIED, `game.mp.i386.so`, read with
`tools/re/annotate_func.py`: `G_RunMissile` pushes `bulletPriorityMap` on
both arms of the move trace (0x54095 and 0x540a7, the call at 0x540c6) and
for the trace at 0x54187 (0x54168). VERIFIED,
`game.mp.uo.i386.so`: `G_MissileTrace` (0x5edae) passes it too, through
the GOT slot 0xb45cc (`R_386_GLOB_DAT bulletPriorityMap`), read at
0x5edc0. VERIFIED, live (retail 1.1d, mp_carentan,
`probe_scriptmodel`'s grenade part, second throw): a frag thrown at an axis
player standing 160 units off flies into his link box unchecked (frames at
x 1468.36 and 1489.92 where the box's near face is at x 1466), strikes a bone
near his back at 1490.48, 2730.42, -12.31 and leaves at an eighth of its
speed, on a heading the bone's face gives it (1.0, -3.3 units a frame), to
rest at 1496.00, 2712.55. A second retail run put the strike at 1490.48,
2730.41: the bone's pose rides the player's idle clip. INFERRED: the
segment scores the posed bones under `bulletPriorityMap` (3.1), the
`trace_t` normal is the struck box face's (3.2, `+16`), and that normal is
what the soft damping above reflects about.

**Resting on a player.** VERIFIED, `game.mp.i386.so`: the
`groundEntityNum` store at 0x5388a takes the word at `tr+0x28`
(`tr.entityNum`), behind the `tr.normal[2] > 0.7` test at 0x53879.
VERIFIED, live (retail 1.1d, mp_carentan,
`probe_scriptmodel`'s prone part, fixture
`crates/server/tests/fixtures/scriptmodel/mp_carentan-scriptmodel-prone.txt`):
a frag thrown onto a prone player's back, 20 past his origin, bounces once
with `groundEntityNum` 0 (his number) on the wire, flies on with 1023, and
rests on him at (1459.67, 2702.32, -1.35), 22.5 above his origin, with
`groundEntityNum` 0. The second frag, thrown with him moved off, rests on
the floor with 1022. VERIFIED, live, mp_brecourt (fixture
`mp_brecourt-scriptmodel-prone.txt`): 0 on him on a slope, 1022 on the
terrain. VERIFIED, live, mp_brecourt with the target lying still (fixture
`mp_brecourt-scriptmodel-prone-rest.txt`; the older capture dropped him
onto the grade every frame, player-model-anim-system.md, "The prone body
on a grade"): the first frag strikes his back with 0, bounces on with
1023 and rests on him at (1657.12, 817.81, -80.92) with 0. vcod writes the
struck entity (the player, the script model, or
`CollisionWorld::entity_num`). `scriptmodel_ab`'s prone gates hold both throws: every frame
within a unit of retail's on carentan (his prone pose strikes half a unit
off retail's) and on the brecourt retake, each frag's resting ground, and
the brecourt retake's rests within a unit. INFERRED: 1.1 never reads a
missile's `groundEntityNum` again (12.6), so on 1.1 it changes the wire
field only.

vcod: the move trace (`game::missile`) runs `combat::trace_bodies_faced`
on every live body but the owner's while it passes, with the bone trace's
struck face as the normal (`bonetrace::BoneHit::normal`, the entry face's
axis turned by the bone and the body); the link box as the contact stops a
missile 25 units short of retail's strike. `scriptmodel_ab`'s grenade gate holds every frame of the
throw to 0.1.

### 12.5 The point trace against terrain

VERIFIED, `cod_lnxded` 1.1d: a point trace (`traceWork+0xc0`, Q3's
`isPoint`, set) clips a terrain or patch partition in 0x8052894, called
from 0x80536e4 (the trace) and 0x80537d8 (a copy of the trace work with
the fraction reset to 1). Per triangle record (0x48 bytes from the
partition's `+4`: normal at `+0`, dist `+0xc`, the two barycentric planes
`+0x10`/`+0x1c` and `+0x20`/`+0x2c`), against the trace work's float
start (`+0`), end (`+0xc`) and delta (`+0x18`). VERIFIED: the offsets,
constants and addresses in the list below. INFERRED: its order and every
"skipped unless" in it, which are branch conditions.

- `d2 = DotProduct(end, normal) - dist`; the triangle is skipped unless
  `d2 < 0`.
- `d1 = DotProduct(start, normal) - dist`; skipped unless `d1 > 0`.
- `f = (d1 - 0.125) / (d1 - d2)` (0x80cd2f8, 0.125); skipped unless
  `f < trace.fraction` (`+0xc4`).
- `p = start + delta * (d1 / (d1 - d2))`; `a = DotProduct(p, u) - ud`,
  skipped below -0.001 (0x80cd2fc) or above 1.001 (0x80cd300); `b` the
  same off the second plane, skipped below -0.001 or when `a + b` is above
  1.001.
- `trace.fraction` takes `f` (`fstps` at 0x80529fb) and `trace.normal`
  the record's normal (0x8052a04-0x8052a15).

VERIFIED: nothing between the loads and that `fstps` stores to memory, so
every distance, the fraction, the hit point and both barycentrics stay on
the x87 stack at 64-bit precision; the products are summed `(x + y) + z`;
`f` is not clamped at 0. VERIFIED: the records are the capsule clip's,
`PlaneFromPoints` (0x8064fec) called on the triangle's first, second and
third point (0x80520e6-0x805213e), `dist` the first point's dot with the
stored normal (0x80650c5). United Offensive's point clip
(coduo-vehicles.md 12) rounds each of those steps to a float.

vcod: `Terrain::point_clip` is 0x8052894 in `f64`, which an exact
64-bit-mantissa emulation of the routine matched on every terrain trace of
the mp_brecourt run below (`the_point_clip_rounds_only_the_fraction_it_stores`
pins one case). Trap: clipping terrain as a static model triangle (a normal
recomputed from the corners, distances off a corner) lands about 20 float
steps off in the fraction. VERIFIED, live (retail 1.1d, mp_brecourt, fixture
`mp_brecourt-scriptmodel-prone.txt`): the prone part's second frag
bounces down the field's terrain and rolls to rest on it; ours lands every
contact on retail's snapshot with retail's `groundEntityNum`, every
`trBase` retail's to the bit (below; `a_frag_rolls_down_terrain_like_retail`
compares the bits).

**The x87 arithmetic of the bounce and the arc (1.1d).** Read in the
disassembly of `game.mp.i386.so` 1.1d. VERIFIED: none of the steps below stores an
intermediate, so each sum and product stays on the x87 stack and only the
named stores round to a float.

- VERIFIED: `BG_EvaluateTrajectory`'s gravity arm (0x2c6c0): `dt` is the integer ms
  times the float 0.001 (`.rodata 0x7008c`). `x` and `y` are `base +
  delta * dt`, and `z` is `(base + delta * dt) - 400 * dt * dt`
  (`.rodata 0x70098`). Each is rounded once.
- VERIFIED: `BG_EvaluateTrajectoryDelta`'s gravity arm (0x2c964): `delta.z - dt *
  800`, with 800 at `.rodata 0x70120`, rounded once.
- VERIFIED: `G_BounceMissile`'s impact time (0x537f4): the integer frame length
  times `tr.fraction`, truncated by `fistp` under control word `| 0xc00`.
- VERIFIED: The reflection (0x53828-0x53873): the dot is `(vx nx + vy ny) + vz nz`
  and `-2 dot` stays on the stack. Each `trDelta` component is stored
  once.
- VERIFIED: The grenade arm (0x538b7-0x53942): `t = stored reflection + n dot` is
  `fst`ored and kept on the stack. Then comes `t * 0.75`, which is stored,
  then `- n * dot * 0.3`. Each component is rounded once, at its last
  store.
- VERIFIED: The rest test (0x5396e) and the event gate (0x53a8b): the sum of
  squares is on the stack and its root is stored before the compare. The
  event gate stores each component of `newDelta - v` first.
- VERIFIED: The nudge: each `0.1 * n` component is stored.
- VERIFIED: `G_RunMissile`'s ground snap (0x542bd): `E = endpos.z + 1.5` stays on the
  stack. `trBase.z` takes `(E - currentOrigin.z) + trBase.z` and
  `currentOrigin.z` takes `E`, each rounded once.
- VERIFIED: The 0.001 move test (0x54034) reads `VectorNormalize`'s length off the
  stack.

vcod: `Trajectory::evaluate_kept`, `missile::delta_at` and
`missile::bounce_kept` port these in `f64`. VERIFIED: `BG_EvaluateTrajectory`'s
`TR_LINEAR` arm (0x2c640) keeps `dt` and each `base + delta * dt` on the
stack the same way, so `evaluate_kept` covers a line too (a rocket's
`pos`, a grenade's `apos`). They run under 1.1's rules
only (`Missile::kept`).

**The arithmetic on 1.5 and UO.** Read in the disassembly of 1.5b's
`game.mp.i386.so` and UO's `game.mp.uo.i386.so` with
`tools/re/annotate_func.py`. The two builds are the same -O0 code at
different addresses: `BG_EvaluateTrajectory`, `G_BounceMissile` and
`G_RunMissile`'s snap have the same length and the same instruction
sequence (UO's `G_BounceMissile` 0x5d6e5 is 0x478 bytes, like 1.5b's
0x53d71). 1.5b addresses below, UO's beside them
where read. Each named store rounds to a float; the rest stays on the
stack.

- VERIFIED: `BG_EvaluateTrajectory` (1.5b 0x1b877, UO 0x1fe63) stores
  `dt`, the ms times 0.001 (`.rodata 0x74e8c`), as a float (0x1ba87). The
  gravity arm (0x1ba6c, UO 0x20058) stores each `base + delta * dt`, then
  takes `z - dt * 400 * dt` (400 at `.rodata 0x74e9c`) off the stored `z`
  and stores it again (0x1bade-0x1baf0). The line arm (0x1b8da) is the
  first half alone.
- VERIFIED: `BG_EvaluateTrajectoryDelta`'s gravity arm (0x1c002) stores
  `dt` the same way (0x1c01d), then `delta.z - dt * 800`, rounded once
  (0x1c053-0x1c062).
- VERIFIED: `G_BounceMissile`'s impact time is 1.1d's: the frame length
  times `tr.fraction` on the stack, truncated (0x53dc8-0x53de6).
- VERIFIED: the dot is summed on the stack and stored (0x53e33, UO
  0x5d7a7). Each `trDelta` component is `dot * -2.0 * n + v`, rounded once
  (0x53e3c-0x53e87). The 0.125/0.5 (UO 0.1/0.4) scale is a float times a
  float, stored.
- VERIFIED: the rest test (0x53fbc) and the event gate (0x54180) are
  1.1d's: the change's components stored, the sum of squares on the stack,
  passed to `sqrt` as a double, the root stored.
- VERIFIED: the nudge stores each `0.1 * n` component, then each
  `currentOrigin + d` (0x54057-0x540de).
- VERIFIED: `G_RunMissile`'s 0.001 move test reads the stored-length
  `VectorNormalize` (0x54c61; `x87::normalize_stored`).
- VERIFIED: the ground snap (0x5501f-0x55073, UO 0x5f477) is 1.1d's:
  `trBase.z` takes `((endpos.z + 1.5) - currentOrigin.z) + trBase.z`,
  rounded once, and `currentOrigin.z` takes `endpos.z + 1.5`.
- VERIFIED: UO's sky push (0x5f24e-0x5f284) is `n * -2.0 + endpos`, each
  component rounded once, which a float add of the exact `2 n` gives.

VERIFIED, live (retail 1.5b, mp_brecourt,
`mp_brecourt-scriptmodel-roll-1.5.txt`, `probe_scriptmodel`'s prone
part): both frags miss the target and bounce up the field's terrain over
four contacts to rest. With the stores above every base is retail's to the
bit (plain `f32` puts every base from the second contact a float step off in
`y`). VERIFIED, live (retail 1.5b, `mp_carentan-scriptmodel-throw-1.5.txt`):
the second frag's wall bounce at x 1583.775 is exact with them (4 steps off
in `z` without). VERIFIED, live (retail UO, mp_brecourt,
`mp_brecourt-scriptmodel-roll-uo.txt`): the second frag's four terrain
contacts are retail's to the bit either way, as are mp_foy's,
mp_dawnville's, the carentan throws' and the harbor water frag's. VERIFIED, live (retail UO,
`missiles/mp_brecourt-frag-roll-uo-45.txt`, `-135.txt` and
`mp_uo_hurtgen-frag-slide-uo.txt`, `client-probes/probe_skyshot`'s spot
part): three frags land on terrain (lump-26 triangles under every
contact) and roll to rest, the hurtgen one sliding down a face of normal
z 0.59, too steep to snap or rest on, struck every frame over 21 floor
contacts. Replayed from each launch's wire `pos` and `apos`, every
snapshot to the explode carries retail's `pos` base and delta, ground and
`apos` to the bit under the stores above; under 1.1d's arithmetic (the
evaluate, the dot, `delta_at` and the move length kept) all three part
from retail at their first contact. A frag striking a body (the first frag in
the UO brecourt capture, the carentan prone captures) still lands a few
float steps off in `z`: the bone trace's own stores, which vcod does not
port (`cod11-turrets.md` 13.5).

vcod: `Trajectory::evaluate_stored`, `missile::delta_at` and
`Missile::bounce` port these under 1.5's and UO's rules (`!Missile::kept`).
`scriptmodel_ab`'s `a_frag_rolls_up_terrain_like_retail_on_15` and
`_on_uo` compare the bits, and `game::missile`'s
`uo_frags_roll_on_brecourts_terrain_like_retail` and
`a_uo_frag_slides_down_hurtgens_terrain_like_retail` replay the three UO
flights above. The replay starts at the launch's wire state, not at a
thrower: the spot part's thrower lands where his cmds' timing puts him,
so retail's muzzle point moves by up to a unit and a half between runs on
a grade.

A held pitch of exactly 30 puts the throw's `trDelta.z` on a whole unit,
and which side of it the throw lands on rides the cmd that releases.
VERIFIED, retail UO 1.51b and vcod-server measurement (mp_brecourt, `probe_skyshot` spot part at (1659, 685, -102.76), yaw 45,
`probe_pitch -30`, `--probe-uo-weapon throw`): a release on the cmd sent
with a snapshot, the first to run after that frame's `setPlayerAngles`,
left at `trDelta` (587, 587, 600) on retail three runs out of three and
on vcod once; a release one cmd later (retail with the probe's step clock
pushed 8 and 12 ms, vcod six runs on its own spawn timing and one pushed
38 ms) left at (587, 587, 599) on both. VERIFIED, game.mp.uo:
`SetClientViewAngle` (0x54301) writes the script's angles into
`ps.viewangles` (`ps+0xe8`) unrounded and only `delta_angles` on the
short grid; `PM_UpdateViewAngles` (0x3b15c) rewrites the view as
`(360 / 65536) * short`; `ClientThink_real` (0x50c91) copies
`ps.viewangles` plus `BG_CalculateViewAngles`' kick (zero here) into
`client+0x4650`, which `CalcMuzzlePoints` (0x8bc58) turns through
`AngleVectors`. INFERRED: only the first cmd after the script reads 30;
every later one reads -5461 shorts, 29.998169, whose `sin` puts z at
599.97. The 599 against 600 noted here before was the thrower's release
landing on a different cmd of the frame, not arithmetic (`game::missile`'s
`a_held_pitch_of_30_throws_600_on_the_first_cmd_and_599_after`).

### 12.6 1.5's and UO's fall test

VERIFIED, `game.mp.i386.so` 1.5b, read with `tools/re/annotate_func.py`:
`G_RunMissile` (0x54abd) opens with a test 1.1's has not. When
`s.pos.trType` (`+0xc`) is 0 and `s.groundEntityNum` (`+0x7c`) is not
0x3fe (the world), `G_MissileTrace` runs from `r.currentOrigin` (`+0x138`)
to 1.5 below it (0x7b1d0) with `r.ownerNum` (`+0x150`) and `clipmask`
(`+0x198`). INFERRED: when that fraction is exactly 1.0 (0x7b1d4), `s.pos`
takes `trType` 5 (`TR_GRAVITY`), `trTime` `level.time`, `trDuration` 0,
`trBase` the current origin and `trDelta` 0 (0x7b1d8), and the frame goes
on to the move. VERIFIED, `game.mp.uo.i386.so`: UO's `G_RunMissile` opens
with the same compare against 0x3fe and the same store of 5 into
`trType`, around its `G_MissileTrace` call at 0x5ef16. VERIFIED: 1.1's
`G_RunMissile` (0x53fcc) begins at `BG_EvaluateTrajectory` and has no
such test (12.1).

VERIFIED, `game.mp.i386.so` 1.5b: `G_MissileTrace` (0x54a18) is
`trap_LocationalTrace` with `bulletPriorityMap`, read through the GOT slot
0x899c4 (`R_386_GLOB_DAT bulletPriorityMap`) at 0x54a2a. INFERRED: on a
start-solid trace it zeroes the fraction and sets the normal to the
normalized `start - end` (0x54a5e-0x54ab0). VERIFIED: every trace in 1.5's
`G_RunMissile` calls it (0x54b56, 0x54cf8, 0x54d32, 0x54e40, 0x54eea,
0x54fe0), so 1.5 scores posed bones under the same map 1.1 does.

VERIFIED, live (retail 1.5b, mp_carentan, the prone part,
fixture `mp_carentan-scriptmodel-prone-1.5.txt`): the first frag strikes
the prone target (`groundEntityNum` 0 on the bounce) and rests on his back
with 0; the snapshot after the script moves him it reads `trType` 5 with
its base unchanged and ground still 0, and it falls to the floor and rests
there with 1022. The same capture's throws leave from z 36.148 where
1.1's leave from 37.0 on the same spot: 1.1 truncates the muzzle point and
1.5 does not (2.1, "1.5 and United Offensive").

vcod: `Missile::falls`, set for 1.5 and UO, runs the test through the
missile's own trace (`a_frag_left_hanging_falls_on_15_and_uo_only`). Ours'
1.5 throw now leaves from 1.5's muzzle (2.1); this capture's thrower's
feet sat at a height ours' never settles at, so its throw leaves 0.25
off ours', and the same gate holds its strike and rest on the target
within 0.3. `mp_carentan-scriptmodel-throw-1.5.txt` replays the prone part
(`a_frag_leaves_15s_untruncated_muzzle`), and
`mp_carentan-scriptmodel-prone-rest-1.5.txt`, which logs the target's
prone body too, holds the rest on him: retail's frag strikes his back at
(1455.86, 2695.97, -10.67) and rests on it at (1469.11, 2713.16, -16.47);
vcod's within 0.12 of each (`a_frag_rests_on_a_prone_players_back_on_15`).
The ground snap onto the world only (12.2) and the airborne prone body
(cod15-movement.md 4) are what put it there.

### 12.7 `G_MissileLandAngles`: the angles a contact leaves

VERIFIED, `game.mp.i386.so` 1.1d, read with `tools/re/annotate_func.py`:
`G_MissileLandAngles(ent, tr, angles, rest)` (0x535d0) has two callers,
both in `G_BounceMissile` (0x537c0): the rest arm with `rest` 1 before
`G_SetAngle` (0x5399c), and the bounce with 0 (0x53a37), whose `angles`
`G_BounceMissile` then copies into `s.apos.trBase` with `s.apos.trTime`
`level.time` (12.4). 1.5b's (0x53ad0, called at 0x54034 and 0x5413f) and
UO's (0x5d444, at 0x5d9a8 and 0x5dab3) have the same branches and
constants but one (below).

- VERIFIED: the impact time is `level.previousTime + (int)((level.time -
  level.previousTime) * tr.fraction)`, the product on the stack and
  truncated (`fldcw` `| 0xc00`, 0x5360a). `BG_EvaluateTrajectory(&s.apos,
  that time, angles)` (0x53621) gives the tumble there. Yaw and roll leave
  the function as evaluated: no wrap. VERIFIED, `net/snapshots-skyfrag.bin`:
  the sky frag's roll reads 893.8452, 1158.6882 and 1250.0592 on its bounces.
- VERIFIED: a contact with `tr.normal[2]` not above 0.1 (`.rodata 0x75af8`,
  0x53640) is a wall. `rest` 0 adds `(rand() & 0x7f) - 63` to
  `s.apos.trDelta[0]` and stores it through `AngleNormalize360` (0x53793);
  the angles are left as evaluated.
- VERIFIED: otherwise `flat = PitchForYawOnNormal(yaw, tr.normal)` (0x5365e)
  and `diff = AngleSubtract(flat, pitch)` (0x53674). With `rest` 0 the
  pitch rate is scaled by a draw, `0.85 - 0.3 * rand() / 2^31` on 1.1d
  (`.rodata 0x75afc` is -2^-31, 0x75b00 0.3, 0x75b04 0.85, so 0.55 to
  0.85), and negated when `|diff|` is under 80 (0x536a4, 0x536d4).
  VERIFIED, 1.5b and UO: their factor is `rand() / 2^31 * 0.3 + 0.85` (1.5b
  `.rodata 0x7b174` 2^31, 0x7b178 0.3, 0x7b17c 0.85, 0x7b180 -1.0 for the
  negation; UO 0xa1774..0xa1780 the same), 0.85 to 1.15, so a floor
  contact can spin the frag up there.
- VERIFIED: then `a = AngleNormalize180(pitch)` (0x53706) and the pitch
  becomes `AngleNormalize360` of: `flat`, or `flat + 180` when `|a|` is
  above 90 (`.rodata 0x75b08`, 0x75b10), when `rest` is 1 or `|diff|` is
  under 45 (0x53719); `a + diff * 0.25` (0x75b14) when `|diff|` is under 80;
  `a` otherwise. INFERRED: a frag lands flat on the floor, the right way up
  or upside down, eases a quarter of the way there when it struck within
  80 of flat, and the pitch rate reverses on the near-flat strikes.
- VERIFIED, `net/snapshots-skyfrag.bin` (1.1d): the five bounces' pitch rates
  read 731.9147 (the throw), -403.78943, 245.17664, -138.76308, 116.83154,
  -66.59565: each floor contact's factor (0.552, 0.607, 0.566, 0.842, 0.570)
  inside 1.1d's band, and its pitch 0 on four bounces and 36.77124
  (6694/65536 of a turn) on the fourth. INFERRED: that one struck at a
  pitch of 49.03, between 45 and 80 off the flat floor, and was eased to
  0.75 of it.
- VERIFIED, `mp_carentan-tdm-grenade.txt`: the committed take's three
  throws rest at pitch 357.9, 357.9 and 353.3, `flat` on mp_carentan's
  sloped floor, and an older take's at 177.9, 357.9 and 173.3, the first
  and last `flat + 180`, with the roll unwrapped (533.7, 599.0 and 298.0;
  499.9, 480.6 and 298.4 on the older take).
- VERIFIED, `game.mp.i386.so` 1.1d `AngleNormalize360` (0x3eb30) and
  `game.mp.uo.i386.so`'s: the angle times 182.04445 stays on the x87
  stack into the `fistp`, not rounded to a float first. 988.76947 is
  179999.993 65536ths, which truncates to 179999 (268.76404); an `f32`
  product rounds it onto 180000 (268.76953). VERIFIED,
  `missiles/mp_uo_hurtgen-frag-slide-uo.txt`: with the `f32` product
  vcod's pitch at the 22800 contact sat one 65536th (80.04639 against
  80.04089) off retail's.
- VERIFIED, `game.mp.uo.i386.so`: UO's floor arm with `rest` 0 also
  stores the evaluated angles into `s.apos.trBase` and the impact time
  into `s.apos.trTime` (0x5d551-0x5d58a) before the draw; its caller
  `G_BounceMissile` overwrites both after the call (0x5dab8-0x5dae2, the
  angles and `level.time`), so the wire never shows it. UO's
  `BG_EvaluateTrajectory` line arm (0x1fec6) stores `dt` (0x1fee1) and
  each `base + delta * dt` (0x1fef8, 0x1ff11, 0x1ff2a).
- VERIFIED, live (retail UO, the three `missiles/*-uo*.txt` roll
  captures): every floor factor lies in 0.85 to 1.15 (0.867 to 1.143
  over 29 contacts, the rate reversed on some and kept on others). VERIFIED:
  UO sends `apos.trBase` as 16-bit angles (`bits -100`, coduo-diff.md 3),
  so the wire carries a pitch or roll as 65536ths of a turn (a roll of
  463.25226 reads 103.24951), and the yaw and roll leave
  `G_MissileLandAngles` unwrapped as on 1.1d. VERIFIED: replayed with the
  kept `apos` evaluate in place of the stored one, the three still match
  every snapshot. INFERRED: the two evaluates differ by far less than a
  65536th, so UO's wire cannot separate them.

vcod: `Missile::land_angles` (`crates/server/src/game/missile.rs`), its
draws off the pool's own state (`Draws`). Retail's `rand()` sequence is
not reproducible, so a gate pins the draws a capture shows
(`Server::pin_missile_draws`): `missile_edge_ab`'s
`bounce_angles_match_retail_on_1_1` replays the sky frag with its tumble
and five factors pinned, and every `apos` ours sends matches retail's bit
for bit (wrapping all three angles, keeping the throw's pitch rate or
resting at pitch 0 are the traps). On UO
`game::missile`'s roll replays (12.5) pin each contact's draw the same way
and compare `apos` through the wire's 65536ths.

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
  `G_SetOrigin(ent, &tr.endpos)` place it. VERIFIED, the disassembly:
  1.1d's `SnapVectorTowards` (0x692d8) runs `frndint` in round-down mode
  where the axis is not below the base and round-up where it is; 1.5b's
  (0x6dd91) and UO's (0x7a745) call `floor` and `ceil` on the same test.
  Q3's truncate-and-add-one is not what runs: it lands a unit off on a
  negative axis. VERIFIED, live (`mp_harbor-water-rocket.txt`): the round
  that meets the harbor floor explodes with its base at z -191 and y
  -6648. INFERRED: the point trace stops at -191.875 over the floor at
  -192, a fraction past y -6648 (vcod's replay strikes at y -6647.35), and
  only `floor` and `ceil` give that base from there. vcod's `snap_towards`
  does the same (truncation puts that explode at -190;
  `a_rocket_into_water_splashes`).
- With `ent->+0x23C` non-zero, `G_RadiusDamage(&tr.endpos, ent, ent->parent,
  (float)ent->+0x23C, (float)ent->+0x240, (float)ent->+0x244, other,
  ent->+0x24C)`. VERIFIED: the ignored entity is `other`, the entity the
  missile hit. INFERRED: the direct-hit victim is deliberately kept out of the
  splash, having already been charged the direct damage.
- VERIFIED, live (retail UO 1.51b,
  `fixtures/missiles/mp_carentan-takedamage-rocket-uo.txt`,
  `client-probes/probe_takedamage`): three bazooka rounds at a
  `setTakeDamage(1)` barrel each give it one `damage` notify, 1000
  `MOD_PROJECTILE` at the contact, and no splash, while the shooter 200
  back takes 35 `MOD_PROJECTILE_SPLASH`. vcod hands the direct `G_Damage`
  to any struck entity with `takedamage` (`GameHost::entity_takes_damage`:
  turrets, UO vehicles, `setTakeDamage` entities, linked damageable
  `func_static`s).
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
  read here says what reads it. VERIFIED: no link call follows its spawn
  (0x53f6f..0x53fcb) and `Concussive_think` has none. INFERRED: it never
  reaches the wire, and what it costs is an entity number for 700 ms (14.7,
  "Entity numbers"); 13.5's probe measured the 700 on the same entity.

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
committed take reads it on all three throws' first `!missile` line: 17700,
27750 and 36400 against release snapshots at 17750, 27800 and 36450
(484950, 494950 and 503650 against 485000, 495000 and 503700 on an older
take; the first capture's throws had already bounced there). The
death-drop capture pins it twice over, `trTime` 1405500 against a death frame
at `serverTime` 1405550. vcod stamps both the thrown and the dropped grenade
that way.

VERIFIED, off the same capture's `!trace` lines: from the first snapshot
whose `grenadeTimeLeft` reads the pullback to the first that carries the
missile is 1000 ms on `cook_release`, 5000 on `pin_out` and 600 on
`throw_down`. VERIFIED, `missile_ab.rs`: the replay reads the same three.
VERIFIED: on the committed take the first traces of `cook_release`
(`serverTime` 16750), `cancel` and `throw_down` already read
`grenadeTimeLeft` 4000 and `weaponDelay` 600, where `pin_out` opens on a
`weaponstate` 0 sample from before its first pressed cmd (an older take had
it the other way round, `pin_out` alone opening on the pullback).
VERIFIED: `to_frag`'s last trace is `serverTime` 16700 at `weaponstate` 0,
so no snapshot is missing between the two steps. INFERRED, off
`CombatProbe::step` in `crates/client/src/probe.rs`: a step's clock starts
on the probe's first iteration after the step before it ended, and a
snapshot already traced is not traced again, so a step's first trace is
whichever snapshot arrives next, before or after its first cmd reached the
server. `missile_ab.rs` therefore times both flights from the pullback
frame; `playerstate_combat_ab.rs` runs retail's cmds on retail's frames
(1.13) and pairs the samples by `serverTime`.

VERIFIED, off the same capture's `!missile` lines: a bounce's `trTime` is a
whole millisecond, so vcod truncates the impact time rather than carrying the
trace fraction into it, and the fraction it snaps the origin back with is the
1.5-unit trace's, not the move trace's. Reproducing the committed arcs to
within 0.1 units needed both.

VERIFIED, `missile_ab.rs` on the committed take: on every one of the three
throws'
`!missile` samples (all 1.1d, through the bounces, the rest and the explode
frames) our `pos` matches retail's field for field at the probe's one
decimal (`traj_str`), `trTime` counted from the pullback (a resting one's
is 0 on both). The gate compares those strings, with no distance
tolerance. `apos` carries draws: its `trDelta` is the throw's random spin
(763.5, 0, 358.7 on retail's `cook_release`; 703.5, 0, 335.9 on an older
take), and retail's pitch rate changes at every bounce (486.7, -411.7,
279.9 on `cook_release`'s first three; 593.7, -474.7, 323.1 on the older
take). Ours runs `G_MissileLandAngles` (12.7), and the gate compares the
part of `apos` no draw moves: `trType`, `trTime`, the yaw and its rate, and
a resting pitch modulo 180. VERIFIED, `missile_ab.rs`: every sample of the
three throws matches; the resting pitches read 357.9, 357.9 and 353.3
(177.9, 357.9 and 173.3 on the older take, the same modulo 180). The draws
themselves are gated off the full-precision sky frag capture (12.7).

VERIFIED, off the same lines: the exploded entity stays on the wire for about
300 ms after the event frame. Retail's `G_RunEntity` frees a
`freeAfterEvent` entity on its first turn more than 300 ms past its event
(0x50309; 14.7, "Entity numbers"); vcod's missile pass does the same
(`EVENT_VALID_MS`).

VERIFIED: the lone capture's second throw comes to rest at `z` 179.3 while its
thrower stands at 144.1, on a stack of `crate_misc1` xmodels at (1193, 1653)
whose `clip_nosight` brushes (contents 0x28031640, tops at 178 and 205) are
outside the 0x2802091 move mask (11.1). VERIFIED: the move trace is `trap_LocationalTrace`
(12.1), the one syscall whose `SV_Trace` walks the static models
(`cod11-mantle.md`, "Static models are clipped as a segment"), so the frag
rests on the crate's own collision mesh, and the bounce parm it carries
(21 on both throws that land there) is that surface's `surf_flags`.
`World::from_bsp` takes the paks for that, and `props::collision_tris`
keeps each surface's word, so vcod's bounce reads 21 too. VERIFIED: the
explode's 16-unit downward trace is a `trap_Trace`, which walks no static
model, so a frag resting on a crate explodes with parm 0 (a zero normal)
where one on world brush packs 5; vcod's explode traces the same way
(`missile.rs`), and `crates/server/tests/missile_ab.rs` compares the parms.

VERIFIED: a missile's `index` is 0 on the wire on both sides, and its
`eFlags` `0x03000000` (11.1) sits above the 24-bit netfield, so nothing of it
travels. The model still registers at map load, which `configstrings_ab` pins.

INFERRED, and the reason there is no direct-hit arm: 13.1's `MOD_GRENADE`
path is gated on `ent->damage`, and `fraggrenade_mp` spells `damage` 0, so a
stock frag bounces off a live player at the soft damping instead of
detonating on it. vcod traces against live players' posed bones (12.4,
"The bounce off a player") and applies that damping; the arm itself is out
for a grenade. A rocket carries `damage`
1000 and takes it (13.6).

---

### 13.5 `grenadeExplosionEffect`, the script explode

VERIFIED, `game.mp.i386.so`: the builtin table entry `grenadeexplosioneffect`
points at `0x5aea4` (`tools/re/dump_builtins.py`, entry 71). It reads its one
vector with `Scr_GetVector(0)`, adds 1.0 to z (`fld1` at `0x5aec6`), calls
`G_TempEntity(origin, 0xB2)` (`0x5aeda`), writes `DirToByte((0, 0, 1))` into
its `eventParm` (`0x5af00`, `+0xa0`), runs `trap_Trace(origin, 0, 0, down,
0x3FF, 0x11)` (`0x5af43`) with `down` the raised origin less `17.0` (`.rodata
0x778EC`) on z, writes `(tr.surfaceFlags >> 20) & 0x1F` into the temp
entity's `surfType` (`0x5af53`, `+0x88`), and calls `Concussive_fx(origin)`
(`0x5af61`). VERIFIED, `Concussive_fx` (`0x54840`): `G_Spawn`, the origin
into `r.currentOrigin`, `think` `Concussive_think`, `nextthink` `level.time +
100` and `+0x274` `(float)level.time + 500.0` (`.rodata 0x75B74`), the same
entity 13.2's `G_ExplodeMissile` builds inline, with no link. INFERRED: unlike
`G_ExplodeMissile` it packs a fixed up normal rather than the trace's and
has no water test, and it damages nothing. VERIFIED: the stock paks' only
mention is a comment in `maps/_fx.gsc` (`pak4.pk3`) saying it was removed.

VERIFIED, `client-probes/probe_concnum.gsc` on the retail 1.1d Linux server,
mp_carentan: a spawn read 299, the call, a spawn 302; one spawn a
frame after it read 303..309 through +350, 300 at +400, 310..315 through
+700, 301 at +750 and 316 on. INFERRED: the temp entity took 300 and the
concussion entity 301; the first went back to the free list on the +350
pass (the temp entity rule of 14.7, "Entity numbers") and the second on the
+700 one (`Concussive_think` passes `+0x274` on +600 and frees on +700).

vcod: `builtins::fx::grenade_explosion_effect` raises the temp entity through
`GameHost::add_temp_entity` and calls `GameHost::spawn_concussive`.
`grenade_explosion_effect_takes_a_temp_and_a_concussion_number`
(`crates/server/src/game/script.rs`) is the probe's table.

### 13.6 The panzerfaust: `Weapon_RocketLauncher_Fire` and `fire_rocket`

Addresses are `game.mp.i386.so` (1.1d), read with
`tools/re/annotate_func.py`.

**The fire.** VERIFIED: `FireWeapon`'s `weaponType == 2` arm calls
`Weapon_RocketLauncher_Fire` (0x68c40) at 0x690b1 with the entity, the
spread in degrees the bullet arm computes and the frame of 2.3 (forward at
+0, right +0xc, up +0x18, muzzle +0x24). VERIFIED: it takes
`tan(spread * pi / 180)` (.rodata 0x79c70, 0x79c78) times 16.0 (0x79c68),
scales `gunrandom`'s pair by it, builds `forward * 16 + right * x + up * y`,
normalizes it and calls `fire_rocket(ent, muzzle, dir)` (0x68d1d). INFERRED:
the direction is the bullet's spread aim of 2.2, at 16 units instead of
8192. VERIFIED: with a client (`ent+0x158`) it then adds `forward * -64.0`
(.rodata 0x79c80) to the client's `ps.velocity` (+0x20..+0x28). INFERRED: the
shooter is pushed 64 units/s back along his view, not along the spread
direction. VERIFIED, `snapshots-panzer-wall.bin`: the shooter, standing
still facing +x, slid back 1.6, 1.1 and 0.4 on x over the three snapshots
from the shot's, then stopped. INFERRED: that is 64 units/s spent by
ground friction.

**The direction's bits.** VERIFIED, 1.1d (0x68c40..0x68cf4): the tangent
goes through a double and a float (0x68c62, 0x68c70) as in
`Bullet_Endpos`, and `k`, both offsets and each component's
`forward * 16 + right * x' + up * y'` stay on the x87 stack until the
component's one store. VERIFIED, 1.5b (0x6e6bc) and UO (0x7b3da): `tan`
in double stored as a float (UO 0x7b40e), `k` stored (0x7b417), `x * k`
and `y * k` stored (0x7b432, 0x7b43b), `forward * 16` stored (0x7b446),
then `+ right * x'` (0x7b46f) and `+ up * y'` (0x7b4a2) each stored, and
the stored `VectorNormalize` (0x3a1c3). VERIFIED: `fire_rocket`
normalizes the direction once more before it scales it by
`projectileSpeed` (1.1d 0x54627, 1.5b 0x5559a, UO 0x5fc8c), in place, so
the launcher's local holds the twice-normalized direction once
`fire_rocket` returns. On UO the second normalize moves a last bit: gdb on
retail UO 1.51b (`probe_uofx` with `bazooka_mp` from the hip
along (0, 0), and `probe_skyshot`'s sky part twice along (280.15,
46.25)) read the point before the first normalize (0x7b4c7), the
direction and the rocket's `trDelta` after `fire_rocket` (0x7b508), and
one normalize of the second and third rounds' points comes out 1-2 ulps
short of the direction on each axis where two give it. VERIFIED: the frame, the stored offsets, the
pre-normalize point, the direction and `trDelta`
(`combat::a_uo_rocket_leaves_on_retails_bits`). The same run's
spectator capture is `net/snapshots-uo-bazooka-draw.bin`
(`--capture-tag uo-bazooka-draw`, the `*-uo-bazooka.bin` recipe of
coduo-diff.md 19 on port 34531 under gdb); its rocket's `trBase` and
`trDelta` match gdb's, and
`uo_weapons_ab::the_bazooka_rocket_leaves_on_retails_bits` replays the
shot on ours with gdb's `gunrandom` pair (x 0xbea62115, y 0xbf0d3c83,
`Server::pin_spread_draws`) and sends both on retail's bits (a direction off
the bullet's 8192 end point with f32 sums and one normalize lands 1-2 ulps
off). vcod: `combat::rocket_aim`, `missile::rocket_launch_dir`.

VERIFIED, gdb on the retail 1.1d server (`probe_panzer` on
mp_carentan, the shooter `--probe-weapsel`; dprintfs at 0x68c90, 0x68cf7
and 0x68d22): one panzerfaust round at spread 3.5 read `gunrandom`'s pair
(0xbcf26ff0, 0xbe91d118), the tangent's float 0x3d7a85a8, the frame, the
point 16 out (0x41800063, 0xbcefcbd2, 0xbe8906ae), the direction after
`fire_rocket` (0x3f7ff6ba, 0xbaefc269, 0xbc89014e) and `trDelta`
(2799, -5, -46). The kept arm lands on all of them, the tangent included,
though 1.1d takes it with `fptan` (0x68c5e) where vcod takes libm's in
double (`combat::a_11_rocket_leaves_on_retails_bits`). Only one round
was read.

**The rocket.** VERIFIED, `fire_rocket` (0x54614): `G_Spawn`, `classname`
`scr_const+0x6e`, `nextthink` `level.time + 30000` (0x54666) with `think`
`G_ExplodeMissile`, `s.eType` 4, `s.eFlags |= 0x10000` (0x54683),
`r.svFlags` 0x88, `s.weapon`, `r.ownerNum` and `parent` the shooter, the
weapon's `damage`, inner and outer damage and radius into +0x238..+0x244
as `fire_grenade` copies them (11.1), `methodOfDeath` 5 (`MOD_PROJECTILE`)
and `splashMethodOfDeath` 6 (`MOD_PROJECTILE_SPLASH`), `clipmask`
0x2802091, `s.pos.trType` 2 (`TR_LINEAR`), `trTime` `level.time - 50`
(0x54708), `trBase` the muzzle, `trDelta` `dir * projectileSpeed` truncated
toward zero per axis, `r.currentOrigin` the muzzle, and `G_SetAngle` of
`vectoangles(trDelta)` (0x547e3; `G_SetAngle` 0x67d9c copies the three
floats into `apos.trBase` and `r.currentAngles` with no wrap). VERIFIED,
`net/snapshots-skyrocket.bin` (`probe_skyshot`'s sky part with
`panzerfaust_mp`, `--probe-uo-weapon aim --save-snapshots --capture-tag
skyrocket`): a rocket fired 80 degrees up with `trDelta` (344, 350, 2756)
went out with `apos.trBase` (280.09662, 45.49534, 0): `vectoangles`' pitch,
down positive and wrapped into [0, 360), not Q3's -79.9.
`missile_edge_ab::launch_angles_match_retail_on_1_1` holds ours within the panzerfaust's 3.5 degree `adsSpread` of it (each server
draws the spread off its own RNG). VERIFIED, `game.mp.uo.i386.so`: UO's
`fire_rocket` calls `vectoangles` (0x5ff25) on `s.pos.trDelta` after the
three `fistp`s that truncate it (0x5fe8c..0x5fecc), then `G_SetAngle`
(0x5ff3c). VERIFIED, `net/snapshots-uo-bazooka.bin`: `trDelta` (2799, 22,
43) goes out with `apos` pitch short -161 (65375) and yaw 81, which is
359.12 and 0.45 truncated to 65536ths; -0.88 would truncate to -160, so the
wrap shows on UO's wire. `uo_weapons_ab`'s two bazooka gates check `apos`
against `vectoangles` of the wire `trDelta` on both sides and against
retail's bits on the pinned draw. VERIFIED, `game.mp.uo.i386.so`:
UO's `vectoangles` (0x3aded) wraps both angles into [0, 360) the same way,
beside a separate `vectosignedangles` (0x3af4c). INFERRED: no bounce bit (0x03000000) is set, so
13.1 explodes it on whatever it strikes: a body or anything else that takes
damage gets `damage` (1000 for `panzerfaust_mp`) as `MOD_PROJECTILE` with
the flight as the direction and the splash skips it; the blast is
centred on `tr.endpos`. VERIFIED: 1.5b's `fire_rocket` (0x55582) stores the
same 30000, 0x10000, 2, 5, 6 and 0x2802091. VERIFIED: UO's (0x5fc74) arms
15000 instead (coduo-vehicles.md 13 has its gravity arc), and UO's
`Weapon_RocketLauncher_Fire` (0x7b3da) pushes the shooter by the same
`-64.0` (.rodata 0xa5f68).

**On the wire.** VERIFIED, two retail captures on mp_carentan dm
(`client-probes/probe_panzer`, the shooter a `--probe-weapsel`
client, a following spectator recording): `net/snapshots-panzer-wall.bin`
(`+set probe_run 1000`) and `net/snapshots-panzer.bin`. In the first, the
fire event (161) rides the snapshot at 30500 and so does entity 170, `eType`
4, `eFlags` 0x10000, `index` 0, `trType` 2, `trTime` 30400, `trBase`
(-703, 440, 66), `trDelta` (2796, 121, -56), `apos` stationary at
(1.15, 2.48, 0); at 30700 the same entity is `eType` 0 with event 179
(`EV_ROCKET_EXPLODE`), parm 104, `surfType` 17 (rock), `eFlags` still
0x10000, `trBase` (33, 471, 52): the contact snapped toward the base
(13.1); it stays on the wire through 31000. In the second the round flew
from 30950 to 31550 and went off the same way on `surfType` 10 (grass).
The first probe run, from a spawn facing a wall 80 units off, put the
explode (179, parm 143, `surfType` 21) on the very snapshot that carried
the fire, and the shooter took 18 of his own splash (187); that capture
was not kept.

**vcod.** `Server::replay_moves` turns a `projectile` weapon's fire event
into `Attack::Rocket`; `Server::launch` aims it with `combat::aim_end`,
pushes the shooter and calls `Missiles::fire_rocket`, which builds 1.1's
rocket (or UO's under protocol 22). The contact runs through
`Missile::impact`: 179, or 180 off a body, the blast at the contact and the
direct hit as `MOD_PROJECTILE`. Pinned by
`a_panzerfaust_launches_a_rocket_that_explodes_where_it_strikes` and
`a_rocket_that_strikes_a_player_hits_him_directly`
(`crates/server/tests/combat.rs`), and on the client by
`the_panzer_capture_s_explode_draws_the_rocket_row`
(`crates/client/src/fx/registry.rs`). Its water splash is 12.1's.

VERIFIED, `game.mp.uo.i386.so`: UO's `fire_rocket` (0x5fc74) stores
`trType` 2 (0x5fde6) and a 15000 ms think, `Weapon_RocketLauncher_Fire`
(0x7b3da) leaves it, and `Weapon_Artillery_Fire` (0x7b594), a vehicle main
gun's fire, rewrites it to 5 (0x7b6c8). VERIFIED, live (retail UO 1.51b, `mp_carentan-skyshot-rocket-uo.txt`): a player's bazooka round
reads `trType` 2 and climbs an even 137 a frame; vcod flies it the same
(`a_uo_missile_flies_through_the_sky`). A vehicle main gun's arc is
`coduo-vehicles.md` 13.

### 13.7 A missile whose thrower left

VERIFIED, `game.mp.i386.so` 1.1d: `G_FreeEntity`'s sweep (0x66ad1..0x66b13,
which `ClientDisconnect` reaches at 0x42bfc) stores 0 into `+0x198` where it
holds the freed entity (0x66add, 0x66ae5) and 0x3ff into `r.ownerNum`
(`+0x14c`) where it holds its number (0x66aef, 0x66af7), on every entity in
use. VERIFIED, 1.5b and UO: `G_FreeEntityRefs` makes the same two stores
(`+0x1a0` and `+0x150` on 1.5b, `+0x1a8` and `+0x154` on UO;
`cod11-turrets.md` 8). INFERRED: `+0x198` is `parent`, which `fire_grenade`
and `fire_rocket` write with the thrower (UO 0x5f8c0, 0x5fd6e).

VERIFIED: every missile splash hands `G_RadiusDamage` its attacker off
`parent`: 1.1d `G_ExplodeMissile` 0x53f52 and `G_MissileImpact` 0x53d47
(`+0x198`), 1.5b 0x54986 and 0x545e7 (`+0x1a0`). UO's `G_ExplodeMissile`
takes `parent` (0x5e2fc..0x5e30e), swapped for its gunner when it is a
turret (`eType` 11) with an owner (0x5e314..0x5e342); its `G_MissileImpact`
takes NULL when `r.ownerNum` is 0x3ff (0x5dc8e) and `g_entities[ownerNum]`
otherwise (0x5dc9e..0x5dcb7). VERIFIED: `G_RadiusDamage` returns 0 before
its walk when that argument is NULL (1.1d 0x4a40d, 1.5b 0x47b2d, UO
0x509ac). VERIFIED, 1.1d: `G_Damage` (0x49dac) on a client passes a NULL attacker on
to `Scr_PlayerDamage` (0x5ca18), which pushes it as undefined, and on anything else
substitutes `g_entities[1022]` (`g_entities` 0x21df80 plus 1022 entities of
0x314 is the 0x2e2958 it loads).

INFERRED, from the above: once the thrower is gone his frag or rocket is
passed by nothing (its traces pass 0x3ff), its splash hurts no one at all,
and a direct hit reaches `CodeCallback_PlayerDamage` with `eAttacker`
undefined. The next client into the slot is never charged: the sweep ran
before he connected.

VERIFIED, UO `fire_artillery` (0x5ff4a): a shell fired by a barrage
(`eType` 0) takes the barrage's `r.ownerNum` and `&g_entities[ownerNum]` as
`parent` (0x6002d..0x60055). VERIFIED: `G_FreeEntityRefs` (0x79a06) sweeps
every entity in use, the barrage among them (0x79a7c..0x79a8a). INFERRED: a
barrage whose caller left hangs shells with `r.ownerNum` 0x3ff and `parent`
`&g_entities[1023]`, and the shells already hanging were swept with him. A
shell goes off on contact through `G_MissileImpact`, whose 0x3ff test
(0x5dccc) leaves inflictor and attacker NULL: no splash, and a direct hit
with both undefined. Only `G_DropArtillery`'s 15 s fallback,
`G_ExplodeMissile`, reads `parent`, and so walks its splash with entity
1023 as the attacker.

VERIFIED, UO: `G_DropArtillery` (0x5eabe) sets the shell's `nextthink` to
`level.time + 15000` (0x5eadc, 0x5eae1) and its `think` to
`G_ExplodeMissile` (0x5eaf0). VERIFIED: `G_ExplodeMissile` first copies
`+0x330` into `r.ownerNum` when that is 0x3ff (0x5e2db..0x5e2f3), then takes
`parent` (`+0x1a8`, 0x5e2fc..0x5e308) as `G_RadiusDamage`'s attacker, with
the shell as inflictor and ignore and `+0x25c` as the means of death
(0x5e79c). VERIFIED: `fire_artillery` writes `+0x25c` 14 and no `+0x330`
(0x5ff4a..0x601fb); `fire_rocket` writes `+0x25c` 6. INFERRED: `+0x330`
is then the 0 that `G_FreeEntity`'s memset or `G_InitGame`'s left. INFERRED: a shell that strikes nothing in 15 s goes off as
`MOD_ARTILLERY_SPLASH`, charged to its caller while he is there, to no one
when his leave swept `parent` to NULL, and to `&g_entities[1023]` when the
barrage hung it after he left. VERIFIED: `G_InitGame` (0x586cf) memsets
`g_entities` (0xd3000 bytes), and the only reference to `g_entities +
0xd2cb4` (entity 1023) in the module is a compare; `Scr_AddEntity`
(0x7239f) pushes `Scr_AddEntityNum` of the entity's first word, `s.number`.
INFERRED: entity 1023's number reads 0, so `CodeCallback_PlayerDamage` gets
entity 0 as `eAttacker` from that splash. vcod: the drop arms
`Missile::explode_at_ms` 15 s on, a shell `fire_artillery` hangs with no
owner keeps `parent_unspawned`, and its fuse blast is charged to
`host::ZEROED_ENTITY`, which `ScriptRuntime::attacker_entity` reads as entity
0. A fuse blast's means of death is the missile's own splash one (a rocket's
`MOD_PROJECTILE_SPLASH`, a shell's `MOD_ARTILLERY_SPLASH`), not
`MOD_GRENADE_SPLASH`.

VERIFIED, UO: `fire_rocket` arms `G_ExplodeMissile` 15 s on for every
rocket, a vehicle's shell among them (0x5fc74: the `nextthink` and
`think` stores at 0x5fce1 and 0x5fcf0, ahead of the `eType` 12 test at
0x5fd33), and `G_ExplodeMissile` swaps `parent` for
`g_entities[parent->r.ownerNum]` only when `parent`'s `eType` is 11
(0x5e314..0x5e342). INFERRED: a vehicle shell that strikes nothing in
15 s walks its splash with the vehicle itself as the attacker, never the
driver its direct hit names, and `CodeCallback_PlayerDamage` gets the
vehicle as `eAttacker`, whether or not the driver is still in the slot. A
freed vehicle swept `parent` to NULL, so no walk. vcod: the fuse's
`Explosion::owner` is the vehicle's entity number, which
`ScriptRuntime::charged_owner` keeps and `attacker_entity` resolves while
the vehicle is live (pinned by
`a_vehicle_shell_s_fuse_blast_is_charged_to_the_vehicle` and
`a_blast_charged_to_a_vehicle_names_it`), never the driver's slot.

The fuse measured, UO 1.51b: `client-probes/probe_shellfuse`
on mp_kharkov calls `fireTurret` on the empty Panzer IV (162), then seats
client 0 in it, fires twice and pins each shell with an `origin` write
every frame, the first 180 above the Elefant (192), the second 180 above
the Panzer, where client 0 sits in the second seat by the time it goes
off (fixtures `crates/server/tests/fixtures/vehicles/mp_kharkov-shell-fuse-uo.txt`
and `crates/common/tests/fixtures/net/*-uo-shellfuse.bin`, recipe in the
first's header). Each row is VERIFIED, live capture:

| What | Retail |
|---|---|
| `fireTurret` with no driver | no `rocket` spawned (0 before, 0 after) |
| shell 0, fired at 33050 | its explode on the wire at 48050, 15000 on, at the pin truncated, (-1494, 6195, 120); `eType` 0, `eFlags` 0x8080 |
| the Elefant's `damage` notify | 76 `MOD_PROJECTILE_SPLASH`, attacker 162 (the Panzer), inflictor the shell |
| shell 1, fired at 35050 | its explode at 50050, (-1218, 6214, 115), `eFlags` 0x8080 |
| client 0's `CodeCallback_PlayerDamage`, second seat | 18 `MOD_PROJECTILE_SPLASH`, `sHitLoc` `none`, `eAttacker` 162, `eInflictor` the shell |
| the Panzer's own `damage` notify | 81 `MOD_PROJECTILE_SPLASH`, attacker 162 |

So the vehicle is the attacker on a vehicle and on a player, and the
rider who fired, now out of the driver's seat, is charged nothing. Not
measured: a shell whose driver left the server before the fuse.

VERIFIED, live capture: the probe's `pin` thread, which loops while its
shell is defined and then logs `PROBE gone`, never logged it; the thread
ended with its entity. VERIFIED: `Scr_SetOrigin` (UO 0x6e801, 1.1d
0x5c61c) calls `G_SetOrigin` (UO 0x7a17a), which writes `trBase`, zeroes
`trType`, `trTime`, `trDuration` and `trDelta` and copies the point into
`r.currentOrigin`; UO's `G_RunMissile` (0x5ee7d) then turns a `trType` 0
missile whose ground is not 0x3fe and has nothing within its trace below
into `trType` 5 at `level.time` from that point (12.6). INFERRED: the
evaluation at that same `level.time` moves it nothing, so a write every
frame holds it. VERIFIED: UO's `G_ExplodeMissile` ORs `eFlags` 0x80
(0x5e435; 0x100 in 1.1's layout) and 1.5b's 0x100 (0x547dc). INFERRED,
decompile read: UO's `G_MissileImpact` sets 0x100000 on a vehicle hit and
no such bit.

vcod: `Missiles::set_origin` (from `GameHost::set_field`) is the
`G_SetOrigin`; the explode flag rides `Missile::fused`, set by the fuse
alone; `fireTurret` fires inside the thread. Pinned by
`crates/server/tests/vehicle_fuse_ab.rs`, every probe line and both
explodes exact (times past the placement and the first seat, the shells
named by their order: ours numbers temp entities apart).

VERIFIED, UO: the `r.ownerNum` copy from `+0x330` (0x5e2db..0x5e2f3) runs
ahead of the explode's two traces, which pass the missile (`*param_1`)
under mask 0x11 (0x5e519 down, 0x5e745 the 10 up; 0x5e616 is the water
trace), and the explode clears `r.contents` (0x5e46c). VERIFIED:
`CanDamage`'s (0x503f8) traces pass the victim, not the inflictor
(`trap_LocationalTrace` with `*param_1`, mask 0x802091).
INFERRED: a disowned shell's `r.ownerNum` of 0 makes those two traces pass
entity 0, which under mask 0x11 only a solid entity 0 could stop, and slot
0 is a player's body (0x2000000); with its contents cleared the exploded
shell clips nothing for 300 ms whatever owns it. vcod models no
`r.ownerNum` on an exploded missile, and nothing it traces reads one.

VERIFIED, UO `fire_rocket` (0x5fc74): from a vehicle (`eType` 12) the
rocket's `+0x330` takes the vehicle's own `r.ownerNum` (0x5fd3f, 0x5fd45),
else 0x3ff (0x5fd50); its `r.ownerNum` and `parent` are the vehicle
(0x5fd62, 0x5fd6e). INFERRED: the vehicle's `r.ownerNum` is its driver. VERIFIED: `G_MissileImpact` reads `+0x330`
as the attacker when the owner's `eType` is 12 and it is not 0x3ff
(0x5dcff..0x5dd1d), and `G_FreeEntityRefs` makes no store into `+0x330`
(0x79a06..0x79b40). INFERRED: a driver who leaves keeps the credit for a
shell already fired, and the client who next takes his slot is charged
with it, as vcod charges the slot (`Missile::owner`, kept by
`free_owner_refs` for a vehicle's shell). A vehicle freed under its shell
sweeps the shell's `r.ownerNum` to 0x3ff, which drops the `+0x330` read:
vcod's `Missiles::free_vehicle_refs`, from `GameHost::free_entity`.
VERIFIED, UO: `ClientDisconnect` (0x45433) ends in `G_FreeEntity`, whose
memset clears the whole 0x34c-byte entity, `s.number` with it.
VERIFIED: `G_DamageClient` passes the attacker on untouched to
`Scr_PlayerDamage`, whose `GScr_AddEntity` pushes a non-NULL one through
`Scr_AddEntity`, `Scr_AddEntityNum(ent->s.number, 0)`; `G_Damage`'s other
arms push it through `Scr_AddEntity` too. INFERRED: with the slot empty
the shell's direct hit and splash are charged to the freed entity and the
callback sees entity 0 as `eAttacker`: the client in slot 0 when there is
one. vcod: `ScriptRuntime::charged_owner` turns an owner slot with no client
into `host::ZEROED_ENTITY`, delivered as entity 0. Not modelled: with no
entity 0 in use vcod passes undefined, where retail pushes the free
entity's object.

VERIFIED, UO: with the owner 0x3ff `G_MissileImpact` keeps the inflictor
NULL too (0x5dc8e), `G_Damage` hands a client victim to `G_DamageClient`
(0x4fd4b), which with both NULL reads weapon 0 and passes both on to
`Scr_PlayerDamage`, whose `GScr_AddEntity` (0x6e92d) pushes NULL as
undefined. INFERRED: weapon 0's name is `"none"`, as 1.1 measured for a
fall (`cod11-player-clip.md` 8.8). 1.1's `G_MissileImpact` hands the missile
as the inflictor whatever the owner. vcod: a UO disowned shell's direct
hit goes to `deliver_hits` with no inflictor and weapon `"none"`.

vcod: `Missiles::free_owner_refs`, called from `ClientDisconnect`, makes the
missile's owner `None`; the explode's `Explosion::owner` then skips the
blast walk and `ScriptRuntime::deliver_hits` hands a direct hit an undefined
attacker. A UO vehicle's shell is owned by the vehicle in retail
(`fire_rocket`'s `self`), so it keeps its driver in vcod. Pinned by
`a_frag_whose_thrower_left_charges_no_one` (`crates/server/tests/combat.rs`),
`a_leaver_s_grenade_loses_its_owner_and_a_vehicle_s_shell_does_not`,
`a_shell_whose_vehicle_was_freed_charges_no_one`,
`a_barrage_whose_caller_left_hangs_shells_nobody_owns` and
`a_hit_with_no_inflictor_and_no_attacker_reaches_the_callback_undefined`.

### 13.8 A free set on a missile

VERIFIED: 1.1d's `G_RunMissile` ends in `G_RunThink` (0x54050, 0x54398),
UO's too (0x5f03b, 0x5f5db), and a missile's fuse is its think
(`G_ExplodeMissile` at `nextthink`, section 11). VERIFIED: the `delete`
method arms `G_FreeEntity` 100 ms out (1.1d 0x5da14,
`cod11-gsc-object-model.md` 14; UO 0x68bda, its `nextthink` store at
0x68c86), and 1.1d's drop ring eviction arms it at `level.time + 1` on
whatever entity holds the evicted number (`cod11-items.md` 8). INFERRED:
either replaces the fuse; the grenade flies on until the think is due, then
goes without an explode, a blast or an event. A shell that strikes on the
way still goes off in `G_MissileImpact`. INFERRED, from Q3's `G_RunFrame`:
an exploded missile riding out its event runs no think, so a free set then
changes nothing. vcod: `ScriptRuntime::run_entity` hands a `ThinkFn::Free`
armed on a missile's entity to `Missiles::schedule_free` before its turn,
and the pool frees it on the turn the think is due.

A shell that strikes on the very turn its free is due. VERIFIED:
`G_RunMissile` tests `eType` against 4 straight after `G_MissileImpact`
and skips `G_RunThink` when it changed (1.1d 0x54354..0x54358, UO
0x5f574..0x5f578), and the explode writes `eType` 0. VERIFIED, UO:
`G_RunEntity` (0x59a35) frees an entity whose `freeAfterEvent` (`+0x194`) is
set once 300 ms have passed since its event (0x59ac5..0x59aec) and runs
nothing else on it (0x59b5c); 1.1d's (0x502bc, `+0x184`) is the same.
INFERRED: the strike's explode goes out and the entity rides out its 300 ms
whatever free is pending, as vcod's pool does (`free_at_ms` cleared on a
strike). The earlier note that retail frees it before its event goes out
was wrong. Pinned by `a_deleted_grenade_flies_on_and_goes_unexploded`,
`on_1_1_a_grenade_on_an_evicted_ring_number_goes_unexploded` and
`a_shell_striking_as_its_free_falls_due_rides_out_its_event`.

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
  wall the blast went off against. VERIFIED: the midpoint is
  `0.5 * (absmin + absmax)` (`+0x11C` and `+0x128`, read at
  `0x4A687..0x4A6B5`), and those two are written only by `SV_LinkEntity`
  (`cod_lnxded` `0x8090b60..0x8090c13`), as `r.currentOrigin + mins` and
  `r.currentOrigin + maxs`, each widened by one unit on every axis. INFERRED:
  the widening is symmetric, so the midpoint is the origin of the last link
  plus the box centre, and for a client that is the snapped origin of 14.3.
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

VERIFIED, `probe_blastmove` on retail (14.5): `setPlayerIgnoreRadiusDamage(true)`
called inside a victim's callback left the rest of that walk reaching its
victims, and the next blast reached nobody. VERIFIED: the builtin's store
of 0 to `level+0x29F4` is at `0x5EF5A`, the instruction after its
`G_RadiusDamage` call at `0x5EF55`. INFERRED: the walk tests the
`level+0x29F4` copy, which only the builtin writes, so the flag takes hold
at the next `radiusDamage`; and since a nested `radiusDamage` writes 0 back
into the copy on its way out, a walk resumed after one tests 0,
which no walk can observe, since a walk that tests 1 reaches no client and
so runs no callback a nested call could sit in.

VERIFIED, `probe_blastloop` on retail (14.5): a blast after
`setPlayerIgnoreRadiusDamage(true)` reached nobody, a second one with no call
in between reached nobody either, and one after `(false)` reached both
players in range. INFERRED: the builtin's write of 0 lands on its own copy,
and the flag stays set until script clears it.

What is left of the `radiusDamage` divergence entry in
`cod11-gsc-language.md` after this: the victim walk's order and reach, the
standing box the builtin measures a victim with. The flag is not among them,
and neither is the attacker: vcod hands the callbacks the world entity too.

### 14.3 `CanDamage`

VERIFIED: `CanDamage` is `0x4A098`, `0x35C` bytes, takes `(targ, origin)` and
returns a float on the x87 stack rather than an integer. VERIFIED: both of its
arms trace five times with `trap_LocationalTrace(&tr, point, origin,
targ->s.number, 0x02802091, bulletPriorityMap)`, the same mask
`fire_grenade` gives a missile (11.1): the pushes at `0x4A283..0x4A29E` (and
`0x4A3A3..0x4A3BE` on the other arm) put the probe point (`ebx`) in the
`start` slot and `origin` (`[ebp+0xC]`) in the `end` slot, the order 2.3
reads off `Bullet_Fire_Extended`'s call to the same trap. INFERRED: so each
probe traces from the victim toward the blast, which matters wherever a clip
is one-sided, as a model's collision surface is (the mantle doc, "Static
models are clipped as a segment"). VERIFIED: the only call to it in the
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

**Which origin a client is read at.** The muzzle starts from the snapped
origin because `FireWeapon` runs inside the window where
`ClientThink_real` holds the truncated `s.pos.trBase` in `r.currentOrigin`
(2.1). A blast does not run inside it:

- VERIFIED: `ClientThink_real` writes the snapped origin into
  `r.currentOrigin` at `0x4051E..0x40533`, links at `0x40595` and writes
  `ps.origin` (`client+0x14`) back at `0x405C7..0x405DC`. VERIFIED:
  `G_RadiusDamage` has three call sites, `G_MissileImpact` (`0x53D50`),
  `G_ExplodeMissile` (`0x53F5B`) and the `radiusDamage` builtin
  (`0x5EF55`), none of them in `ClientThink_real`.
- VERIFIED: the only relocations naming `G_ExplodeMissile` are the three
  `think` stores of 13.2. VERIFIED: `G_MissileImpact`'s one caller is
  `G_RunMissile` (`0x5434C`), which also calls `G_RunThink` (`0x54050`, `0x54398`), and
  `G_RunMissile` is called at `0x50375` from `G_RunEntity` (`0x502BC`, 11.4),
  which also calls `ent->think` itself (`0x503E1`, `0x5046F`) and which
  `G_RunFrame` calls at `0x50949` and `0x50955`. INFERRED: a fuse or an
  impact explodes inside `G_RunFrame`'s entity loop. VERIFIED:
  `ClientThink_real` is called from `ClientThink` (`0x415B4`), which `vmMain`
  calls at `0x50E38`, apart from its `G_RunFrame` call at `0x50EA4`; from
  `G_RunClient` (`0x40695`), on `g_synchronousClients` only; and from
  `ClientSpawn` (`0x42A82`). INFERRED: every grenade blast and every script
  `radiusDamage` runs inside `G_RunFrame`, after each client's last
  `ClientThink_real` has written the unsnapped origin back.
- VERIFIED: `ClientEndFrame` (`0x40E98`) has no store to `r.currentOrigin`.
  VERIFIED: `TeleportPlayer` copies `ps.origin` into `r.currentOrigin` at
  `0x51454`, and so does the `setorigin` player method (object-model doc
  23.2). VERIFIED: `G_RunClient`'s arm for a client with a tag parent
  (`ent+0x2E4`) calls `G_SetFixedLink`, `G_SetOrigin(ent,
  &ent->r.currentOrigin)` (`0x406E9`) and `trap_LinkEntity` (`0x40713`), and
  that arm holds its only link. INFERRED: no client reaches a blast with a
  snapped `r.currentOrigin`, so the distance, the `dir`, the eye and the
  five probes of this section are all built off the unsnapped `ps.origin`.
- INFERRED, off the link at `0x40595` sitting inside the window: an
  unlinked client's `absmin` and `absmax` are those of its last cmd's link,
  at the snapped origin, while a linked client's are relinked every frame at
  the anchored, unsnapped one. So the second chance (14.1) is the one place
  a blast reads a client's snapped origin. INFERRED: a `setOrigin` links
  unsnapped, and that box holds until the client's next cmd relinks it.
- INFERRED, unmeasured: a `CodeCallback_PlayerDamage` that called
  `radiusDamage` before its first `wait`, from a hit inside a cmd, would read
  the client running that cmd at its snapped origin. No stock script does.

VERIFIED: the count of traces returning `fraction == 1.0` maps to the return
value as 0 for none, `1.0` for four or five, and `count / 3.0`
(`.rodata 0x7442C`) otherwise. INFERRED: so a client behind partial cover
takes a third or two thirds of the falloff damage, and three of five clear
points is already full damage. INFERRED: nothing on the bullet path consults
any of this, which 4.6 already said.

**As implemented.** `crate::game::combat`'s `Blast` and `can_damage`, with
`Cod1Module::run_frame_cod1` walking each of the frame's explosions where it delivers
damage, so a grenade damages on the frame it goes off, and the
`radiusDamage` builtin (`builtins/combat.rs`) walking the same `Blast` for a
script's own; both walks are 14.5's. The divergences left are listed in
`cod11-gsc-language.md`'s `radiusDamage` entry. Every victim is measured
and probed at its unsnapped origin, and the second chance's midpoint is
taken at `BlastVictim::link_origin`: `ClientSim::link_origin` for a
grenade's blast, the truncated origin unless the client is linked or its last
link was a `setOrigin` or `TeleportPlayer` with no cmd since, and for the
builtin's the same value mirrored onto `GameHost::client_link_origin`, which
the `setOrigin` builtin moves to its own unsnapped origin at once. Pinned by
`a_set_origin_links_unsnapped_until_the_next_cmd`
(`crates/server/src/spectate.rs`). INFERRED, off `TeleportPlayer`'s
`r.currentOrigin` store ahead of its link (turrets doc 8): a release links
unsnapped the same way. The falloff is computed at
double precision because f32 loses a point of damage at the round ratios a
script picks -- `50 + (1 - 100/300) * 1950` truncates to 1349 in f32 and 1350
in f64 -- and retail's own x87 arithmetic is not reproducible in either
width.
`crates/server/tests/combat.rs`'s two blast tests are the end-to-end gate, and
the grenade capture's first version (in git at `c52e9606`) pinned the
number a third time. VERIFIED: its `throw_down` pain frame reads
`aimSpreadScale` 94.00 off a counter that was 0. INFERRED: section 6's step
5 adds `damage * 100 / maxHealth` there, so retail charged 94, which is what
the falloff gives at the 78 units the replay measured between the blast and
the thrower. VERIFIED: the later takes' thrower takes no damage, so they
carry no such frame.

### 14.4 What stops a blast, measured

Two client probes, run against the retail 1.1d server and against
`vcod-server` with four `--net-probe --probe-team` clients each (allies, axis,
allies, axis), `sd` rules on mp_carentan. The recipe and the raw lines are in
`crates/gsc/tests/fixtures/semantics/client-probes/README.md`.

`probe_bomb` puts the charge where the committed plant capture's `getPlant`
put it, `(-176.8, 2473.1, -22.96)`, on the plate of the flak88 script model
beside `bombzone_A` (2.7, and the object-model doc 23.6), stands clients 0
to 3 at `(-196, 2455, -22)` in the open 26 units off and at
`(-84, 2511, -22)`, `(-112, 2446, -22)` and `(-77, 2473, -32)` across the
flak, runs four `radiusDamage(…, 500, 20, 20)`s with the bomb model still
standing, then the stock `bomb_countdown`, whose fuse deletes the intact
flak, shows its `_d` twin and deletes the bomb model before its own
`radiusDamage(origin, 500, 2000, 1000)`. VERIFIED, the servers'
`games_mp.log` `D;` and `K;` records and the probe's health line after the
fuse:

| blast | retail | vcod, world | vcod, script models | vcod, bodies too |
|---|---|---|---|---|
| at the charge | none | 0: 20 | none | none |
| 20 above the charge | 0: 20 | 0: 20 | 0: 20 | 0: 20 |
| at `bombtrigger getorigin()`, which reads the charge | none | 0: 20 | none | none |
| 40 above client 0's feet | 0: 20 | 0: 20, 2: 13, 3: 6 | 0: 20, 2: 6, 3: 6 | 0: 20 |
| the fuse | none, all four at 100 | 0 killed, 1944 | none | none |

The three vcod columns are builds with the world alone in `CanDamage`, with
the script models added, and with the player bodies added as well; the last
is vcod's current rule.
INFERRED, off the blast sitting inside client 0's box: client 0's bones stop
the probes from clients 2 and 3 on the fourth row, which is the shielding the
`probe_blastbody` rows below measure directly. INFERRED, off the first, third and fifth
rows against 2.7: the flak models and the bomb model are script models, they
stop `CanDamage`'s locational traces, and the second chance's
`trap_Trace` (mask 0x11) does not see them, so a charge sitting on the flak
harms nobody, even a player 26 units away in the open. A charge planted on
open floor is not what this run measured.

`probe_blastbody` puts client 0 at `(-226, 2424, -32)` and client 1 at
`(-269, 2381, -32)`, 70 and 130 units down one line from a blast at
`(-176.8, 2473.1, 7)`, parks the other two out of range, blasts
`radiusDamage(…, 500, 20, 20)`, moves client 0 to `(-300, 2473, -24)` off the
line and blasts again, then puts client 0 back on its station, turns it
through the yaws 0 to 315 in steps of 45 with `setPlayerAngles`, one blast
each, and last `suicide()`s it and blasts over its corpse. Each yaw row logs
client 0's `angles` and both origins as they read before the blast. Client 0
is the allied probe, which joins with `m1carbine_mp`.

VERIFIED, two retail runs, the `D;` records for client 1,
beside what `a_body_in_the_line_shields_a_blast_the_way_the_retail_probe_measured`
(`crates/server/src/game/combat.rs`) reads for the same geometry with client 0
posed as an idle carbine player at the yaw its `angles` read:

| row | client 0 `angles` yaw | client 1 origin | retail | vcod replay |
|---|---|---|---|---|
| shielded | 0 | `(-269, 2381)` | 13, 13 | 13 |
| unshielded | -- | `(-269.8, 2380.2)` | 20, 20 | 20 |
| yaw rows reading 0 | 0 | `(-270.9, 2379.1)` to `(-274.7, 2375.3)` | 13 (nine rows) | 13 |
| yaw rows reading 0 | 0 | `(-275.4, 2374.6)`, `(-275.8, 2374.2)` | 6, 6 | 6 |
| yaw 135 | 135 | `(-273.3, 2376.7)` | none | none |
| yaw 180 | 180 | `(-273.3, 2376.7)` | 6 | 6 |
| yaw 225, 270, 315 | as set | `(-273.3, 2376.7)` | none | none |
| corpse | -- | `(-273.3, 2376.7)`, `(-276.5, 2373.5)` | 20, 20 | 20 |

VERIFIED: client 0 took 20 on every row it stood through. VERIFIED: the first
run's `angles` read 0 through the yaw 90 row and the set yaw from 135 on, and
the second run's read 0 on every row. INFERRED: `setPlayerAngles` did not
land on the rows reading 0, since those match the yaw 0 rows' damage and the
rows reading 135 to 315 do not; rows are filed by the yaw read back, and what
kept the call from landing is not established. INFERRED: client 1 drifts
between rows under the blasts' knockback.

INFERRED, off 3.1 and 14.3: every probe is a locational trace with the
victim as its pass entity and mask 0x02802091, which carries
`CONTENTS_BODY` (0x02000000), the contents a playing player's end frame
writes (`docs/research/cod11-player-clip.md`, 1.2), and not
`CONTENTS_CORPSE` (0x04000000), which is what `player_die` writes (5.1,
step 10). So a live player's posed bones stop a probe, ranked through
`bulletPriorityMap` like a pistol round's, the victim's own body never does,
and a corpse or a dead player does not. INFERRED: 13 is `(int)(20 * 2/3)`,
two of five probes clear; 6 is `(int)(20 / 3)`, one; none is zero clear
with the second chance out of reach at 130 units. VERIFIED: the corpse rows
read 20.

vcod: `can_damage` traces each probe from the probe to the blast, against
the collision world, every live script model (`GameHost::placed_script_models`,
the set `bulletTrace` clips) and every other live, playing client's posed
body (`HitBody`, through the same `trace_bodies` a bullet uses), posed at
the tick's clock; the second chance traces the world alone with mask 0x11 and
no static models. A grenade's blast takes its bodies from the sims; the
`radiusDamage` builtin takes the pose `Server` mirrors onto the host before
the script frame (`GameHost::client_bodies`) at the script `origin` of each
client, so a `setOrigin` earlier in the frame moves the body.
The replay column above holds at every anim phase the test pose was tried
at but one: at yaw 135 one of nine phases reads 6 where retail read none,
since the body's yaw and its pose decide which probes the bones cross.
VERIFIED, the same probe against `vcod-server`: client 1 took
13 shielded, 20 unshielded, 13 on each of the eight yaw rows, whose `angles`
all read 0 there too, and 20 behind the corpse.

### 14.5 One victim at a time, and who is still a victim

`G_RadiusDamage` collects its candidates once and damages each inside the
walk. VERIFIED, `game.mp.i386.so`: the one `trap_EntitiesInBox` call
(`0x4A492`) sits ahead of the loop, whose back edge is `0x4A894`, and the
`G_Damage` call (`0x4A87C`) sits inside it. VERIFIED: the `takedamage` byte
(`0x4A4D3`), `r.currentOrigin` (`0x4A4E9`) and `level+0x29F4` (`0x4A5D3`) are
read inside the loop, once per candidate. INFERRED: since `G_Damage` runs
`CodeCallback_PlayerDamage` to its first `wait` (4.2, 4.4), each victim's
callback has run before the next candidate is tested or measured, and a
candidate a callback moved or killed is measured as it now stands.

`takedamage` on a client. VERIFIED, the stores to `gentity+0x171` in the
module, by the function holding each: `ClientEndFrame` stores 0 on the
intermission arm (`0x40ED9`), 1 on the arm for `sessionState` 0 or 1
(`0x40F93`) and 0 again for `sessionState` 1 (`0x4107C`);
`SpectatorClientEndFrame` stores 0 (`0x40788`); `ClientSpawn` stores 0
(`0x4273C`) and calls `ClientEndFrame` at `0x42A75`; `player_die` stores 1
(`0x49C21`, 5.1 step 10). The other stores are `G_SpawnTurret` (`0x5301A`),
`G_MissileDie` (`0x548A7`, 0), `Blocked_DoorRotate`, `SP_func_door`,
`SP_func_door_rotating`, `SP_func_static` (two) and `SP_trigger_damage`.
INFERRED: a client takes damage while playing; a death leaves it a
candidate until its own `ClientEndFrame`, which clears it when the script
has set `sessionstate` to `"dead"`; a spectator or a player at intermission
never is one.

`finishPlayerDamage` on a dead client. VERIFIED (`0x43C18..0x43CE4`): health
is decremented with no test of the session or of health beforehand, health
at or below 0 is raised to -999 when below it (`0x43C5C`), and the `die`
pointer is called only when non-null (`0x43C76`). INFERRED: a client hit
again after `player_die` cleared `die` (5.1 step 14) loses health below 0 and
does not die again.

Measured. VERIFIED, `client-probes/probe_blastloop` on retail,
four clients under sd on mp_carentan, the damage callback wrapped to log
each victim (rows in that directory's README):

| blast | victims, in call order, `iDamage` and health before |
|---|---|
| flat 20, line 1, both alive | 0: 20 (100), 1: 13 (100) |
| flat 200, line 1, slot 0 in front at 100 health, slot 1 behind at 1000 | 0: 200, killed; 1: 200 |
| flat 20, same frame | 0: 20 (`dead`, 0, left at -20); 1: 20 |
| flat 20, next frame | 1: 20 |
| flat 200, line 2, slot 3 in front at 100, slot 2 behind at 1000 | 3: 200, killed |
| flat 20, same frame | 3: 20 (`dead`, 0, left at -20); 2: 20 |
| flat 20, next frame | 2: 20 |

INFERRED, from the rows:

- Line 1's back player took the full 200 where a live player in front takes
  13 of a flat 20 (14.4): the front one was killed first, and its corpse
  contents (5.1 step 10) stopped no probe of the back one.
- Line 2's back player took nothing from the lethal blast and 20 from the
  next: it was walked before slot 3 was killed, behind a live body that
  stopped all five probes. The walk is not entity order: slot 2 came before
  slot 3 there and after it in the same-frame blast, once `player_die` had
  relinked slot 3. Retail's list is `SV_AreaEntities`', whose order is not
  read out here.
- A player killed in a frame is still a candidate for a second blast in the
  same frame, takes its callback with `sessionstate` `"dead"` and loses
  health below 0, and is no longer one a frame later.

The same probe checked 14.2: see there.

What a callback does to the rest of its own walk. VERIFIED,
`client-probes/probe_blastmove` on retail, three clients under dm
on mp_carentan at 1000 health set down at `probe_blastorder`'s first three
spots (walk order 1, 0, 2), flat-20 blasts from `(-176.8, 2473.1, 7)`, the
damage callback wrapped so the first victim of each walk does one thing
before it returns (rows in that directory's README):

| row | the first victim's callback | walk |
|---|---|---|
| plain | nothing | 1, 0, 2 |
| move_out | `setOrigin`s 0 and 2 out of the radius | 1 |
| move_in | 2 starts out of the blast's box; it `setOrigin`s 2 to `(-200, 2430)`, inside the radius | 1, 0 |
| ignore_mid | `setPlayerIgnoreRadiusDamage(true)` | 1, 0, 2 |
| ignore_after | (the next blast, flag still set) | none |
| nested | `radiusDamage` of 5 round slot 0 | 1, 0 (5, inside 1's callback), 0, 2 |
| kill | a lethal `radiusDamage` round 0 and round 2 | 1, 0 and 2 die inside 1's callback, then 0 and 2 `dead` |

INFERRED, from the rows: each candidate is measured where it stands when its
turn comes, so a victim moved out by an earlier callback is passed by; the
list is taken once, so one moved in from outside the box is never walked; the
walk reads the copy the builtin took of the flag, not the flag; a nested
`radiusDamage` runs its own walk to the end inside the callback, which is
`G_RadiusDamage` re-entered; and a victim killed inside the walk is still
reached, as 14.5's same-frame rows read.

The same probe's fourth client throws grenades (`--save-grenade`), and the
three stand round each grenade in flight; the first victim of each grenade
walk sets the other two out of reach. VERIFIED, retail, with the probe's
first glue (60 units round the grenade in its own plane): three walks, each
logged its first victim (slot 2, 99, 66 and 66 damage) and no other.
INFERRED: a grenade's walk measures each victim on its turn as the builtin's
does, which is the one `G_RadiusDamage` both reach (14.1).

**As implemented.** `Vitals::takedamage` on the host, written by the end
frame from `sessionstate`, by the `spawn` builtin and by `GameHost::die`;
`ScriptRuntime::deliver_hits` and `deliver_world_hit` drop a hit on a victim
without it, `G_Damage`'s first test. `finish_player_damage` takes health off
a dead client and clamps it at -999 without a second death. Both walks take
the client entities whose link box meets the blast's box, test each for
`takedamage` on its turn, measure it, and run its callback before the next:
the builtin through a `Cx::spawn_then` per victim whose
`Host::spawn_returned` takes the next turn (`builtins::combat::blast_step`),
the grenade pass (`Cod1Module::run_frame_cod1`) by measuring each candidate on its turn and
delivering its hit before the next. A client killed
earlier in the walk, or anywhere earlier in the frame, is left out of the
bodies that stop a probe. `a_blast_walks_its_victims_one_callback_at_a_time`
(`crates/server/src/game/builtins/combat.rs`) replays the rows above in
both slot orders; the same probe against `vcod-server` read every line-1 row
as retail did, and since the walk took the area tree's order (14.7) and
the player set down inside the flak88 clip stays there (mantle doc, "A
start inside a solid") every line-2 row too. A grenade blast reads victims
and bodies off the sims, except a client whose link a callback of the walk
moved (`setOrigin`, `client_link_origin`), which it measures and stands at
that link with the sim's box. `probe_blastmove` against `vcod-server` read
every builtin row above as retail did;
`a_callback_reaches_the_rest_of_its_walk`
(`crates/server/src/game/builtins/combat.rs`) replays them, and
`a_grenade_walk_meets_a_victim_where_an_earlier_callback_moved_it`
(`crates/server/tests/combat.rs`) the grenade case (the trap: reaching the
parked victim at the spot it left). The probe's grenade half measures the
walk's link order; 14.7, "The frame's links before the walk", has that run
on both servers.

### 14.6 Turrets, and the other entities with `takedamage`

VERIFIED, the stores to `gentity+0x171` listed in 14.5 by function:
`G_SpawnTurret` 1 (`0x5301a`), `SP_trigger_damage` 1 (`0x65341`, with
`health` 32000 at `0x65337` and `use`, `pain` and `die` set to
`Use_trigger_damage`, `Pain_trigger_damage` and `Die_trigger_damage`),
`SP_func_static` 1 (`0x5828a`, `0x582d0`, with `pain` `Static_Pain` and a
store of `health` 9999 at `0x582d7`), `SP_func_door` 1 (`0x56fb7`). VERIFIED,
the entity lumps of every BSP in the 1.1 paks: no `maps/MP/` map places a
`func_door`, `func_static` or `trigger_damage`; `mp_brecourt`,
`mp_carentan`, `mp_dawnville`, `mp_hurtgen`, `mp_railyard` and `mp_rocket`
place `misc_mg42`s, and the single-player maps place all four. INFERRED: on
a stock MP map the turrets are the only entities with no client a blast
reaches.

What a blast does to one, read off 4.2's entity arm of `G_Damage`
(`0x49e70..0x4a08b`). VERIFIED: the offsets, immediates, `scr_const` slots
and call targets in this list. INFERRED: the ordering and every condition.

- A null inflictor or attacker is replaced by `g_entities + 0xc49d8`, the
  world (`0x49e83`, `0x49e8e`).
- `s.eType` 5 takes the `use`-pointer arm (`0x49e93`); a turret is 11.
- `flags & 1` returns (`0x49efd`), a damage at or below 0 becomes 1
  (`0x49f10`), and `health` (`+0x230`) takes the damage off (`0x49f3e`).
- `Scr_Notify(ent, scr_const+0x1A, 2)` with the attacker pushed first and
  the damage second (`0x49f47..0x49f6a`). `scr_const+0x1A` is `"damage"`
  (12.2), so a waiter reads `waittill("damage", amount, attacker)`.
- With health at or below 0: clamped at -999 (`0x49f83`),
  `Scr_Notify(ent, scr_const+0x1C, 1)` with the attacker (`0x49f8d..0x49fa4`),
  where `GScr_LoadConsts` allocates `scr_const+0x1C` from `"death"`
  (`.rodata 0x75f9e`, `0x586b8`), the attacker stored at `+0x258`, and
  `die` called when non-null. With health above 0, `pain` when non-null.
- INFERRED, off `cod11-turrets.md` 3: a turret has neither, and nothing
  on this path clears `takedamage`, so every later hit notifies `"death"`
  again.

VERIFIED: `G_SpawnTurret` stores 100 to `health` behind a compare of it
against 0 (`0x52ee7`, `0x52ef0`) and links with `r.mins` (-32, -32, 0) and
`r.maxs` (32, 32, 56). The `G_RadiusDamage` arms 14.1 and 14.3 give a
turret: `r.bmodel` 0, so the distance is origin to origin, and `CanDamage`'s
no-client arm, the box midpoint 28 above the origin and four points 15 off
it on x and y, any one clear for the whole falloff.

VERIFIED, `client-probes/probe_victims` on retail: carentan's
`misc_mg42`s, entities 297 and 298, read `health` 100. Three flat-60
`radiusDamage`s 70 units from 298 with 1000-health slot 1 inside the radius
too logged, per blast, slot 1's callback, the line after the builtin, then
the turret's waiters:

| blast | slot 1's callback | after the builtin | turret waiters, in run order |
|---|---|---|---|
| 1 | 60 | `health` 40 | `damage` 60, attacker 1022 |
| 2 | 60 | `health` -20 | `death` 1022, then `damage` 60 1022 |
| 3 | 60 | `health` -80 | `death` 1022, then `damage` 60 1022 |

INFERRED: the notifies wake their waiters after the builtin's caller has
moved on, and the later notify's waiter runs first, the newest-first resume
order of AGENTS.md's script notes. Where the turret falls in the walk
relative to slot 1 is not visible here, since its notifies are deferred.

**As implemented.** `GameHost::blast_entities` lists the turrets with
`G_SpawnTurret`'s box, `Blast::entity_damage` measures one
(`crates/server/src/game/combat.rs`), and `GameHost::damage_entity` is the
entity arm of `G_Damage`: health, `"damage"`, and `"death"` at or below 0.
Both walks take the turrets where the area tree lists them among the
clients (14.7): the `radiusDamage` builtin with the world as the attacker
and its notifies raised through the builtin's `Cx`, a grenade with its
thrower. The map load
gives a turret 100 `health` when its key left 0. The same probe against
`vcod-server` read the table above row for row, and
`a_blast_damages_a_turret_and_notifies_it`
(`crates/server/src/game/builtins/combat.rs`) replays it. A `func_static`
with `takedamage` is listed too, measured by brush-model distance
(`r.bmodel`), and takes `G_Damage`'s `eType` 5 arm, a `"trigger"` notify and
`Use_Static`, never `health` (`cod11-movers.md` 21). A `trigger_damage` is
listed too, measured off its brush model's box (14.8). Not modelled:
doors, whose spawn vcod does not run.

A bullet never reaches a turret. VERIFIED: `G_SpawnTurret` writes
`r.contents` `0x200004` and is the only writer of that value in the module;
the bullet's trace mask is `0x2802031` (2.3), as is the melee trace's (2.5);
the engine's per-entity clip (`cod_lnxded` `0x809105c`) begins with a test
of the trace's mask against the entity's `r.contents`. INFERRED: the two
share no bit, the clip returns at that test, and a round passes through a
mounted MG42 as through air, so `G_Damage` never sees a turret from a gun.
vcod's shot trace has no turret in it and does the same. Not measured on a
live server.

### 14.7 The walk's order: the area tree

`trap_EntitiesInBox` is the engine's, so the order 14.5 measured is read out
of `cod_lnxded` (1.1d, stripped).

VERIFIED: the game-module syscall dispatch sends 0x32 to `0x80908b0`, 0x33
to `0x8091f94` and 0x34 to `0x805a540`. INFERRED: those are
`SV_LinkEntity`, `SV_UnlinkEntity` and `SV_AreaEntities`, by the
`"CM_AreaEntities: MAXCOUNT\n"` string inside the walk `0x805a540` calls and
by `0x80908b0` building `r.absmin`/`r.absmax` as 14.1 reads them.

**The tree.** VERIFIED: the addresses, sizes, offsets and constants in this
list. INFERRED: what each function does with them, read off the decompiled
control flow, and every condition and ordering.

- `0x8058dd0`, called once, from `CM_LoadMap` (`0x804b3c0`), builds the root
  at `0x831b2a8` over inline model 0's x and y bounds, and threads a pool of
  0x400 nodes of 0x28 bytes from `0x831b2fc` into a free list at
  `0x831b2d0`, linked through `+0x1C`; a sentinel at `0x831b2d4` stands for a
  missing child. The root splits on axis 1 when the x extent is not larger
  than the y extent, 0 otherwise, at the midpoint (`* 0.5`,
  `.rodata 0x80cd474`). The model bounds are the BSP lump's grown by one unit
  each way, by the submodel loader `0x804a81c` (the `- 1.0`/`+ 1.0` pair).
- A node holds its axis (`+0`), a static-model mask (`+4`, `+8`), an entity
  contents mask (`+0xC`), `dist` (`+0x10`), an entity list head (`+0x14`), a
  static-model list head (`+0x18`), its parent (`+0x1C`) and two children
  (`+0x20`, `+0x24`).
- `0x8059344`, the entity link, walks down from the root, OR-ing the entity's
  contents into each node's `+0xC`: to `+0x20` when its `absmin` on the
  node's axis is above `dist` (`fcom` at `0x80593fc`), to `+0x24` when its
  `absmax` is below it (`0x805941d`), and it stops at a node the box touches
  or straddles, or whose child on that side is missing. Each step down
  narrows a copy of the world's xy bounds to that side before the child is
  looked at, so a walk that stops for a missing child holds the half it was
  heading for; one that stops on a straddle holds the node's own region.
- When the node it stops at is the one the entity sits in and the new
  contents keep every bit the stored ones had (`(old & ~new) == 0`, stored
  at svEntity `+0x168`), it refreshes the stored contents and xy box
  (`+0x16C..+0x178`) and keeps its place: on a straddle it returns there,
  and for a missing child it runs the push-down below first. Otherwise it
  unlinks (`0x8058e9c`) and walks again from the root, and an entity with no
  node is prepended to the node's list, its contents and box stored, and the
  push-down run.
- `0x8058f80`, the push-down, walks the node's entity list from the head,
  then its static list, and moves each entry that lies wholly on one side of
  `dist` by its stored box into that child, prepended. A missing child is
  taken from the pool, cut on the region the walk handed in (the longer of x
  and y, y on a tie, at its midpoint, `.rodata 0x80cd47c`), not on the
  child's own half of it. The pool is tested first, then the region's extent
  on the chosen axis against 512 (`.rodata 0x80cd478`); an empty pool or an
  extent of 512 or less returns from the whole pass, the static list
  included. It goes down one level per call.
- `0x8058e9c`, the unlink, takes the entity out of its list, then frees every
  node on the way up that has no entities, no static models and no children
  back to the pool (pushed on the free list's head, the parent's child slot
  set to the sentinel), and recomputes `+0xC` on every node above as its
  children's masks OR its own entities' current `r.contents`.
- `0x8059590`, the box query, does nothing at a node whose `+0xC` misses the
  query's mask; otherwise it walks the node's own list from its head, takes
  each entity whose current `r.contents` meets the mask and whose
  `r.absmin`/`r.absmax` touch the box, then recurses into `+0x20` when
  `dist` is below the box's max on the axis and into `+0x24` when the box's
  min is below `dist`.
- `SV_LinkEntity` (`0x80908b0`) unlinks an entity whose `r.contents` is 0.
  Otherwise the xy box it files the entity under is `r.absmin`/`r.absmax`,
  except for an entity with a server DObj (`0x806e498`, the table at
  `0x812a600`) and `r.svFlags & 6`: with bit 2, the origin's x and y plus
  (-64, -64) and (64, 64) (`.data 0x80e30d0`, `0x80e30d4`, `0x80e30dc`,
  `0x80e30e0`); with bit 4 alone, the model's bounds (`0x80c4f6c`) about the
  origin.
- The static models: `CM_LoadMap` calls `0x80515d4` after `0x8058dd0`, which
  reads every `misc_model` of the entity string (the `classname`, `model`,
  `origin`, `angles`, `modelscale_vec` and `modelscale` keys) and hands each
  to `0x8051420`. That one builds the scaled axis, and `0x80c241c` takes the
  eight corners of every collision surface's stored bounds (surface `+0x8`
  and `+0x14`) through it; the origin is added and the model is linked by
  `0x80594c4` when its masked collision contents (`0x80c2b48`, the word at
  model `+0x48`) are non-zero. `0x80594c4` walks as the entity link does,
  stops on a straddle or a missing child, prepends to `+0x18` and runs the
  push-down.

INFERRED, from the above: a list holds the most recently prepended entry
first; an entity relinked in the node it sits in with no contents bit
dropped keeps its place; a static model, never unlinked, keeps every node on
its path alive, so the tree under a map's props is built at the load and
the entities share it; and a push-down that moves several entries into one
child reverses them.

**Who links, and with what.** VERIFIED, `game.mp.i386.so`, the call sites:
`ClientThink_real` links once per usercmd at `0x40595`, after `ClientEvents`
(`0x40589`, the cmd's shots) and before `G_TouchTriggers` (`0x405b3`);
`SpectatorThink` unlinks at `0x3fc2f`; `ClientSpawn` unlinks at `0x42708`
and calls `ClientEndFrame` (`0x42a75`) and `ClientThink_real` (`0x42a82`),
and has no link of its own; the player `setOrigin` method (`0x43480`)
unlinks at `0x434ee` and links at `0x4356c`; `player_die` writes contents
`0x4000000` (`0x49c28`), unlinks at `0x49d68`, links at `0x49d7d` and again
at `0x49d9d`; `G_SpawnTurret` links at `0x53025` with contents `0x200004`;
`cloneplayer` (`0x4450c`) links its corpse at `0x446ae` and never writes its
contents; `G_TempEntity` links at `0x67a18`; `G_FreeEntity` unlinks at
`0x66aab`; `ClientEndFrame` has no link outside the gunner's turret think.
VERIFIED: `r.svFlags` bit 2 is set only by `ClientEndFrame` (`0x40fa0`, on
the playing arm) and bit 4 only by `SP_script_model` (`0x61020`); the
server DObjs are made by `G_DObjUpdate` and `BG_UpdatePlayerDObj`, and no
corpse has one. INFERRED: a playing client is filed under the 128-unit
square about its origin, from its spawn on, since `ClientSpawn` reaches its
link through `ClientEndFrame`; a client's cmd relinks keep its place while
it stays in one node; a `setOrigin`, a spawn and a death each put it at the
head of the node it lands in; a corpse and a temp entity link with contents
0, so neither is ever in the tree; a dead client leaves the tree at its
first cmd after the end frame that zeroed its contents.

**map_restart.** VERIFIED: `0x8058dd0` has the one caller in `CM_LoadMap`,
and the restart path (`0x8083de4`, `0x8089350`) does not reach it.
VERIFIED: `G_ShutdownGame`'s loop (`0x4ff06..0x4ff4a`) calls `G_FreeEntity`
on every entity in use, and the restart runs it. INFERRED: a restart keeps
the nodes and the static models where the previous level's push-downs left
them, and starts with no entity in the tree.

**Measured.** VERIFIED, `client-probes/probe_blastorder` on retail, four clients under sd on mp_carentan at 1000 health, flat-20
blasts from `(-176.8, 2473.1, 7)`, the `cb` order of each:

| step | `setOrigin` before the blast | walk |
|---|---|---|
| placed | 0 to (-290, 2430), 1 to (-260, 2480), 2 to (-230, 2380), 3 to (-230, 2540) | 1 0 3 2 |
| moved_within | 0 to (-290, 2440) | 0 1 3 2 |
| moved_into | 2 to (-200, 2430) | 2 0 1 3 |
| moved_out | 1 to (-290, 2380) | 2 0 3 1 |

INFERRED, from the rows and the model above: carentan's root splits x at
1152 and its child below at y 2464; slots 0 and 1, whose 128-unit squares
straddle y 2464, sit in that child's list in reverse `setOrigin` order; 3,
wholly above, is walked before 2, wholly below; slot 0's move of ten units
within the node still puts it first, the `setOrigin` unlink; a move into
the node lands at its head and a move out of it to the far side lands
last. 14.5's rows follow the same way: line 1's slot 0 straddles the split
and slot 1 is wholly below it, so 0 is walked first whatever the
`setOrigin` order; line 2's two straddle it and were set down 3 then 2, so 2
comes first until `player_die` puts 3 at the head.

**The frame's links before the walk.** VERIFIED, `game.mp.i386.so`, the
call sites in `G_RunFrame` (0x50478): the touch queue's
`Scr_RunCurrentThreads` at 0x50653, `Scr_SetTime` at 0x5070c, and the
entity loop at 0x50930..0x50975, which calls `G_RunEntity` (0x502bc) at
0x50955 for every in-use entity (and at 0x50949 for its link parent first);
`G_RunEntity` calls `G_RunMissile` for `eType` 4 at 0x50375. INFERRED, off
that instruction order and no jump back from the loop: a grenade's flight
and its blast run after the frame's script threads, so a thread reads the
grenade's last-frame origin and every `setOrigin` a thread makes that frame
has relinked its player before the walk.

VERIFIED, `client-probes/probe_blastmove`'s grenade half on retail, three clients and a `--save-grenade` thrower under dm on
mp_carentan: while a grenade is live the thrower is parked out of reach and
a thread sets the three down every frame side by side over the grenade's
`origin`, at (0, -32, 60), (0, 0, 60) and (0, 32, 60) for slots 0, 1 and 2,
in slot order on a frame whose `getTime()` is a multiple of 100 and in
reverse on the others. The first victim of each walk:

| walk at | this frame's last link | last frame's last link | first victim |
|---|---|---|---|
| 62350 | 0 | 2 | 0 |
| 72400 | 2 | 0 | 2 |
| 81050 | 0 | 2 | 0 |

INFERRED, from the rows: the three sit in one node (their 128-unit squares
are 64 units apart) and the walk starts at the player this frame's thread
linked last, so the blast went off after that thread's links. The same probe
against `vcod-server` with the missile pass ahead of the script frame took
the last frame's (55800: 0, 65850: 2, 74550: 2, each the other parity's
last link); with the pass moved behind it, this frame's (53200: 2, 59750: 0,
68300: 2). A middle build that ran the pass after the threads but before
their `setOrigin`s reached the sims traced the flight against the bodies
where the last frame left them: the thrower's first grenade bounced off slot
1, set down over the thrower the frame before, at `trDelta`
(0, -120.5, -10.1), an eighth of the throw, where retail's flew on at 960
(`--save-grenade` captures of both, kept out of the fixtures).

vcod: `Cod1Module::run_frame_cod1` runs the entity pass (below) after
`ScriptRuntime::run_threads` and every client write the frame's threads
queued (spawns, `setOrigin`s, `linkTo` re-anchors, mover pushes), then
applies what the walks' callbacks queued.
`a_grenade_walk_meets_this_frame_s_script_links_first`
(`crates/server/tests/combat.rs`) is the two-client form of the probe.

**One entity pass.** VERIFIED, `game.mp.i386.so`, the call sites in
`G_RunFrame` (0x50478): `Scr_SetTime` at 0x5070c; past it a loop at
0x50912..0x50975 that compares its counter with `level.num_entities` (0x50927,
0x5096f), tests the in-use byte `+0x160` (0x50930) and the link record
`+0x2e4` (0x5093f), and calls `G_RunEntity` (0x502bc) on that record's
first dword (0x50949) and on the entity (0x50955). VERIFIED, `G_RunEntity`'s
compares and calls: `+0x194` against `level.framenum` (`level+0x1e4`,
0x502cb) and a store of it (0x502d7); `level.time - ent+0x180` against 300
(0x50309), `G_FreeEntity` (0x5031d) and `trap_UnlinkEntity` (0x5033e);
`eType` against 4, 3, 5 and 8 (0x5036c, 0x50380, 0x50400, 0x50405), the
link record (0x50385), byte `+0x161` (0x503e8) and the client pointer
(0x50415); `G_RunMissile` (0x50375), `G_GeneralLink` (0x50392), `G_RunItem`
(0x503f5), `G_RunMover` (0x5040e), `G_RunClient` (0x50422) and the think
pointer (0x503e1, 0x5046f). INFERRED, off those branches and their order:
each in-use entity runs once a frame, its link parent first, through the
first arm its type matches (a missile, a linked item's link, an item, a
mover, a client, a think), so items, links, movers, missiles and thinks
are one pass by entity number. A blast, which runs inside `G_RunMissile`
(14.1), meets the entities numbered below its grenade on this frame's turn
and the ones above it on the last frame's, and an entity spawned during the
pass runs the same frame when its number is past the loop's, since the
bound is reread.

VERIFIED, `client-probes/probe_blastlink` on retail, dm on
mp_carentan: a `script_origin` parent moved 8 units up or down every frame,
the 40 `mp_deathmatch_spawn` points (numbers 78..110 and 261..294) linked to
it from the start, and 8 spawned `script_origin`s linked once the first
grenade was live. In the victim's damage callback, per child, how far it sat
off the gap it linked at:

| grenade | walk at | children below its number | children above it |
|---|---|---|---|
| 170 | 24450 | 9, all 0 | 31, all 8 |
| 180 | 34450 | 17, all 0 | 23, all 8 |
| 243 | 43150 | 17, all 0 | 23, all 8 |

VERIFIED, the same log: every `MOD_FALLING` callback (inside a cmd, ahead of
`G_RunFrame`) read all 40 at 0. The same probe against `vcod-server` with
the pass split by kind read 0 for all 40 in every walk; with the one pass,
9/31, 17/23 and 17/23 split as retail's, at grenades 170, 179 and 180 (ours
hands out free numbers in another order).

vcod: `ScriptRuntime::run_entity_pass` is the loop: a temp entity's free
(below), a missile (`Missiles::run_one`), a link (`link::run_linked`), a
mover (`mover::run_one`), an item (`item::run_item`), then the entity's
think (`GameHost::run_entity_think`) and an unmanned turret's
(`GameHost::run_turret_think`); the body queue's think on its numbers
64..71; and on a linked client's number, after its parent's turn, the
`G_RunClient` re-anchor and relink. Each entity runs once a frame, its link
parent first. The loop returns at each turn that needs the client sims (a
blast, a mover's push, a linked client), which `Cod1Module::run_frame_cod1` handles before
calling it again from the same place.
`a_blast_meets_the_links_below_its_grenade_s_number_only`
(`crates/server/tests/combat.rs`) is the probe's two-client form.

**The pass runs after the threads.** VERIFIED, `client-probes/probe_entnum`
on retail, mp_carentan, no client: a `script_origin` spawned and
`delete()`d at 2500 read `isdefined` 1 at 2500, 2550 and 2600 and 0 at 2650.
The `delete` method (0x5da14) arms `G_FreeEntity` 100 ms out
(cod11-gsc-object-model.md section 14). VERIFIED, `game.mp.i386.so`:
`G_RunEntity`'s think arm (0x50430..0x5046f) fires a think whose
`nextthink` (`+0x1fc`) is positive and not past `level.time`. INFERRED, off
those: the think came due on the 2600 frame and ran in that frame's entity
pass, after the thread that read 1 on it; the pass sits past `Scr_SetTime`
(0x5070c), which wakes the frame's waits; and the defer is 100 ms, since 50
would have read 0 at 2600 and 150 would have read 1 at 2650. VERIFIED, the same run: a
`script_model` given `moveto((0, 0, 100), 0.1)` at 3650 woke its
`waittill("movedone")` at 3800. INFERRED: `G_RunMover` raised the notify on
the 3750 pass, the frame the trajectory ended, and the waiter ran on the
next frame's threads, which is the one-frame lag cod11-movers.md section 8
measured. Before this, vcod ran thinks and movers ahead of the threads,
with the mover's fields evaluated a frame back to fake the lag; the same
probe against it read `isdefined` 0 a frame early (2550, the delete at
2450).

#### Entity numbers

VERIFIED, `game.mp.i386.so`: `G_Spawn` (0x667e0) pops the head of a free
list at `level+0x10` (0x668c8..0x668e1), the next link at `ent+0x304`,
clearing the tail `level+0x14` when the list empties, and otherwise takes
`level.num_entities` (`level+0xc`) and increments it (0x6688b..0x668a3),
with a `G_Error` once that reaches 0x3fe and nothing is free (0x667f7,
0x66883). No `freetime` is read: Q3's "not reused for a second" rule is not
in this module. VERIFIED: `G_FreeEntity` (0x66948) zeroes the entity
(0x66b97), and when its number is above 0x47 (0x66bae) appends it to the
tail (0x66bb3..0x66bd8), the body queue never. VERIFIED: `G_TempEntity`
(0x67938) gets its entity from `G_Spawn` (0x67947). VERIFIED,
`G_RunEntity`: when `level.time - ent+0x180` (`eventTime`) is above 300
(0x50309) and `ent+0x184` (`freeAfterEvent`) is set it calls
`G_FreeEntity` (0x5031d), and a set `freeAfterEvent` returns before any arm
(0x5035c). INFERRED: the free list is first in, first out, and every temp
entity, a bullet impact, a `playFx`, an obituary, a teleport's pair, takes a
number from the same list as a script `spawn`, holds it on the wire until
the first pass more than 300 ms past its event and then goes to the back of
the queue.

VERIFIED, `probe_entnum` on retail, the same run: `a` spawned 299, the
`playFx` after it took 300, and `b` after that 301; spawns at +300 and +350
from the event took 302 and 303, the one at +400 took 300 and the one at
+450 304. INFERRED: the temp entity was freed on the +350 pass, the first
whose `level.time - eventTime` is above 300, behind the threads that took
303. That is why the blast probe's third grenade was 243 on retail: the
numbers its frames' temp entities cycled through were ahead of it on the
queue. VERIFIED, `client-probes/probe_entfree` (a scratch probe, not kept)
on retail and on `vcod-server`, stock dm on mp_carentan: 40 spawns one second
in took 170..180, 182, 243, 245, 246, 252, 258, then 299 up, on both: the
numbers dm's `_gameobjects` deletes at the start, in the order the pass freed
them. INFERRED, off that list and 13.2's concussion entity: the blast probe's
first grenade took 170, the eight children 171..178, its concussion entity
179, the second grenade 180, its concussion entity 182, and the third
grenade 243. vcod had kept temp entities in a block of its own at 958..1021,
one frame each, spawned no concussion entity, and gave the three 170, 179
and 180; with both ported, `probe_blastlink` against `vcod-server` reads
170, 180 and 243 and the 9/31, 17/23, 17/23 splits retail read.

vcod: `ObjectTable::spawn` and `free` are the free list.
`GameHost::add_temp_entity` takes a number from `spawn` for each event, the
snapshot build sends every live temp entity at its number each frame, and
`GameHost::run_temp_entity` frees it on its turn of the pass.
`GameHost::spawn_concussive` is 13.2's concussion entity, spawned after each
blast walk and by `grenadeExplosionEffect` (13.5), and freed by
`ThinkFn::Concussive`.
`a_temp_entity_takes_a_spawn_number_and_frees_it_past_300_ms` and
`a_delete_s_free_runs_after_the_threads_of_its_frame`
(`crates/server/src/game/script.rs`) are the probe's two halves; against
`vcod-server` it reads line for line as retail does, times included
(`cod11-gsc-language.md`, "The script clock").

**Other callers.** VERIFIED: `trap_EntitiesInBox` is also called by
`G_TouchTriggers` (`0x3f925`, mask `0x405c0008`), `G_GetActivateEnt`
(`0x4f237`, `0x200000`), `positionWouldTelefrag` and `G_KillBox`
(`0x2000000`), `G_MoverPush` (`0x2000180`), and
`G_CheckHitTriggerDamage` and `G_GrenadeTouchTriggerDamage` (`0x400000`).
INFERRED: `G_GetActivateEnt` sorts its candidates by score, so the list
order reaches it only through ties; `G_TouchTriggers` touches every hit in
list order, so the order shows only where two touches' side effects meet
(two weapons on one spot); the rest walk the whole list.

**Every other link.** VERIFIED, `game.mp.i386.so`, the stores to
`r.contents` (`ent+0x118`) and the link calls beside them: `G_SpawnItem`
writes 0x407c0108 (0x4e778) and links (0x4e847), and `FinishSpawningItem`
links again after its drop (0x4e4f2); `LaunchItem` writes 0x407c0108
(0x4dc97) and links (0x4dd2f); `G_RunItem` links (0x4ec19); `Touch_Item`
writes 0 (0x4d90c, 0x4d9ce) and links (0x4da33); `RespawnItem` writes
0x407c0008 (0x4ece9) and links (0x4ed05). `SP_trigger_multiple` and
`SP_trigger_once` write 0 with `spawnflags & 8`, else 0x40000000 (0x64d66,
0x64d79; 0x65cb1, 0x65cc4), OR 0x40000, 0x80000 and 0x100000 into it for
`spawnflags` 1, 2 and 4 (the byte at `+0x11a`, 0x64d83..0x64dac,
0x65cce..0x65cf7), and link (0x64db7, 0x65d02); `SP_trigger_damage`
writes 0x405c0008 (0x653cd) and links (0x653ec); `SP_trigger_lookat` writes
0x20000000 and links (0x65e03, 0x65e1f); `SP_trigger_hurt` writes 0x405c0008
(0x64f6c) and has no link of its own; `trigger_use` links (0x5744b) and then
writes 0x200000 (0x5745a). Each of them calls `trap_SetBrushModel` first,
and `cod_lnxded`'s `0x8089544` behind it sets the submodel's bounds into
`r.mins`/`r.maxs`, `r.bmodel` 1, `r.contents` -1, and calls `SV_LinkEntity`.
`SP_script_brushmodel` sets the brush model, writes 1 (0x60fd4) and links
(0x60fe5); `SP_script_model` writes 0x2080 (0x61010), ORs 4 into
`r.svFlags` and links (0x61028); `SP_script_origin` writes 0 (0x61047) and
links (0x61055). `Scr_SetOrigin`, the `origin` field's setter (0x5c61c),
links (0x5c64f); `Scr_SetAngles` (0x5c660) does not. `G_RunMover` reaches
`G_MoverTeam` only when `s.pos.trType` or `s.apos.trType` is non-zero
(0x57666..0x57670), and `G_MoverPush` links the pusher at its new
`r.currentOrigin` (0x553ae). `fire_grenade` and `fire_rocket` write no
contents (cod11-movers.md 12). VERIFIED, `cod_lnxded` `SV_LinkEntity`: a
brush model whose `r.currentAngles` are all zero takes `r.absmin`/`r.absmax`
as origin plus `r.mins`/`r.maxs`, and one with any angle set a cube of
`RadiusFromBounds` (0x8066e54, called at 0x8090b20) about the origin;
either is then grown one unit. VERIFIED, `cod_lnxded` 1.1d, the bounds a
`script_model` is filed under, traced word for word: `SV_LinkEntity`
hands `0x80c4f6c` the server DObj (0x8090ea2); `0x80c4f6c` passes the
DObj's first model, `+0x18` (the model array whose count byte is `+0x16`,
as `0x80c53d0` walks it), to `0x80c28e0`, which copies that model's
`[+0x4]+0x4c..+0x60` out as mins and maxs. The xmodel loader `0x80c13f8`
hands its file to `0x80c1164`, which checks version 14 (0x80c1176) and
stores the six floats after it at `buf+0xc0c..+0xc20` (0x80c119d..
0x80c11d4, `buf` the loader's `ebp-0xc28`); the loader copies
`ebp-0x1c..-0x8`, those six, to the new record's `+0x4c..+0x60`
(0x80c1781..0x80c17a2) and stores the record at the model's `+0x4`
(0x80c17b2). So the box is the `.xmodel` descriptor's own mins and maxs,
the base model's only (vcod: `model::bounds`).

VERIFIED, the entity lumps of every `maps/mp/*.bsp` in the stock paks: no
`trigger_multiple` or `trigger_once` carries `spawnflags & 8`; the values
seen are 0, 2 and 7.

INFERRED, from the above: items lying in the world, triggers but a
`spawnflags & 8` one with no 1, 2 or 4, `script_brushmodel`s and `script_model`s are in the
tree from their spawn; missiles, temp entities, corpses, `script_origin`s
and taken items never are. VERIFIED, `tools/re/annotate_func.py`: 1.1's
`fire_grenade` and `fire_rocket` store to `+0xf4` (0x88, at 0x54415 and
0x54687) and `+0x198`, never to `r.contents` (+0x118); UO's `fire_grenade`
writes `r.contents` 0x2000 (+0x120, 0x5fbc3) on the arm that makes a
`takeDamage` grenade shootable, and its `fire_rocket` and `fire_artillery`
write none. INFERRED: `G_RunMissile`'s `trap_LinkEntity` every frame is
`SV_LinkEntity`'s unlink for every missile but UO's shootable grenades,
which is why vcod links those alone (`GameHost::link_shootable`). A `trigger_hurt` stays where its contents -1
link filed it and the walk tests its 0x405c0008; a `trigger_use` the same
with 0x200000. A `notSolid` brush model stays linked with contents 0.

**The touch pass.** VERIFIED, `G_TouchTriggers` (0x3f88c): the walk at
0x3f925 with mask 0x405c0008, then one loop over the list in its order
(0x3f9aa..0x3faa2). Per entity: it skips one whose `touch` (`+0x20c`) is
null when the player's is null too (0x3f9cd..0x3f9e0); tests `eType` 3
(0x3f9e6) and calls `BG_PlayerTouchesItem` for it (0x3fa03) or
`trap_EntityContact` for anything else (0x3fa16); on a hit, with the script
system up, notifies the entity `"touch"` with the player (0x3fa4d) and the
player `"touch"` with the entity (0x3fa6f); then calls the touch function
(0x3fa8b). VERIFIED: `SP_trigger_multiple` and `SP_trigger_once` set
`Touch_Multi` (0x64cdd, 0x65c22), `SP_trigger_hurt` `hurt_touch`
(0x64fe2), and `SP_trigger_damage` sets a `use` and no touch (0x65348).
INFERRED: items and triggers are touched in one walk, in the tree's order,
and a `trigger_damage` is never touched.

VERIFIED, `client-probes/probe_touchorder` on retail, one
client under dm on mp_pavlov, standing at (-10936.5, 12051.0, -29.7), where
the minefield triggers 101 and 102 overlap; three `item_health` spawned
there 8 units apart in the order a, b, c (77, 78, 79), the player at full
health so none is taken. Per phase, the lines of the first cmd of a frame:

| phase | before it | lines |
|---|---|---|
| spawned | | `f 101`, `f 102`, `t c`, `t a`, `t b`, `mt 101`, `pt 102`, `mt 102` |
| kept | a's `origin` one unit along x | the same |
| moved | a's `origin` 4000 units along -y, then back | `f 101`, `f 102`, `t c`, `t b`, `t a`, `mt 101`, `pt 102`, `mt 102` |

`f` is a trigger's `"trigger"`, `t` an item's `"touch"`, `mt` a mine's
`"touch"` and `pt` the player's `"touch"` with the entity it names. VERIFIED,
the same log: the `f` pair repeats once per further cmd of the frame, and
no `t`, `mt` or `pt` line does.

VERIFIED, the same probe with `probe_mode ammo`: three `mosin_nagant_mp`
items spawned the same way as d, e, f, `count` 5 each, the player's mosin
reserve set to 149 of 150; `PROBE take e` and no other take.

INFERRED, from the ammo run: the walk meets e first, since only the first
grab finds room in the reserve. vcod's tree replaying the same link history
lists the five as 102, 101, b, a, c after the spawn and after `kept`, and
102, 101, a, b, c after `moved` (`touch_pass` trace on `vcod-server`, below),
and lists e first in the ammo run. INFERRED: those lists are retail's too:
running each phase's woken threads in the reverse of the order they were
woken, the first notify of a frame waking each waiter, gives every `t`,
`mt` and `pt` line in the table from them, and the `f` pair in the same
reverse. VERIFIED since, `G_RunFrame` 0x50540..0x50665: the `"trigger"`
of a `trigger_multiple` is not notified in the cmd but queued, and the
frame hands the queue out one trigger per entity a round with
`Scr_RunCurrentThreads` after each round, which is why the `f` pair comes
once a cmd while a `"touch"`, notified in the cmd to a waiter that has not
run again by the next one, comes once a frame (`cod11-gsc-language.md`,
"Engine notifies and the trigger queue"). VERIFIED, a rerun
(`crates/server/tests/fixtures/touchorder/mp_pavlov.txt`): every frame of a
phase repeats the first's `t`, `mt` and `pt` lines after its first `f` pair.

**As implemented.** `crate::area::AreaTree` is the tree above, port for
port: the lazy split on the region handed in, the one-level push-down, the
512 floor, the 0x400 pool, the freeing unlink, the keep-your-place relink
and the walk. `GameHost::area` holds it; `AreaTree::for_map` builds it at a
map load from inline model 0 and `vcod_common::props::area_bounds` (the
static models' boxes, as `0x80c241c` takes them), and a `map_restart`
carries it with every entity unlinked. Every link goes through
`AreaTree::link`. A client's: after each cmd's shots and before its touch
pass (`Server::replay_moves`, contents as the sim's end frame left them, 0
for a spectator, none at intermission), `self spawn` and `setOrigin` as an
unlink and a link, `GameHost::die` as `player_die`'s unlink and corpse
link, an unlink at a disconnect. Every other entity's through
`GameHost::link_entity`, off the `LinkShape` (`r.contents`, `r.mins`,
`r.maxs` and the box rule) its spawn function set: a turret at its spawn;
a trigger's two links at the map load (`trigger::link`); a
`script_brushmodel`'s two and a `script_model`'s one at the map load or a
script `spawn`; an item at `G_SpawnItem`, `LaunchItem`, its taking and its
respawn; any `origin` write on an entity with no client
(`Host::set_field`, which is how `Scr_SetOrigin`, an item's flight and its
drop to the floor link); a mover once a frame while it moves, at its clip
pose, as an unlink and a link (`mover::run_one`, `G_MoverPush`'s 0x55315 and
0x553ae); what a push lists, as `G_MoverPush` relinks it
(`crate::push::push`, cod11-movers.md 12). `notSolid`/`solid` on a `script_brushmodel` write the contents
without a link (`AreaTree::set_contents`), as `trigger_hurt` and
`trigger_use` do at their spawn; a delete and a free unlink. The walks:
both blast walks take their candidates from `entities_in_box` over the
blast's box and keep the clients and turrets, in the tree's order (the
`radiusDamage` builtin's `builtins::combat::blast_candidates` and the
grenade pass in `Cod1Module::run_frame_cod1`); the touch pass (`trigger::touched`, run by
`ScriptRuntime::touch_triggers_at` per cmd) walks it with mask 0x405c0008
and touches items and triggers in that one order, each with the two
`"touch"` notifies first. `probe_blastorder` against `vcod-server` read every `cb` row
above in retail's order, and `probe_blastloop`'s `same_frame_high` now runs
3 then 2. `probe_touchorder` against `vcod-server` takes e in the ammo run,
as retail does, where the entity-number order took d.
`the_tree_replays_probe_blastorder_on_mp_carentan`
(`crates/server/src/area.rs`) and
`radiusdamage_walks_its_victims_in_the_area_trees_order`
(`crates/server/src/game/builtins/combat.rs`) replay the blast table.

The other walks and links:

- `positionWouldTelefrag` walks the tree under 0x2000000 over
  `playerMins`/`playerMaxs` at the point and answers 1 off the first
  client whose `pm_type` is below 6 (`builtins::client::position_would_telefrag`,
  cod11-gsc-object-model.md 20).
- `G_GetActivateEnt` takes its candidates off the walk over the muzzle
  ±(192, 192, 96) under 0x200000, in its order, and scores each at its
  link's `(absmin + absmax) * 0.5` (`item::activate_ent`, cod11-items.md
  2.1). VERIFIED, the relocation at 0x4f421: the sort is the C library's
  `qsort`. INFERRED: glibc's `qsort` merges when it can allocate, which
  keeps ties in walk order, so vcod sorts stably.
- `G_KillBox` (1.1d 0x66bf8, 1.5b 0x6d57d, UO 0x79dd9) has no caller:
  VERIFIED, no relocation in 1.1d's `.rel.text` names it and no `call`
  reaches it in the 1.5b or UO disassembly. Nothing to port.
- The trigger-damage walks: `G_CheckHitTriggerDamage` and
  `G_GrenadeTouchTriggerDamage` walk the tree under 0x400000 over the
  segment's box (14.8).
- VERIFIED, `game.mp.i386.so` and `game.mp.uo.i386.so`: `setModel` links
  after `G_SetModel` and `G_DObjUpdate` (1.1d 0x5db16, UO 0x68ce4),
  `setContents` links after its store (1.1d 0x5dd4a, UO 0x6970b), and UO's
  `setBounds` writes `r.mins` (+0x108) and `r.maxs` (+0x114) and links
  (0x6976c). vcod links in all three (`builtins::entity::set_model`,
  `builtins::uo::set_contents`, `set_bounds`): a `script_model` spawned
  with no model is filed under its world box until its `setModel` files it
  under the model's bounds.
- VERIFIED, `Reached_ScriptMover` (0x5f98c): each group whose segment has
  ended is evaluated and linked (0x5fa17, 0x5faba), behind
  `G_MoverPush`'s link of the same frame. vcod relinks in place once per
  such group (`mover::reached`). INFERRED: the entity keeps its place, but
  each link runs its node's push-down, one level further.
  `ScriptEntCmd_MoveTo` and the other verbs link at their start
  (`mover::verb_linked`).
- `G_RunClient` (UO 0x41dfc) on a seated rider: `G_SetFixedLink` mode 2
  writes `r.currentOrigin` from the seat link, the tag of the vehicle as it
  stands shifted by the link's offset, then links him (coduo-vehicles.md
  13). vcod runs it on the rider's turn of the pass, his vehicle first
  (`run_rider_link` in `server.rs`), with the offset `VEH_LinkPlayer`
  asked for (`Seat::link_offset`).
- VERIFIED, `cod_lnxded` `SV_LinkEntity`: the unlink at 0x8090c84 runs
  when `CM_BoxLeafnums` (0x8053d94) returns 0. INFERRED: that walk ends in
  a leaf on every path down the BSP and appends each leaf it reaches
  (0x8054054), so it never returns 0 and the unlink never runs.

- VERIFIED, `TeleportPlayer` (1.1d 0x51380, 1.5b and UO alike): it reads
  `ent+0xf0` (0x513dc; 1.5b 0x50dc8, UO `ent+0xf4` 0x5a3f8), unlinks
  (0x513e6), writes the lifted `ps.origin` into `r.currentOrigin`
  (0x51457..0x51469) and links again when the read was non-zero (0x5146f,
  0x51477). INFERRED, off the 0xf0 and 0xf4 offsets
  (`sizeof(entityState_t)`, `r` first): the read is `r.linked`. INFERRED,
  off those branches: a turret's release, from `turret_think_client` in the gunner's end frame
  or from `G_FreeTurret`, files him at the mount spot, at the head of his
  node's list, ahead of his next cmd's link. vcod links there
  (`ScriptRuntime::teleport_link`). VERIFIED, `game.mp.i386.so`:
  `G_FreeTurret` (0x5297c) calls `TeleportPlayer` itself (0x52a0f), after
  the gunner's stance event (`G_AddEvent`, 0x529f1). So a `G_FreeTurret`
  release links in the freeing thread (`GameHost::free_entity`, since 2026-11-13; it used to
  wait for the gunner's end frame), so the rest of that frame's script and
  entity pass read him at the mount spot; the sim half (stance event,
  teleport events, the lock let go) still lands at his end frame, with no
  second link. INFERRED: no cmd of his runs between, as cmds run outside
  `G_RunFrame`. `ClientSpawn`'s release (0x426e1) is followed by its own unlink
  (0x42708) and the spawn's link, so vcod skips it there. 1.5b's and UO's
  extra block after the lift tests `eFlags | 0xc000` (1.5b 0x50e44),
  never zero, so it never runs. `turret_ab`'s
  `the_release_links_the_gunner_where_it_put_him` and
  `deleting_a_manned_turret_releases_its_gunner` read the link.
- VERIFIED, UO `VEH_UnlinkPlayer` (0x89fb2): `G_EntUnlink` (0x8a1ac) and
  `SetClientOrigin` (0x8a463), and neither `SetClientOrigin` (0x44210)
  nor anything else there calls `trap_LinkEntity`. INFERRED: a dismount
  leaves the rider filed where his seat link put him until his next cmd,
  as vcod does.

- A linked turret's `G_GeneralLink` comes from its own think
  (`turret_think` 0x5328c, `nextthink` +50 so every frame), on its turn of
  the entity pass, not from the gunner's end frame; vcod links it on the
  same turn (`link::run_linked`, then the think).

Not routed through the tree: the
relink `G_RunFrame` makes of every client when `g_bounds_width` or
`g_bounds_height_standing` changes (0x5082f, cheat cvars whose change vcod
ignores, cod11-gsc-object-model.md 16.2);
`func_*` entities, which vcod spawns none of. vcod raises the `"touch"` notifies at the
top of the next script frame in the order the walk made them and hands the
`"trigger"` queue out in retail's rounds (`ScriptRuntime::trigger_rounds`);
`crates/server/tests/touchorder_ab.rs` replays the probe and reads every
frame's lines in retail's order.

### 14.8 `trigger_damage`

Where it is placed. VERIFIED, the entity lumps of every BSP in the stock
paks: no 1.1 or 1.5 MP map places one; UO's `mp_uo_stanjel` places 17
(`targetname` `birdbush`, no spawnflags, no keys past `model`, read by
`maps/mp/_birdbush.gsc`'s `waittill("trigger")`) and `mp_foy` one
(`belltower_damage`, spawnflags 115, `script_exploder` 2, read by
`mp_foy.gsc`'s `belltower_damage()` and `_load.gsc`'s exploder thread).
`_treefall_gmi.gsc` waits on `script_model`s, not on a `trigger_damage`.

**The spawn.** VERIFIED, `SP_trigger_damage` (1.1d 0x65278, 1.5b 0x6ac1e,
UO 0x76f59), with the `.rodata` strings at 1.1d 0x79940..0x79aae:
`G_SpawnFloat("wait", "0.5")` to +0x268 (UO +0x278), `G_SpawnFloat("random",
"0")` to +0x26c; when `wait <= random` and `wait >= 0`, `random = wait - 100`
(`.rodata` 0x79ab8, 100.0) and `G_Printf("trigger_damage has random >=
wait")`; `G_SpawnInt("accumulate", "0")` to +0x250 (UO +0x260) and
`G_SpawnInt("threshold", "0")` to +0x2a8 (UO +0x2b8). Then `health` 32000,
`takedamage` 1, `use`/`pain`/`die` = `Use_`/`Pain_`/`Die_trigger_damage`,
the brush model, contents 0x405c0008, `svFlags |= 2`, and a link. UO alone
reads `teamname` (+0x1e8): `axis` stores 1 at +0x2dc, `allies` 2, else 0.

**`Respond_trigger_damage(self, mod)`** (1.1d 0x65024, 1.5b 0x6a7f5, UO
0x76aca). VERIFIED, the jump tables at 1.1d 0x799a8 and 0x79a00 read
through their relocations: a spawnflag refuses these means of death (1.1
and 1.5 share the table; UO's means of death are its own, `defs.rs`):

| flag | 1.1 / 1.5 refuses | UO refuses |
|---|---|---|
| 0x1 | 1 | 1 |
| 0x2 | 2 | 2 |
| 0x4 | 3..6 | 3..6, 13, 14 |
| 0x8 | 3..6, 9, 10, 13, 14, 15, 24 | 3..6, 9..14, 23 |
| 0x10 | 4, 6, 10, 14 | 4, 6, 10, 12, 14 |
| 0x20 | 7 | 7 |
| 0x40 | 18 | (no arm) |
| 0x100 | 0, 11, 12, 16, 17, 19..23 | 0, 15, 16, 19..22 |

**`Activate_trigger_damage(self, attacker, damage, mod)`** (1.1d 0x650c0,
1.5b 0x6a973, UO 0x76c2f). VERIFIED, the offsets and compares; INFERRED,
the order of the gates:

1. UO only: each side's team is its client's `+0x45bc`, or the trigger's
   own +0x2dc for one with no client; refused when both are non-zero and
   differ.
2. Refused while `nextthink` is non-zero unless the think is
   `Think_GeneralLink` (an `enableLinkTo`).
3. Refused when `threshold > 0` and `damage < threshold`.
4. Refused when `Respond_trigger_damage` refuses `mod`.
5. Refused when `accumulate != 0` and `32000 - health < accumulate`.
6. `activator = attacker`; unless `mod` is -1 (the `use` arm),
   `G_Trigger(self, attacker)` queues the `"trigger"` (UO 0x76218;
   1.1d inlines it: the queue at `level+0x1dec`, `cod11-gsc-language.md`).
7. Unless the think is `Think_GeneralLink`: `wait > 0` arms `multi_wait`
   (1.1d 0x65870, which only clears `nextthink`) at
   `level.time + (int)((crandom * random + wait) * 1000)`, the `fistp`
   under `| 0xc00` (0x6521f) truncating, `crandom` being
   `2 * rand() * 4.656613e-10 - 1` (`.rodata` 0x79a60); otherwise the touch
   function is cleared and `G_FreeEntity` thinks 100 ms on.
8. `health = 32000`.

`Use_trigger_damage` calls it with `accumulate + 1` and mod -1;
`Pain_trigger_damage` and `Die_trigger_damage` pass `G_Damage`'s damage and
mod, then set `health` back to 32000 when `accumulate` is 0. They run off
4.2's entity arm, after its `"damage"` (and `"death"`) notify and the
`health` decrement.

**The walks with no `G_Damage`.** VERIFIED, `G_CheckHitTriggerDamage`
(1.1d 0x653f8, 1.5b 0x6ad7f, UO 0x77130) and `G_GrenadeTouchTriggerDamage`
(1.1d 0x655a0): `trap_EntitiesInBox` over the two points' box under
0x400000, kept to `classname == "trigger_damage"` (and for the grenade
walk `flags & 0x8000`, which `enableGrenadeTouchDamage` sets,
12.2), `trap_SightTraceToEntity(start, 0, 0, end, ent, -1)`, then
`Scr_Notify(ent, "damage", 2)` with the attacker and the damage (no mod,
no point), `Activate_trigger_damage`, and `health` back to 32000 when
`accumulate` is 0. Nothing comes off `health`. Callers, VERIFIED:
`Weapon_Melee` (1.1d 0x78720 in the Ghidra numbering) with the trace's
start and end and `meleeDamage`, mod 7, ahead of the swing's damage;
`Bullet_Fire_Extended` on every leg with the leg's start and the trace's
end and the leg's damage, mod 1 or 2, ahead of the leg's `G_Damage`;
`G_RunMissile` for a `MOD_GRENADE` missile after its link (12.2), from the
origin before the move to `r.currentOrigin`, with the grenade itself as
the attacker and its `explosionInnerDamage` (1.1d `+0x23c`, 0x54306; UO
`+0x24c`, which UO's `fire_grenade` fills from the weapon def's
`+0x37c`) as the damage; UO's `G_MissileImpact` (0x5df57) for a missile
with damage (+0x248), from `r.currentOrigin` (+0x13c) to `trace + 4`,
with the missile's owner (or the world) as the attacker, its damage and
`methodOfDeath`. 1.1d's and 1.5b's `G_MissileImpact` make no such call.
VERIFIED, gdb breakpoints on retail UO's `G_MissileImpact` and
`G_CheckHitTriggerDamage` (`probe_dmgtouch` and
`probe_belltower` below): the missile trace's float at `+4` is its end
position, equal to `r.currentOrigin` at the impact, and the struck
face's normal sits at `+0x10` (`DirToByte(trace + 0x10)` reads it); every
shell's walk ran with start and end the same point, `dmg` 1000 and mod 5
(`MOD_PROJECTILE`) for a panzerschreck. So the shell's walk tests its
contact point alone: a `trigger_damage` fires only when the shell strikes
inside its brushes. Trap: `trace + 4` read as the normal walks from the
contact to near the world origin.

**Measured.** VERIFIED, `client-probes/probe_dmgtrig.gsc` on retail UO
1.51b, mp_uo_stanjel (`crates/server/tests/dmgtrig_ab.rs`):
every bush reads `health` 32000; a `radiusDamage` notifies `"damage"` with
the full charge, `MOD_EXPLOSIVE` and the world, and `health` reads 32000
on the builtin's next line; a charge over the health adds `"death"` ahead
of `"damage"`; a blast's direct and indirect falloff matches 14.1 against
the brush box. No `"trigger"` reached a script waiter in any phase.
VERIFIED, a gdb breakpoint on retail's `Activate_trigger_damage` and
`G_Trigger` in the same run: the first blast activated (mod 23,
`MOD_EXPLOSIVE`, `health` 31900 on entry) and called `G_Trigger`; blasts
inside the 500 ms wait reached `Activate_trigger_damage` with `nextthink`
2050 and were refused; each later phase activated once. INFERRED: the
queue entry is dropped because it is raised from a script thread
(`radiusDamage`) inside `G_RunFrame`'s trigger rounds, whose queue is
cleared once a round keeps nothing; vcod's `trigger_rounds` clears it the
same way.

VERIFIED, `client-probes/probe_dmgshot.gsc` on retail UO, mp_uo_stanjel, one `--probe-uo-weapon direct` client with a kar98k held 160
units off bush 86 (`dmgtrig_ab.rs`'s second test). A round through the
bush logs `"trigger"` (the player) then `"damage"` 120 with the player and
no mod: the queue's `"trigger"` and the cmd's `"damage"` wake in the next
frame's first round, the trigger's waiter first. A second round 2.1 s on
fires again; a round 40 degrees wide fires nothing; a swing from 90 units
reads `"damage"` 150 (the kar98k's `meleeDamage`) and fires. A frag's
blast reaches two bushes through `G_Damage`'s six-argument arm
(`MOD_GRENADE_SPLASH`), both `"trigger"`s ahead of both `"damage"`s in the
same frame. Two runs read the round and swing lines alike and the blast's
charges as 100/16 and 98/18: the landing rides the client's cmd timing.
The second run logged the frag at rest at (-2021, 3339, 3), 66 units off
bush 86's box, which 14.1's falloff gives as 98.

VERIFIED, `client-probes/probe_dmgtouch.gsc` on retail UO, mp_uo_stanjel, four runs (`dmgtrig_ab.rs`'s third test): a frag thrown
through bush 86 after `enableGrenadeTouchDamage` logs `"trigger"` with
the grenade as the activator, then `"damage"` 120 with the grenade and no
mod on each frame the flight spends inside the bush (4, 5, 4 and 4
frames); after `disableGrenadeTouchDamage` the flight raises nothing. A
panzerschreck shell into the ground inside the bush logs `"damage"` 1000
with the player and no mod; one 300 units the other way raises nothing
off its contact. The shells' and frags' blasts reach bushes through
`G_Damage` as 14.6 has it, their charges and victim order riding where
the missile lands.

VERIFIED, `client-probes/probe_belltower.gsc` on retail UO, mp_foy, four runs (`dmgtrig_ab.rs`'s fourth test). The belltower
(spawnflags 115, `*44`) takes a kar98k round's walk (`"damage"` 120, no
mod) and refuses it (mod 2, flag 0x2), as it refuses a shell's splash
(`MOD_PROJECTILE_SPLASH`, flag 0x10): a shell that strikes the tower's
roof outside the brush logs nothing. A shell from inside the belfry into
its floor fires it, and in two of four runs so did one fired from 600
units off whose spread put its contact inside. What follows, in order:
the shell's own splash hurts the shooter (78 to 84, `MOD_PROJECTILE_SPLASH`);
`mp_foy.gsc`'s `belltower_damage()` wakes first and its
`radiusDamage(origin, 256, 200, 190)` hurts him 198 with the world as
attacker (`MOD_EXPLOSIVE`); the probe's own `"trigger"` waiter runs after
it; its `"damage"` waiter never runs, because `_load.gsc`'s `exploder()`
thread has deleted the trigger by then; `maps/mp/_utility.gsc`'s
`exploder(2)` deletes the `script_brushmodel` `delete` (`*36`, the tower's
top) and keeps the `xmodel/fx` model, which plays `bell_tower_exp` after
its `script_delay` .04.

**A script-spawned one.** VERIFIED, `client-probes/probe_dmgspawn.gsc` on
retail 1.1d and 1.5b, mp_carentan, no client, both servers
logging the same lines (`dmgtrig_ab.rs`'s fifth test):
`spawn("trigger_damage", origin)` runs `SP_trigger_damage` with no `model`
key and no spawn keys, and the entity reads classname `trigger_damage` and
`health` 32000. INFERRED, by the movers doc 19 reading of the brush-model
setter (`cod_lnxded` 0x8089544, `CM_InlineModel(s.index)` with `s.index`
0): it links inline model 0, so its box is mp_carentan's world box,
(-8513, -6721, -637.12) to (10817, 11649, 2369) grown a unit (the BSP's
model 0, `bsp::Model::clip_bounds`), moved to the trigger's origin, and its
clip is a copy of the whole world standing there. The blasts, all
`radiusDamage(at, 64, 100, 50)` with the trigger at the first tdm spawn
point's origin `o` (844, 3480, -21.25), and `c` the box's centre less `o`,
(1152, 2464, 865.94):

| blast at | `"damage"` |
|---|---|
| `o + (0, 0, 32)` | none |
| `o + (0, 0, 2000)` | 100, no mod |
| `o + c` | 100 |
| `o + c - (0, 0, 40)` | 100 |
| 70 over the box's top, over the centre | none |
| `o + c`, the trigger moved to `o - (0, 0, 1500)` | none |
| the moved trigger's centre | 100 |
| `o + c`, the trigger back at `o` | 100 |
| `o + c` after `delete` | none |

INFERRED, 14.1 and 14.3 applied to that box: every blast but the one over
the top is inside the box, distance 0 and the full 100; `CanDamage`'s
no-client arm probes from the box's centre, (1996, 5944, 844.7) with the
trigger at `o`, and a blast at the centre has a probe of no length. The
blasts that miss inside the box have every probe to the centre blocked by
the world (vcod's clip blocks the same five). Moved 1500 down, the centre
sits under the world's bottom and the old centre is cut off from it.
INFERRED: so the first blast's miss is `CanDamage` reading the
world-sized box's centre, not anything about the spawn, and an `origin`
write relinks the trigger and moves its centre with it. VERIFIED, the same
logs: the `"damage"` notify carries two arguments on 1.1d and 1.5b (4.2),
hence no mod.

VERIFIED, `client-probes/probe_dmgspawnshot.gsc` on retail 1.1d (two runs)
and 1.5b, mp_carentan, one axis `--probe-uo-weapon direct`
client with a kar98k standing on the first tdm spawn point's ground and
firing at the ground 160 units east (`dmgtrig_ab.rs`'s sixth test). The
trigger's origin shifts the copy's ground against the world's, and so
whether the round's segment meets it: with the trigger at (0, 0, 20) or
(0, 0, 1) the round ends inside the copy's ground and logs `"trigger"`
(the player) then `"damage"` 120 with the player and no mod, as 14.8's
UO round did; at (0, 0, -20), (0, 0, -1) and (0, 0, 0) it logs nothing.
INFERRED: `G_CheckHitTriggerDamage`'s `trap_SightTraceToEntity(start, 0,
0, end, ent, -1)` clips the segment against the copy alone, and a round's
end, backed off the world's face, does not reach the same face in a copy
at the same place.

**As implemented.** `crate::game::damage_trigger`: `spec_of` (the spawn's
keys), `responds`, `activate`, `damaged` (`pain`/`die`, from
`GameHost::damage_nonclient`) and `sweep` (both walks; the sight trace is
the segment against the trigger's brushes, or its box without them).
`GameHost::blast_entities` lists every linked `trigger_damage` with its
brush model's box. `spawn_scripted` is a script `spawn`'s: model 0's box,
linked as a world copy (`CollisionWorld::set_world_copy`, contents
0x405c0008, which no trace mask but the walks' -1 meets), measured from
that box's centre with its own copy passed, and walked by tracing the
segment against the copy (`pusher_box_trace` with `Pusher::Copy`). A round's legs and a swing raise
`Effect::TriggerSweep` ahead of their hits; the missile pass sweeps a
grenade's move with its `explosionInnerDamage`; UO's shell impact
tests its contact point. A blast's notifies on an entity raised in the entity pass go through
`engine_notifies` (the pass's other notifies' path), so they wake with
the trigger round rather than in the next frame's packet pass.
`enableGrenadeTouchDamage` / `disableGrenadeTouchDamage` set the bit.

---

## 15. Where a shot goes: the aim block and the sight sway

A scoped shot on retail does not leave along the view. Section 2.1 already
read `FireWeapon` replacing the view's pitch and yaw with `client+0x220C` and
`client+0x2210`; this section is what writes those two, which is a block at
the top of `ClientThink_real` that runs once per usercmd ahead of `Pmove`
and turns the view into the direction the gun points, sight sway included.
The retail client draws its scoped view off the same functions (15.5), so
the reticle and the bullet agree on retail by construction, and a server
that fires along the raw view puts the bullet where the scope is not.

### 15.1 The block

VERIFIED, `ClientThink_real` `0x3fee0`, the addresses below; INFERRED, the
order and every gate, which are control flow.

- `0x3ff52`: `msec = ucmd->serverTime - ps->commandTime`, capped at 200
  (`0xc8`); this is the cmd length the sway is stepped by, not the
  66 ms chop `Pmove` runs.
- `0x40186`: `BG_GetSpeed(ps, level.time)`. VERIFIED, `0x3450c`: with
  `pm_flags & 0x10` (the ladder) it returns `ps->velocity[2]` when
  `level.time - ps->jumpTime (0x64) > 499` and 0 otherwise; off a ladder
  the horizontal `sqrt(vx² + vy²)`.
- `0x401a2`: `BG_CalculateViewAngles` on a frame of `{ps, client+0x226c,
  level.time, client+0x2274, client+0x2270, speed}`, and `0x401a7`-`0x401dd`
  add its three outputs onto `ps->viewangles` (`ps+0xc0`) into a local
  `aim`. The three client fields are section 6's damage kick: step 11's
  stamp and steps 7 and 8's two angles.
- `0x40229`: `BG_CalculateWeaponPosition_Sway(ps, &client+0x2278,
  &client+0x2284, &client+0x2290, 1.0f, msec)`; 15.3.
- `0x40332`: `BG_CalculateWeaponAngles` on a frame of `{ps, speed,
  msec * 0.001 (.rodata 0x72d2c), client+0x229c..0x22a8, level.time,
  client+0x226c, +0x2274, +0x2270, client+0x22ac..0x22c0,
  client+0x2290..0x2298}`, written back into the same client fields at
  `0x403d2`-`0x4043e`; 15.2.
- `0x40347`: `BG_IsAimDownSightWeapon(ps->weapon)`, which is
  `weaponDefs[w]->aimDownSight` (`0x2cc`) and nothing else (`0x3a970`, 25
  bytes). With it set and `ps->fWeaponPosFrac` (`0xb8`) not 0 (`0x4035b`):
  `AnglesToAxis(weaponAngles)` (`0x4037e`), `AnglesToAxis(aim)`
  (`0x40394`), `MatrixMultiply(weaponAxis, viewAxis)` (`0x403b4`),
  `AxisToAngles` back into `aim` (`0x403c4`).
- `0x4044a` / `0x40456`: `client+0x220c = aim[0]`, `client+0x2210 = aim[1]`.
- `0x40466`: `Pmove`.

The `level.time` the block reads is the one a cmd runs on, the last
frame's: cmds run ahead of `G_RunFrame` (player-clip doc 8.12). VERIFIED,
vcod measurement (`leanhit_ab`'s first `-cmds` captures, 1.1d, 1.5b and UO, each
round's direction logged by the damage callback): fed the frame's own
time, ours' scoped rounds left 0.01 to 0.02 degrees off retail's in pitch
and yaw. Read as a shift of the idle's sines, the pitch residual of 12
of the 1.1d capture's 13 rounds, and the yaw residual of every round whose
yaw moved more than 0.005 degrees, put retail's time 48.5 to 51.8 ms
behind ours; the odd one is the shooter's left lean against the target's
right (`s_left_t_right`). On the last frame's time every 1.1d round but
that one leaves within 2e-5 of retail's direction.

VERIFIED: `MatrixMultiply` (`0x3b5a8`) is the row product `out[i][j] =
Σ in1[i][k] * in2[k][j]`, `AnglesToAxis` (`0x3ef3c`) is `AngleVectors` with
the right vector negated (`vec3_origin - right`), and `AxisToAngles`
(`0x3c808`) takes the yaw as `atan2(f[1], f[0])` and the pitch as
`atan2(f[2], sqrt(f[0]² + f[1]²)) * -180 / PI` (`.rodata 0x72960` is
-180.0), each with 360 added while negative. INFERRED: the composed forward
is therefore the gun's forward expressed in the view's basis, `wf.x *
viewForward + wf.y * viewLeft + wf.z * viewUp`, which is the gun's angles
applied as a rotation in the view's own frame, and the stored pitch is in
0..360 (a shot 0.4 degrees up reads 359.6).

VERIFIED: the two fields are read by `FireWeapon` (`0x68dcc`, `0x68dd5`) and
`FireWeaponMelee` (`0x69424`, `0x6942d`) and nowhere else in the module, and
11.3 already has the grenade throw going through `FireWeapon`'s same angles.
INFERRED: a bullet, a swing and a throw all leave along the block's aim; a
grenade is not `aimDownSight`, so a throw gets the view plus its damage kick
and no gun angles. INFERRED, from the position of the block ahead of
`Pmove`: the aim is built from the playerstate the previous cmd left,
`viewangles` included, so a shot goes along the view of the cmd before the
one that pulled the trigger.

**The cmd that fires.** `client-probes/probe_leanhit`'s hold calls
`setPlayerAngles` every frame, so the view a round reads is one of two:

- VERIFIED, vcod measurement (the six `leanhit` captures with `probe_fall`,
  1.1d, 1.5b and UO; each `PROBE dir` against 15.2's idle sway
  on the last frame's `level.time` composed with the hold's script angle):
  every round fits to 5e-7 either the exact script angle or that angle cut
  to 65536ths, and none fits anything between. A 1.1d run also carried a
  constant 3 65536ths of pitch on every round, the turn sway's lag the
  scope froze (15.3); a 1.5b run carried 4 on its first round only.
- INFERRED, from those fits and `SetClientViewAngle` (0x41e30): the hold
  writes the exact angles into `viewangles` and `delta_angles =
  ANGLE2SHORT(angle) - lastcmd`, truncated. A round reads the exact angles
  when its cmd is the first to run after the frame's threads, and the
  quantized `SHORT2ANGLE(cmd + delta)` of a cmd that ran in between when
  not. A cmd built on the snapshot before the frame (its `delta_angles`
  a frame stale) lands a short low when the angle's fraction is past a
  half: retail's quantized rounds read the angle floored, never rounded.
- VERIFIED, the `-press` captures (same recipe, the net
  probe's `CMDS` lines now naming each input change, `in=`, and the
  snapshot each cmd went out on, `snap=`): the probe stamps a cmd with a
  snapshot's time the moment it arrives, so the trigger cmd's stamp is
  the frame's time; cmds stamped 2 ms before the frame to 3 after it went
  out on the snapshot before. In the first run, where the probe tapped in
  the frame that ran its 81-ray grid, a round read the quantized view
  even where the shooter's cmd before the trigger was stamped 18 ms
  ahead of the frame: a long frame takes in the cmds sent while it runs.
  `probe_tapwait 1` taps a frame later; in that run every round on all
  three servers reads the exact angles.
- vcod: `leanhit_ab`'s `-press` gates replay each cmd's logged input on the
  frame its snapshot's `ct=` names (`ProbeHold::ran`, 3.6), both clients on
  retail's frames: every round of the 1.1d, 1.5b and UO runs leaves within
  2e-5 of retail's direction and every fall row matches. The `-cmds`
  captures are reruns with `probe_tapwait 1` (an earlier take tapped in the
  grid's frame and logged no input by cmd).
- The 1.5b first round's lag (vcod measurement, the
  `-press-1.5` replay with the shooter's sway state logged per cmd). The
  shooter holds the sight from his first cmd (12450, `in=10:16`), but
  `PM_UpdateAimDownSightFlag` (1.5b 0x2cf6c) clears it unless `pml+0x34`
  is set or `pm_type` is 1 (VERIFIED disassembly), so the scope's freeze
  waits on the hold's spawn drop. The turn onto the site at 12466 leaves
  3 65536ths of pitch lag, 12482 2, the re-aim after the 12500 re-pin 5,
  12516 4, 12532 3; the freeze takes the lag of the cmd the sight starts
  on. The float steps are excluded: each raw lag sits 0.19 to 0.5 of a
  65536th from the cut (3.50, 2.81, 5.79, 4.68, 3.74), and porting 1.5b's
  stored sway step (helper 0x31fe6, UO 0x39236: distance and `lerp *
  distance * (msec * 0.001)` each stored as a float, VERIFIED disassembly)
  and the unrounded `AngleNormalize360` product (1.1d 0x3eb30, 1.5b
  0x36a3c keep it on the x87 stack before the truncating `fistp`,
  VERIFIED disassembly) moved no round; vcod keeps its float sway step and
  float product. `pml+0x34` is 1.5's `almostGroundPlane`, set off the
  ground when a floor lies within a unit below (`cod15-movement.md` 5,
  "The sight off the ground", VERIFIED decompile). The shooter is 0.63 over
  the floor at the 12516 cmd's ground trace, so 1.5b sights there and
  freezes 4 (read as the ground itself, the sight starts on 12532, the
  landing, and freezes 3). VERIFIED, replay: with the test ported every
  round of the 1.5b run leaves within 2e-5 of retail's direction.

### 15.2 `BG_CalculateWeaponAngles` (`0x3a1e4`)

VERIFIED, the offsets, constants and call targets below, out of the
function and the five static helpers it calls in this order; INFERRED, every
condition. The output starts at zero and each term adds.

- Lean roll: with `ps->leanf` (`0x40`) non-zero, `out[2] -= 2 *
  GetLeanFraction(leanf)`, where `GetLeanFraction` (`0x7ba64`) is
  `(2 - |f|) * f` (`.rodata 0x7a340` is 2.0).
- Sight pitch: with `aimDownSight` set, `out[0] += fWeaponPosFrac *
  adsAimPitch` (`0x344`). Every stock MP file reads `adsAimPitch 0`.
- `0x39604`, the tilt under movement. The stance's minimum speed is
  `proneRotMinSpeed` (`0x180`) under `eFlags 0x40`, `duckedRotMinSpeed`
  (`0x17c`) under `0x20`, `standRotMinSpeed` (`0x178`) otherwise. At or
  below it, or with `weaponstate` 5 (`WEAPON_RELOADING`), the target tilt is
  zero; above it the target is the stance's `standRotP/Y/R` (`0x108`),
  `duckedRot*` (`0x12c`) or `proneRot*` (`0x150`) scaled by `(speed - min) /
  (ps->speed (0x44) - min)` clamped to 0..1, and then by `1 - fWeaponPosFrac`
  when the fraction is non-zero. Each axis of the state at `client+0x229c`
  eases toward its target by `dt * (target - cur) * rate`, `rate` being
  `posProneRotRate` (`0x174`) when `viewHeightCurrent` (`0xd0`) equals
  `proneViewHeight` (`0x33c`) and `posRotRate` (`0x170`) otherwise, with the
  step floored at `dt * 0.1` toward the target (`.rodata 0x727fc`,
  `0x72800`) and clamped at it. The state is added whole at a fraction of 0,
  scaled by `1 - 2 * frac` below 0.5 (`0x72804`), and not at all above.
- `0x3990c`, the idle. The amount is `hipIdleAmount` (`0x274`), or 80
  (`0x7280c`) when the file spells 0, on a weapon with no sight, and
  `hipIdleAmount + (adsIdleAmount (0x270) - hipIdleAmount) * frac` on one
  with; a stance factor at `client+0x22a8` eases toward 1, `idleCrouchFactor`
  (`0x278`) under `eFlags 0x20` or `idleProneFactor` (`0x27c`) under `0x40`
  at 0.5 a second (`0x72808`), and scales the amount. Then, with `t` the
  frame's `level.time` in milliseconds: `out[2] += amount * sin(t * 0.0005)
  * 0.04` (`0x72810`, `0x72814`), `out[1] += amount * sin(t * 0.0007) *
  0.01` (`0x72818`, `0x7281c`), `out[0] += 0.01 * sin(t * 0.001) * amount`
  (`0x72820`). The sines are taken in x87 extended precision off an
  integer time; an f32 argument at a few million milliseconds has no
  fractional part left to carry the phase.
- `0x39a5c`, the walk bob on the gun. `c = PI/4 + 2 * PI * bobCycle / 255 +
  4 * PI` (`0x72824` is 255.0, `0x72828` PI, `0x72830` 2 PI, `0x72838` PI/4;
  `bobCycle` is the byte at `ps+8`), `sp = speed * 0.16` (`0x72840`), and
  the stance multiplier is 0.03 (`0x72848`) with `viewHeightTarget` (`0xcc`)
  equal to `proneViewHeight` (`0x33c`), 0.0075 (`0x7284c`) with
  `crouchViewHeight` (`0x340`), 0.007 (`0x72850`) otherwise, the product
  capped at 10 (`0x72844`). `pitch = -((sin(PI/2 + 4c) * 0.2 + sin(2c)) *
  0.75 * a)` (`0x72858`, `0x72854`, `0x72860`, `0x72864`), `yaw = -sin(c) *
  a`, and `roll = min(sin(c - 3PI/20) * a', 0)` with `a'` off `sp * 1.5`
  (`0x72870`, `0x72868`). With the fraction non-zero all three are scaled by
  `1 - (1 - adsBobFactor (0x234)) * frac`, which is what that key is for.
- `0x39ce8`, the damage kick on the gun, section 6's `client+0x226c` stamp
  and its two angles. Nothing when the stamp is 0. `f = frac * 0.5 + 0.5`
  (`0x72874`), the envelope rises over `f * 100` ms (`0x72878`) as
  `GetLeanFraction(dt / rise)` and falls over `f * 400` more (`0x7287c`) as
  `1 - GetLeanFraction(1 - r)` with `r = 1 - (dt - rise) / fall`, gone once
  `r <= 0`; with the fraction non-zero and `adsOverlayReticle` (`0x228`)
  set, `f *= 1 - frac * 0.75` (`0x72880`). Then `out[0] += g * f *
  client+0x2274 * 0.5`, `out[1] -= g * f * client+0x2270`, `out[2] += g * f
  * client+0x2270 * 0.5`.
- `0x39e14`, the gun-kick spring: `adsGunKickAccel/SpeedMax/SpeedDecay/
  StaticDecay` (`0x360`-`0x36c`) blended from the `hipGunKick*` four
  (`0x3a0`-`0x3ac`) by the fraction, `gunMaxPitch`/`gunMaxYaw` (`0x280`,
  `0x284`) as the clamps, stepped in 5 ms substeps (`0x72888`, `0x72890`),
  a pitch and a yaw spring each settling at 0.25 (`0x72898`) and a velocity
  under 1, off the state at `client+0x22ac..0x22c0`. VERIFIED, by
  `objdump` over the module: the only instructions that touch
  `client+0x22ac` through `+0x22c0` are `ClientThink_real`'s own copies in
  and out of the frame. INFERRED: nothing in the game module ever kicks the
  spring, so on a server it reads zero for the whole of a life and the
  helper adds nothing; the kick a retail client sees after its own shot is
  the cgame's copy of the same state and never reaches the server's aim
  (15.7). VERIFIED, a capture: a 1.1d sighted burst with the kick ranges
  pinned strikes one spot. 1.5b's and UO's servers do kick it
  (`cod15-movement.md` 6, "The server's gun kick").
- Last, `out[0] = AngleSubtract(out[0], frame+0x44)` and `out[1] =
  AngleSubtract(out[1], frame+0x48)`, the sway angles of 15.3.

### 15.3 `BG_CalculateWeaponPosition_Sway` (`0x3a548`)

VERIFIED: the signature is `(ps, vec3 *lastView, vec3 *offset, vec3
*angles, float scale, int msec)` off the call at `0x40229`, the field
reads below, and the constants; INFERRED: the conditions.

- With `aimDownSight` clear the six hip keys are used as they are:
  `swayMaxAngle` (`0x288`), `swayLerpSpeed` (`0x28c`), `swayPitchScale`
  (`0x290`), `swayYawScale` (`0x294`), `swayHorizScale` (`0x298`),
  `swayVertScale` (`0x29c`). With it set, each is blended toward its
  `adsSway*` twin (`0x2a4`-`0x2b8`) by the fraction, but first: with the
  fraction above 0 and `adsOverlayReticle` (`0x228`) non-zero the function
  returns at `0x3a596` having written nothing, the last view included. A
  scope freezes the turn sway for as long as the sight is up at all, and
  `swayShellShockScale` (`0x2a0`) has no ADS twin and is not read here.
- `AnglesSubtract(ps->viewangles, lastView)` (`0x3a687`) gives the per-axis
  turn since the previous cmd; pitch and yaw are each clamped to
  `±swayMaxAngle`.
- `k = swayLerpSpeed * msec * 0.001` (`0x728e8`). Each of the four state
  values moves `k` of the way to its target when the distance is over 0.001
  (`0x728f0`) and the step does not overshoot it, and to the target outright
  otherwise: `offset[1]` toward `yawTurn * swayHorizScale * scale`,
  `offset[2]` toward `pitchTurn * swayVertScale * scale`, `angles[0]` toward
  `pitchTurn * swayPitchScale * scale`, `angles[1]` toward `yawTurn *
  swayYawScale * scale`, the two angle targets first pulled down by 360
  while more than 180 (`0x728f8`, `0x728fc`) above the current value. Both
  angles then go through `AngleNormalize180` (`0x3a8e5`, `0x3a8f8`) and
  `lastView = ps->viewangles` (`0x3a900`-`0x3a91a`).
- VERIFIED, `AngleNormalize180` `0x3eb70`: it sets the x87 control word
  with `0xc00` (`0x3eb86`) before its `fistp`, which is round toward zero,
  masks with `0xffff` and scales back by 360/65536. INFERRED: the
  truncation is what lets a decaying lag reach zero; rounded to nearest it
  stalls a few 65536ths out once a step is smaller than half of one, and
  15.6 measured exactly that on the first implementation.
- `scale` is 1.0 on the server (`0x4020e` pushes `0x3f800000`). VERIFIED,
  cgame `0x300361f0`: the client passes `1 + (swayShellShockScale - 1) *
  ease` through a shellshock and 1.0 otherwise, so the shellshock key is a
  client-side thing.

**The spawn's view.** VERIFIED, 1.1d `ClientSpawn` (`0x4268c`): the
`__bzero` of the whole `0x22c4`-byte client at `0x42804` clears the sway
state at `client+0x2278` and the playerstate's weapon with it,
`SetClientViewAngle` (`0x429e2`) writes the spawn's angles into
`ps.viewangles`, and `ClientThink_real` (`0x42a82`) runs on the zeroed
cmd. VERIFIED: the block has no weapon or `pm_type` gate (`0x3ffc4`:
only `sessionState` 3 and 2 branch off it), and the sway reads
`weaponDefs[ps.weapon]` straight off the table (`0x3a557`-`0x3a568`).
INFERRED: the think's block runs on weapon 0's def, zero but its name
(`0x35b55`), so its sway moves nothing and stores the spawn's angles as
its last view; the loadout's first cmd then turns off them. A spawn
point at yaw 180 is stored as 180, so the turn onto 0 reads
`AngleSubtract(0, 180)` -180 (`0x3e968`: `while > 180` and `while <
-180`), and the lag starts negative; vcod's quantized view reads -180
there, which would turn it the other way. VERIFIED, vcod measurement
(`leanhit_ab`'s 1.1d `-cmds` capture): the lag that decay leaves when the
scope freezes it is the -1 65536th all 13 of its rounds carried (3.6).
INFERRED: 1.5b's and UO's `ClientSpawn` make the same three calls (1.5b
`0x3ceac`, `0x3cf1e`, `0x3cf35`; UO `0x45342`, `0x453f2`, `0x45409`) and
their sway reads `BG_GetInfoForWeapon(ps.weapon)` (1.5b `0x32083`, UO
`0x392d3`). VERIFIED, vcod measurement: with the spawn's block run on
all three, every 1.5b and UO `leanhit_ab` case still passes.

### 15.4 `BG_CalculateViewAngles` (`0x3a930`)

VERIFIED: the offsets and constants; INFERRED: the conditions. Two helpers
on a zeroed output.

- `0x3a2d4`, the damage kick on the view. Nothing when `client+0x226c` is 0.
  `f = 1 - frac * 0.5` (`0x728a0`), and with the fraction above 0 and
  `adsOverlayReticle` set `f *= frac * 0.5 + 1`; the envelope is 15.2's
  with a fixed 100 ms rise (`0x728a4`) and 400 ms fall (`0x728a8`); `out[0]
  += g * f * client+0x2274`, `out[2] += g * f * client+0x2270`.
- `0x3a3c4`, the sight's own walk bob: with `eFlags & 0xc000` clear, the
  fraction non-zero and `adsViewBobMult` (`0x238`) non-zero, `c = 2 * PI *
  bobCycle / 255 + 2 * PI` (`0x728ac`, `0x728b0`, `0x728b8`), the stance
  multiplier of 15.2 by `viewHeightTarget` (`0x728c4`, `0x728c8`,
  `0x728cc`) on the raw speed capped at 45 (`0x728c0`), then `out[0] -= frac
  * adsViewBobMult * ((sin(PI/2 + 4c) * 0.2 + sin(2c)) * 0.75 * a)`
  (`0x728d0`, `0x728d8`, `0x728e0`, `0x728e4`) and `out[1] -= frac *
  adsViewBobMult * sin(c) * a`.

### 15.5 The client draws the same thing

VERIFIED, `cgame_mp_x86.dll` `0x300371f0`: it calls `0x300361f0` (the sway
with the shellshock scale), `0x30012bf0` (`BG_CalculateWeaponAngles`, the
same five helpers at `0x300120c0`, `0x30012500`, `0x30012610`,
`0x300127e0` and `0x30012a60`), and with `aimDownSight` set and the
fraction non-zero the same `MatrixMultiply` and `AxisToAngles` pair into
`0x3020cb80`/`0x3020cb84`, and the raw pair otherwise. INFERRED: those two
are the refdef's pitch and yaw, so what the scope shows is the world rotated
by the same gun angles the server fires along, with the overlay fixed at the
centre.

### 15.6 Measured: the scoped kar98k on retail, and on ours

The `--probe-sway` run: join axis on `mp_carentan` under `tdm` with
`kar98k_sniper_mp`, turn to the longest clear sightline from the spawn, raise
the sight, tap eight shots standing still, and print every bullet-impact
temp entity's origin beside the eye and view. The file spells `adsSpread 0`,
`adsAimPitch 0`, `adsIdleAmount 45`, `adsOverlayReticle FG42`, so a scoped
shot has no cone and the angle between the raw view and the eye-to-impact
ray is the sway alone: pitch `0.45 * sin(0.001 t)` and yaw `0.45 * sin(0.0007
t)` off 15.2's idle, with the stance factor at 1 and every other term zero
standing still.

VERIFIED, retail, eight impacts 3700 to 4300 units out at yaw
-141.998, muzzle `(976, -792, 56)`, `t` the snapshot's `serverTime`:

| `serverTime` | pitch, measured | model | yaw, measured | model |
|---|---|---|---|---|
| 212050 | -0.438 | -0.450 | -0.317 | -0.317 |
| 215100 | +0.448 | +0.448 | -0.128 | -0.101 |
| 218100 | -0.422 | -0.437 | +0.422 | +0.430 |
| 221150 | +0.428 | +0.425 | -0.347 | -0.343 |
| 224150 | -0.379 | -0.400 | -0.108 | -0.078 |
| 230050 | -0.274 | -0.295 | -0.327 | -0.327 |
| 233100 | +0.254 | +0.262 | -0.122 | -0.086 |
| 236100 | -0.184 | -0.208 | +0.423 | +0.425 |

A retail impact origin is whole units, which at 4000 units is 0.014 degrees,
and every residual is inside two of those. INFERRED: the shot leaves along
the composed aim, both sines with the amplitudes and rates of 15.2, no
constant offset, and the sight bit alone (the sway is frozen by the scope,
the spring is zero) puts nothing else on it. INFERRED: with `t` taken 50 ms
earlier the largest residual falls from 0.036 to 0.020 degrees, which is
the `level.time` a cmd is processed under being the previous frame's, but
the quantization does not separate the two readings.

VERIFIED, ours, the same run against `vcod-server` (impacts 1940 units out,
origins not snapped), first build: pitch within 0.002 degrees of the model
at `t = serverTime` on every shot and yaw a constant 0.022 degrees over it,
which is four 65536ths, the stall 15.3 describes: the lag the turn to the
sightline left was frozen by the scope four units short of zero because the
normalize rounded. With the truncation the same eight shots read within
0.004 degrees of the model on both axes, the largest residual being 0.004
on a yaw of -0.212 against -0.208.

**As implemented.** `crates/common/src/pmove/aim.rs` is the block: the
sway, the weapon angles with the four helpers that can be non-zero on a
server, the view angles, and the composition; `ClientSim::update_aim`
(`crates/server/src/spectate.rs`) runs it once per cmd on the whole cmd
length before the 66 ms chop, off the view the previous cmd left, and keeps
the state and the damage kick (`end_frame` now computes section 6's step 6
angles as well as the bytes); `Attack::Shot`, `Swing` and `Throw` carry the
aim, and `bullet_fire`, `melee_fire` and `throw_velocity` fire along it.
Left out on purpose: the gun-kick spring on 1.1 (zero on its server; 1.5
and UO kick it, `cod15-movement.md` 6), `eFlags 0xc000`
(no mounted MG), and the shellshock scale (client-side).

### 15.7 The fire recoil: the view kick and the gun kick

A shot by the client's own body kicks two things, both in the cgame and
neither in the game module: the view, which the engine adds to the usercmd
angles, and the gun spring of 15.2, which only moves the drawn gun and the
scope. Neither reads the stance.

**The keys.** VERIFIED, the field table at `0x7C9A0` in `game.mp.i386.so`
(section 0): `adsGunKickPitchMin/Max` `0x350`/`0x354`, `adsGunKickYawMin/Max`
`0x358`/`0x35c`, `adsViewKickPitchMin/Max` `0x370`/`0x374`,
`adsViewKickYawMin/Max` `0x378`/`0x37c`, `adsViewKickCenterSpeed` `0x380`,
`hipGunKickPitchMin/Max` `0x390`/`0x394`, `hipGunKickYawMin/Max`
`0x398`/`0x39c`, `hipViewKickPitchMin/Max` `0x3b0`/`0x3b4`,
`hipViewKickYawMin/Max` `0x3b8`/`0x3bc`, `hipViewKickCenterSpeed` `0x3c0`,
all type 6 (float). The spring's own keys are 15.2's.

**`CG_WeaponFireRecoil` (vcod's name, cgame `0x30038850`).** VERIFIED, the
reads and constants: the weapon def is `0x30209484`, which `0x300299d6`
loads as `weaponDefs[0x3020720c]`, and the fraction is `0x30207214`; the
predicted playerstate sits at `0x3020715c` (pushed at `0x30033dff`), so
the two are its `weapon` (`+0xb0`) and `fWeaponPosFrac` (`+0xb8`).
INFERRED, the conditions:

1. The view kick picks the `ads` pair when the fraction equals 1.0 exactly
   (`0x30038864`), the `hip` pair otherwise. `pitch = Min + random() *
   (Max - Min)` off `ViewKickPitch`, then `yaw` the same off
   `ViewKickYaw`; `random()` is MSVC's `rand` (`0x3004b189`, the
   `0x343fd`/`0x269ec3` LCG on the thread's seed, top 15 bits) times
   `1/32768` (`0x300693b4`). VERIFIED, the stores at `0x30038935`-`0x30038940`:
   `kickAVel = { -pitch, yaw, yaw * -0.5 }` (`0x300693b0` is -0.5),
   **assigned**, so a second shot replaces the speed and does not add.
2. The gun kick picks the `ads` pair when the fraction is above 0
   (`0x30038946`), the `hip` pair otherwise, and draws a pitch then a yaw
   the same way off `GunKickPitch` and `GunKickYaw`; both are **added** to
   the spring's speeds (`0x3020cba0`, `0x3020cba4`, at `0x30038a05`).

The draw order is view pitch, view yaw, gun pitch, gun yaw.

**Who calls it.** VERIFIED, `CG_FireWeapon` (`0x30038b70`, the sound doc's
section 8) calls it at `0x30038bd2` when `snap->ps.pm_flags & 0x50000` is
set (`0x30038bbf`) and the event's entity number equals
`snap->ps.clientNum` (`0x30038bca`). INFERRED: so only a shot by the body the
view rides kicks, own (0x40000) or followed (0x10000); a mounted MG's
`EV_FIRE_WEAPON_MG42` rides the turret's entity (`cod11-turrets.md`) and
never kicks. The quad-barrel events run `CG_FireWeapon` twice (the sound
doc's event table), so they kick twice. VERIFIED, `CG_EntityPreEvent`'s
`0xa8` arm (`0x3001e936`) shakes and falls into the fire-weapon arm's
`CG_FireWeapon` call. VERIFIED, UO cgame: `CG_FireWeapon` (`0x30047d20`)
calls its kick (`0x30016120`) under the same test, `snap->ps.pm_flags &
0xc0000` and the state's `number` (`[esi]`) equal to `clientNum`
(`snap+0xe0`), and the pre-event's MG arm (`0xad`, `0x300237dc`) shakes
(`0x3001b420`, 0.05, 100, 100.0) and calls it with the turret's state
(`esi`, the call at `0x30023800`). VERIFIED, `snapshots-uo-turret.bin`:
all 148 of the followed gunner's rounds ride the turret's ring, none the
playerstate's. INFERRED: on both, neither the gunner's own view nor a
follower's kicks for a mounted MG; only the shake moves it.
(`play::replay::a_followed_mounted_gunner_is_not_kicked`.)

**`CG_KickAngles` (vcod's name, cgame `0x300328b0`).** VERIFIED, the
constants: `kickAngles` is `0x3020cb38`, `kickAVel` `0x3020cb2c`, both three
floats; the frame (`0x30207144`, integer ms) is cut into slices of at most
5 ms, each `ft = slice * 0.001` (`0x300693c0`); the centring speed is
`adsViewKickCenterSpeed` when the fraction is above 0.5 (`0x3006930c`),
`hipViewKickCenterSpeed` otherwise, and 2400 (`0x300693ec`) when `weapon`
is 0; the return factor is 0.06 (`0x300693e8`); the clamp is 10 degrees
(`0x30069350`, a double; `0x300693e4`/`0x300693e0` are ±10). INFERRED, per
axis and slice:

1. Nothing when both the angle and its speed are 0.
2. With the angle non-zero, `speed += -sign(angle) * center * ft`.
3. `change = ft * speed`, times 0.06 when it points back toward the centre
   (`change * angle < 0`).
4. When `angle + change` would cross the centre, both go to 0. Otherwise
   the angle takes it; at exactly 0 the speed is zeroed, and past ±10 the
   angle sits at the clamp with the speed zeroed.

INFERRED: this is RTCW's `CG_KickAngles` with the step cut from 20 ms to 5 and the
recoil-pitch half removed.

**Where it runs.** VERIFIED, `CG_DrawActiveFrame` (vcod's name): at `0x30033d4f`
it calls `CG_KickAngles` when `snap->ps.pm_flags & 0x40000` is set, and
otherwise zeroes both vectors (`0x30033d5f`-`0x30033d91`); at `0x30033ec1` it
hands `kickAngles` to the engine through syscall `0x56`. VERIFIED, CoDMP.exe:
the cgame syscall switch copies the three floats to `0x0143a994` (case
`0x56`), and `CL_FinishMove` (vcod's name, `0x0040b690`) writes each cmd angle as
`ftol((cl.viewangles[i] + kick[i]) * 182.044)` (`0x00568f68`, 65536/360;
`0x0040b6b7`-`0x0040b711`) masked to 16 bits, `_ftol2` (`0x00538be0`)
truncating. INFERRED: **the view kick is in the usercmd angles**, so the
server's `PM_UpdateViewAngles`, its aim block and every later shot see it,
the client's prediction draws it, and it is never added to `cl.viewangles`
itself: as it centres, the view comes back to where the mouse left it.
The roll third rides `cmd.angles[2]` the same way.

**How the kick reaches the screen.** VERIFIED, the only references to
`kickAngles` (`0x3020cb38`) are the clears at `0x30028af8` and
`0x30033d7d`-`0x30033d91`, `CG_KickAngles`' own `0x300328d7` and the push
at `0x30033ec1`: the cgame never adds it to the drawn view. VERIFIED,
`CG_CalcViewValues` (vcod's name, `0x300333b0`) copies the predicted
playerstate's `viewangles` (`0x3020721c`-`0x30207224`, `ps+0xc0`) into
`refdefViewAngles` (`0x302095cc`-`0x302095d4`) at `0x300334fa`-`0x30033527`,
the roll included, and the intermission arm does the same at
`0x30033413`-`0x3003343b`; `CG_OffsetFirstPersonView` (vcod's name,
`0x30032ae0`) then adds the three components of the offset `0x30012cb0`
returns (`0x30032b6a`-`0x30032b9c`). INFERRED: the kick's roll reaches the
screen through prediction, as `PM_UpdateViewAngles` writes
`viewangles[2]` from `cmd.angles[2] + delta_angles[2]` like the other two
axes (`docs/protocol-1.1.md`, "View angles").

**The frame a shot kicks.** VERIFIED, the `CG_FireWeapon` call sites:
`CG_EntityEvent` (`0x3001dc10`) reaches it at `0x3001e2c8`-`0x3001e31c`
only behind a test of its stack argument (`0x3001e2b7`, `[esp+0x24]`), and
that argument is 1 only from `0x30028be0` (`push 0x1` at `0x30028c2d` and
`0x30028cb8`, entity `0x3020922c`, the predicted entity) and 0 from the
entity pass's `0x3001ea3d` and `0x3001eadc`. `CG_EntityPreEvent`
(`0x3001e820`) reaches it for events 159 to 161, 168, 169 and 170 off its
table at `0x3001e9e8`/`0x3001e9c0` (`0x3001e8ff`-`0x3001e955`), and runs
only from `0x3001eb10`, the snapshot's entities (17.2). VERIFIED,
`0x30028be0` (vcod's name `CG_CheckPlayerstateEvents`) runs from `0x30028d3a`,
inside `0x30028d00`, which `0x300294f0` calls at `0x30029a3f`, which
`0x30029a70` calls first (`0x30029a71`); `CG_DrawActiveFrame` calls
`0x30029a70` at `0x30033cc8`. Its other caller, `0x3002fc6c`, sits in the
snapshot transition after the damage feedback (`0x300287f0`) and
`0x30028740`. INFERRED: `0x30029a70` is `CG_PredictPlayerState` and
`0x3002fc6c` the follow arm of the transition, both ahead of
`CG_KickAngles` (`0x30033d58`); so a shot by the drawn body sets the kick's
speed and the same frame's `CG_KickAngles` steps it, and the kick that
frame hands syscall `0x56` (`0x30033ec1`) rides the next frame's cmds.
vcod handled the fire in the event pass after the view, which stepped the
kick a frame late; the fire now runs in the event pass ahead of
`OnlineView::step_recoil` (`crates/client/src/main.rs`).

**The reset.** VERIFIED, `0x30028a70` zeroes both vectors and the gun
spring (`0x3020cb94`-`0x3020cba8`) and stamps the weapon name. Its callers:
the first-snapshot setup at `0x3003037f`, and `CG_TransitionSnapshot` (vcod's
name) at `0x300300cd`. INFERRED, the transition's branches
(`0x3002ffda`-`0x300300be`): the call is taken when `0x30207154` is set or
the new snapshot's `stats[5]` (snapshot `+0x114`, the spawn count) or
`clientNum` (`+0xb8`) differs from the old one's. Under `pm_flags & 0x50000`
an `eFlags` 0x8 flip with the same pair takes the other arm
(`0x30030033`-`0x30030082`), which re-copies the playerstate and zeroes
`0x3020948c`-`0x30209494` but does not reach `0x30028a70`. So a respawn
(a new spawn count) and a new followed client start unkicked, and the
spawn flip on its own clears nothing here.

**The gun spring on the client.** VERIFIED, cgame `0x30012a60` is
`0x39e14`'s twin, with the per-axis step factored into `0x30012910`
`(dt, max, accel, speedMax, speedDecay, staticDecay)`, angle in `edx`,
speed in `ecx`; `0x300373a3`-`0x300373ce` hand it the frame as `cg.frametime
* 0.001` seconds and the state `0x3020cb94` (angles) and `0x3020cba0`
(speeds), copied back at `0x3003755c`. VERIFIED, the constants: substeps of
0.005 s (`0x30069608`, a double; `0x3ba3d70a` the float); settle under 0.25
degrees (`0x30069430`) and 1 degree a second (`0x30069328`). INFERRED, the
order, per axis and substep: settled (both zeroed, the axis reports done)
when `|angle| < 0.25` and `|speed| < 1`; else `angle += dt * speed`,
clamped to `±gunMax*` with an outward speed zeroed; `speed -= dt * accel`
while the angle is positive, `+=` while negative; `speed -= dt * speed *
speedDecay`; then `staticDecay * dt` toward 0 without crossing; then the
speed clamped to `±speedMax`. The loop stops once both axes report done.
The whole helper, step and add, runs only for an `aimDownSight` weapon
(`0x30012a79`), so a grenade or a weapon without a sight has no gun kick.

VERIFIED, off `pak0.pk3`: `m1carbine_mp` reads `hipViewKickPitchMin/Max`
40/40, `hipViewKickYawMin/Max` -15/15, `hipViewKickCenterSpeed` 800,
`adsViewKickPitch` 30..45, `adsViewKickYaw` -10..30,
`adsViewKickCenterSpeed` 800, `hipGunKickPitch` -30..-35, `hipGunKickYaw`
-2..18, `adsGunKickPitch` -60..-60, `adsGunKickYaw` -40..40, `gunMaxPitch`
and `gunMaxYaw` 5. INFERRED, worked: a hip carbine shot sets the pitch
speed to -40 degrees a second; 800 a second squared stops it after 50 ms
with the view about 1.1 degrees up, and the return at 6% of the speed
brings it back to the centre about 0.2 s later.

Not measured against a running retail client: the kick is client-side and
non-deterministic, and a capture of a retail client's cmd angles needs a
retail client. A retail-client capture of `cmd.angles` around a shot is
the open check.

**As implemented.** `crates/client/src/play/recoil.rs` holds the view kick,
the fire draw and the RNG, and steps the spring
(`vcod_common::pmove::aim::GunKick`) once per drawn frame from
`OnlineView::frame`; `PlayInput::cmd_angles` adds the kick to the cmd and
the drawn view, truncating the kick's units on their own (at most one unit
off retail's single truncation). The gun kick turns the viewmodel about the
eye and reaches the scope through `AimInput::gun_kick`; a 1.1 server
passes zero, a 1.5 or UO one its own spring (`cod15-movement.md` 6). The drawn view takes its roll from the cmd angles plus
`delta_angles` (`own_view` in `crates/client/src/main.rs`), as the
predicted `viewangles[2]`; a mounted gun's view keeps roll 0.
`OnlineView::track_spawn` clears the kick on a new `(clientNum, stats[5])`
off the newest snapshot, `OnlineView::new_gamestate` at each map load, and
a frame with no drawable playerstate (dead, or not in the world) clears it
too, which retail does not; nothing is drawn or aimed in those frames.

---

## 16. Where a shot runs in the frame

### 16.1 Retail: one packet at a time, the shot inside its cmd

**Packets.** VERIFIED, `cod_lnxded`: `SV_ExecuteClientMessage` (0x80872ec)
has one caller, 0x808ca57, inside `SV_PacketEvent` (vcod's name, 0x808c870, which
holds the 9-byte header read at 0x808c9d1); `SV_PacketEvent` is called from
`Com_EventLoop` (vcod's name, 0x806bed4, the function holding "Com_EventLoop:
oversize packet") at 0x806bfec and 0x806c1b8; the frame function 0x806ce88
calls `Com_EventLoop` in the loop at 0x806cfe0..0x806d001 and then
`SV_Frame` (vcod's name, 0x808cdf8) at 0x806d0e4, whose `VM_Call` with 10 at
0x808d1fa is vmMain case 10, `G_RunFrame` (0x50ea4). VERIFIED, decompile: a packet is executed as it is drained, between game frames and
never inside one. `Com_EventLoop` hands each received datagram to
`SV_PacketEvent` as it takes it off the queue, and its only callers are
`Com_Frame`'s loop and, on a non-dedicated process only, a second call after
`SV_Frame` (cod11-server-handshake.md, "The read schedule"). INFERRED: so
within a server frame every client's cmds, and the shots in them, run in
packet arrival order, and `G_RunFrame`'s `ClientEndFrame` pass runs after all
of them.

**A cmd.** VERIFIED, `game.mp.i386.so`, the calls `ClientThink_real`
(0x3fee0) makes after its gates: `Pmove` at 0x40466,
`BG_PlayerStateToEntityState` at 0x404f2 and 0x40510, `r.currentOrigin` from
`s.pos.trBase` at 0x4051e..0x40533, the bounds from the pmove's at
0x40539..0x40569, `ClientEvents` (0x3fd24) at 0x40589 with the
`eventSequence` saved at 0x4003b, `trap_LinkEntity` at 0x40595,
`G_TouchTriggers` at 0x405b3, `ClientImpacts` at 0x40614 and `Cmd_Activate_f`
at 0x4064e. INFERRED: that is their order, so a cmd's shot is fired after its
move and before its link and its touch pass, from the origin the move left.

VERIFIED, `ClientEvents`: its loop starts at `max(old, eventSequence - 4)`
(0x3fd3c..0x3fd4b) and runs while below `ps+0x84` (0x3fea1), reading
`ps+0x88` and `ps+0x98` at `i & 3`; its jump table at `.rodata 0x72c84` (base
159) sends 159, 160, 161 and 168 to `FireWeapon` (call at 0x3fe36), 165 to
`FireWeaponMelee` (0x3fe44), 196 to `Drop_Weapon` (0x3fe8c), and 116 to 138,
the landing pains, to `G_Damage` with means of death 0x15 (0x3fe0d).
INFERRED: the events one cmd raised, from every 66 ms chop of it, are fired
inside that cmd.

VERIFIED, `FireWeapon` (0x68d68): it branches on `weaponDef+0x70`, 0 to
`Bullet_Fire_Extended` (0x68ff9), 1 to `fire_grenade` (0x6904d) and 2 to
`Weapon_RocketLauncher_Fire` (0x690b1); the muzzle is `ent+0x134` with
`ps+0xd0` added on z (0x68df0..0x68e25), and the pitch and yaw are
`client+0x220c` and `+0x2210` (0x68dcc..0x68ddb), the aim 15.1 describes.
`FireWeaponMelee` calls `Weapon_Melee` at 0x69614. INFERRED: `ent+0x134`
is the snapped origin while a cmd runs (2.1), and a throw's missile is
spawned inside its cmd and first moved by the next `G_RunFrame` (11.4).

**The callback runs inside the shot.** VERIFIED: `Scr_PlayerDamage` calls
`Scr_ExecEntThread` at 0x5cb15 and `Scr_FreeThread` on its result at
0x5cb21, and `Scr_PlayerKilled` does the same at 0x5cc04 and 0x5cc10; both
reach the engine through the table `Scr_FarHook` copies into 0xc0fc0
(0x6c6f0), entry 43 `ExecEntThreadNum` and 83 `FreeThread`. VERIFIED,
`cod_lnxded`: entry 43 is 0x80a9674, which calls 0x809f3bc and then
0x80a79c4, and 0x80a79c4 calls the interpreter loop 0x80a45f0 (its opcode
table at 0x80a4630). INFERRED: `CodeCallback_PlayerDamage`, and the
`CodeCallback_PlayerKilled` a killing `finishPlayerDamage` reaches, run to
their first `wait` inside `G_Damage`, inside the shooter's cmd, and every
later cmd of the frame, the shooter's or anyone's, sees what they wrote.

**What a later cmd meets.** VERIFIED, `player_die` (0x49a48), its stores in
order of address: `ent+0x258` the attacker (0x49ac2), `Scr_PlayerKilled`
(0x49bb6), `takedamage` 1, `r.contents` 0x4000000 (0x49c28), the unlink
(0x49d68), `maxs[2]` 30.0 and the link (0x49d7d), health 0 and the relink
(0x49d9d); there is no store to `ps+0x4` (`pm_type`), to `client+0x20d0`
(`sessionState`) or to `r.svFlags`. VERIFIED: `ClientThink_real` tests no
`sessionState` of 1 and no health before `Pmove`; its session tests there are
the intermission arm (0x3ffca..0x4000a) and the spectator arm, which calls
`SpectatorThink` at 0x4001d; its trace mask is 0x810011 when `pm_type > 5`
and 0x2810011 otherwise (0x40092..0x400a4), and `PmoveSingle` clears
0x2000000 on `pm_type > 5` (0x33e60..0x33e66). VERIFIED: the shot mask
0x2802031 (0x688fe, and `Weapon_Melee` 0x68758) and both pmove masks leave
0x4000000 out. INFERRED, from all of that:

- a player one round kills stops no later round of the frame, and blocks no
  later mover;
- its own later cmds of the frame still run a live `Pmove` at `pm_type` 0,
  since only `ClientEndFrame` writes the dead one (5.4), so `PM_Weapon` finds
  the weapon the death dropped gone and raises `EV_RAISE_WEAPON` on the way
  to empty hands (1.8), exactly as behind a `kill` (9.2); each rewrites the
  bounds (0x40539..0x40569) and relinks, while `r.contents` stays
  `player_die`'s CORPSE, since `ClientThink_real` has no store to
  `ent+0x118`.

VERIFIED, 8.4's bullet death: events `[187, 189, 155]` over `eventSequence`
3, the 187 left from the first hit, and velocity `(-1, 82, 0)` where the
first hit's frame read 80. INFERRED: the victim's cmds that arrived behind
the shooter's ran after the kill, disarmed, and slid under the knockback
timer below, whose ground gravity the walk's clip turns into ground speed
(`cod11-mantle.md`, "The ground clip keeps the speed"), which is the 2 units.

**The pose a round is traced against.** VERIFIED: `G_DObjCalcPose`
(0x67314) is reached through vmMain case 13 (0x50ed0), which `cod_lnxded`'s
entity clip 0x809105c calls at 0x8091254 after its bounds test; its only
other caller is `ClientEndFrame` at 0x414dc under `g_debugLocDamage`.
INFERRED: the pose is computed at the trace, off the anim state
`ClientEndFrame`'s `BG_PlayerAnimation` (0x41486) last set, which is the
previous frame's, at wherever the body is linked now.

What that anim state is made of, read out of `game.mp.i386.so`:

- VERIFIED: `ClientEndFrame` (0x40e98) calls `BG_PlayerStateToEntityState`
  at 0x41180 and 0x41191 (one per `g_smoothClients` arm),
  `BG_UpdatePlayerDObj` at 0x41475 and `BG_PlayerAnimation` at 0x41486, and
  its aim trace, `G_CheckForPreventFriendlyFire`, at 0x4110d. INFERRED, off
  the addresses: the entity state is rewritten from the playerstate first
  and the DObj is fed from it after, and a client's own aim trace runs
  before its own anim update, so in `G_RunFrame`'s slot loop (0x50ab0) a
  lower slot's aim trace meets a higher slot's last-frame pose and a higher
  slot's meets a lower slot's new one.
- VERIFIED: `BG_PlayerAnimation` (0x2c1f4) hands the entity's `legsAnim`
  (`ent+0xcc`, read at 0x2c300) and `torsoAnim` (`ent+0xd0`, 0x2c318) to the
  per-channel updater 0x2aba0 (calls at 0x2c30f and 0x2c327), after the two
  condition and swing updaters at 0x2af78 and 0x2b328.
- VERIFIED: `ClientEndFrame` writes the DObj's controller hook
  `ent+0x220` as 0 at 0x40eaa and as 0x416c0 at 0x41261 on the arm that
  tests `ent+0x4` against 1, and 0x416c0 calls `BG_Player_DoControllers` at
  0x416e8. It copies `ent+0x6c` and `ent+0xd4` (`leanf`) and
  `ps.viewangles` (`ps+0xc0..0xc8`) into the client's record at
  `bgs+0x9b6ec + clientNum * 0x448` (0x41257..0x4126b), at `+0x3dc`,
  `+0x3e0` and `+0x3e4..0x3ec` (0x41273..0x412a6).
- VERIFIED: `BG_Player_DoControllers` (0x2b7f8) reads that record's
  `+0x3e4`, `+0x3e8`, `+0x3ec` (0x2b82d..0x2b83f), the swing angles at
  `+0x37c`, `+0x3ac`, `+0x3b4` (0x2b848..0x2b863) and `+0x3e0` through
  `GetLeanFraction` (0x2b8f2). INFERRED: the spine and lean controllers a
  trace's `G_DObjCalcPose` runs bend the bones by the view and lean the last
  end frame copied, not by the victim's later cmds.
- VERIFIED: `ClientThink_real` writes `ent+0x134..0x13c` from `ps.origin`
  (0x405c7..0x405dc) and `ent+0x140..0x148` as `(0, ps.viewangles[1], 0)`
  (0x405e2..0x40606), and `cod_lnxded`'s entity clip builds the rotation it
  takes the segment into entity space with off `gentity+0x140` (0x8091263,
  call 0x809126a). INFERRED: the frame a body is posed in follows its cmds,
  its yaw and origin included, while what is posed in it waits for the end
  frame.

**The knockback timer a hit starts.** VERIFIED: `finishPlayerDamage` stores
`clamp(knockback * 2, 50, 200)` into `pm_time` (`ps+0x10`) only when it
reads 0, and ORs 0x200 into `pm_flags` (0x43a22..0x43a4a), both on the arm
where the knockback is non-zero (4.5). VERIFIED, in pmove: `PM_Friction`
tests `pm_flags` 0x200 at 0x2e4fb and jumps past the ground term;
`PM_WalkMove` takes an accel of 1.0 on it (0x2f4a2) and, on it, subtracts
`ps.gravity * frametime` from `velocity[2]` (0x2f59c..0x2f5b5);
`PmoveSingle` calls `PM_DropTimers` (0x342f9) ahead of `PM_WalkMove`
(0x3431b). INFERRED: for the timer's 50 to 200 ms a hit player slides with
no ground friction, and a cmd as long as the timer runs its walk with the
timer already dropped.
VERIFIED, by capture: under the timer `PM_SlideMove` leaves a push into a
wall in the velocity (`cod11-player-clip.md` 8.5, `probe_slide`).

### 16.2 As implemented

`Server::replay_moves` (`crates/server/src/server.rs`) runs every packet
since the last tick in the order the server executed it: `handle_packet`
numbers each client packet, a bot's cmd is a packet of its own numbered
after the real ones, each queued cmd and `kill` carries its packet's number,
and the replay takes the lowest number across all clients each time, a
`kill` ahead of its packet's cmds. Per cmd it runs the move, mirrors what the
move left onto the host, fires the shots and swings the cmd raised and
spawns the missile of any throw it raised (11.4), and then runs the touch
pass, the item pass and the use key. A shot is traced against
every client as its own packets so far left it; each impact goes out and
each hit runs `CodeCallback_PlayerDamage` there and then, and what the
callback queued for the victim (the damage, the death, the drop) is applied
to its sim before any later cmd runs. What the mover's own touch pass, item
pass and use key queued for it, a `trigger_hurt`'s damage or death and a
pickup's ammo and event, is applied the same way at the end of that cmd, so
a hit takes only its own callback's ops off the victim's queue
(`cod11-items.md` 13.2). `finishPlayerDamage`'s timer
is `ClientSim::take_damage`, carried as `knockback_flags` beside
`knockback_ms` (`vcod_common::pmove`), and the wire's `pm_flags` and
`pm_time` carry it.

A body's pose is `ClientSim::commit_pose`'s: its models, both anim channels
with their start times, its view pitch and its lean, taken in the aim-trace
pass right after that slot's own aim trace, the place `BG_PlayerAnimation`
holds in retail's `ClientEndFrame`. `hit_body` pairs it with the origin, yaw
and box the last cmd left, so every round, blast probe and aim trace until
the next end frame meets that pose wherever the body has moved since.
`a_body_stood_up_since_its_end_frame_is_still_posed_crouched`
(`crates/server/src/game/combat.rs`) pins the split.

What still differs, each INFERRED from 16.1 and not measured:

- The packets of a tick run at the tick, after the clock has advanced;
  retail runs them as they arrive, on the previous frame's `level.time`
  (15.6's 50 ms). Their relative order is the same.

The pitch the pose bends the spine by eases after the view the way retail's
does (16.3), not whole at the end frame that copied it.

**A turret round inside its gunner's end frame.** VERIFIED,
`game.mp.i386.so`: `G_RunFrame`'s slot loop (0x50ab0..0x50add) calls
`ClientEndFrame` once per slot at 0x50abd; `ClientEndFrame` calls
`G_CheckForPreventFriendlyFire` at 0x4110d, `G_CheckForCursorHints` at
0x41119, `P_DamageFeedback` at 0x41128, `BG_PlayerAnimation` at 0x41486 and
`turret_think_client` at 0x414b1; the turret fire 0x521d4 calls `Bullet_Fire`
at 0x522f1, `Bullet_Fire` calls `Bullet_Fire_Extended` at 0x691b4, and that
calls `G_Damage` at 0x68beb, which calls `Scr_PlayerDamage` at 0x49e62.
INFERRED, off those calls and 16.1's callback path: a gunner's
`CodeCallback_PlayerDamage`, and the `CodeCallback_PlayerKilled` and
`player_die` a killing round reaches, run inside the gunner's own
`ClientEndFrame`, so a higher slot's turn of the same loop (its feedback,
its aim trace, its pose, its own turret) reads what they wrote, and a lower
slot's turn already ran.

Measured. VERIFIED, `client-probes/probe_turret` under `probe_pose 1` on
retail, mp_carentan's gun at (1712, 1830, 8): the
`--save-turret` gunner held the trigger 8 s at the axis client's eye, one
round a frame, and the axis client (`--probe-pitch-flip 85`) held view pitch
85 and 0 in alternate 400 ms windows; the gsc logged every hit's time and
point instead of landing it. Per pitch change, `k0` is the frame whose
snapshot first read the new pitch, so its cmds carried it and its
`ClientEndFrame` copied it; the columns are each frame's hit x:

| target slot | change | k-1 | k0 | k+1 | k+2 | k+3 |
|---|---|---|---|---|---|---|
| 1, gunner 0 | 0 to 85 | 1518.34 | 1518.34 | 1515.40 | 1512.97 | 1512.08 |
| 0, gunner 1 | 0 to 85 | 1518.11 | 1515.98 | 1513.45 | 1512.48 | 1512.03 |

Over the whole run, counting the changes whose three frames k-1 to k+1 all
hit: with the target in slot 1, 11 of 11 read k0 within 0.2 units of k-1;
with it in slot 0, 18 of 18 had already moved at k0. INFERRED: a round meets
a higher slot posed as its last end frame left it and a lower slot posed as
this frame's, 16.1's reading. VERIFIED, the same rows: the point eases over
four to six frames after a change rather than stepping, so the pitch the
controllers bend the spine by is smoothed between end frames; 16.3 reads
where.

Ours does the same: each gunner's rounds are traced inside its own turn,
right after its `commit_pose`, meeting a lower slot's new pose and a higher
slot's last-frame one (pinned by
`a_round_meets_the_torso_easing_after_a_pitch_flip`,
`crates/server/tests/turret_ab.rs`; a stance change's clip comes in at
weight 0, so it reads the same from either slot,
`a_stance_change_blends_in_whatever_the_slot_order`, movers doc 15; the same `probe_pose` run against
`vcod-server` switched hit location at k+1 with the target in slot 1 and at
k0 with it in slot 0), and delivered there by
`deliver_turret_rounds` (`crates/server/src/server.rs`): the callback, the
weapon and sim ops it queued, the vitals mirror, a higher victim's end frame
again for its feedback, and every body refreshed for the later slots' aim
traces. A victim below the gunner takes its feedback on the next frame.

- A higher-slot gunner the round kills is dead on its own turn, so
  `turret_think_client` releases instead of firing (0x5236b, turrets doc 8)
  and a held trigger raises no shot that frame; pinned by
  `a_gunner_killed_by_a_turret_round_lets_go_that_frame`. A lower-slot
  gunner killed by a higher slot's round keeps the gun until its turn on the
  next frame; INFERRED, `player_die` calls no release (its calls at
  0x49a6e..0x49d9d).
- Two gunners on one target in one frame: INFERRED, from 16.1's
  `player_die` stores (`r.contents` 0x4000000 at 0x49c28) and the shot mask
  0x2802031 without that bit, the later slot's rounds pass the body the
  earlier slot's round killed. Ours drops a dead sim from the traced bodies
  (`ClientSim::hit_body`), and the delivery inside the loop makes that
  death visible before the later gunner traces. No test pins it: carentan's
  rig has two clients and the guns' arcs do not overlap. No retail capture
  covers it. Two gunners firing at each other are captured:
  cod11-turrets.md 13.10.

VERIFIED, two runs against ours, the `probe_passthru` recipe
of `client-probes/README.md` with one `--probe-target --probe-team axis` and
one `--save-hit --probe-sweep --probe-team allies` shooter, `vcod-server
mp_carentan --gametype-script .../probe_passthru.gsc --set probe_teleport=1`,
eight bullet deaths, each off two head hits of 67: every death frame ends its
ring on 189 then 155, two past the frame before it, at `pm_type` 6 and
`torsoAnim` 512, which is 8.4's retail frame. The victim's speed on seven of
them reads 82, 82, 82, 82, 81, 81 and 79, and on the eighth 89, whose frame
before still read 16 from the first hit, against retail's 82 (8.4). Seven of
the eight also carry `EV_ITEM_PICKUP` (146) ahead of the pain: the probe's
spot is where the last life's `dm.gsc` health pack fell, and the victim's
cmds behind the first hit now walk over it at 33 health and take it to 58,
which the `EV_PAIN` parm of 58 reads at the end frame; retail's own capture
stood elsewhere. The first run also logged one callback reading the
victim's `self.origin` at its spawn point, 3500 units from the hit, on a
round that met it in the frame after the gametype's `setOrigin`: the
teleport's op had not reached the host's copy, which it now does
(`apply_sim_op`). The second run logged none.

Pinned by `a_bullet_deaths_frame_is_retails`, which holds the victim's death
frame to the capture's line on health, `pm_type`, the event ring and its
parms, the damage feedback and `torsoAnim`, and
`a_player_killed_earlier_in_the_frame_stops_no_later_round`
(`crates/server/tests/combat.rs`), and
`a_damage_knockback_slides_free_of_friction` (`vcod_common::pmove`).

### 16.3 The torso pitch eases after the view

**The swing.** VERIFIED, `game.mp.i386.so`: `BG_PlayerAnimation` (0x2c1f4)
calls the angle updater 0x2af78 first, and its last call (0x2b317) is
`BG_SwingAngles` (0x2ae00) on the pitch: destination the record's
`viewangles[0]` (`+0x3e4`, `ClientEndFrame`'s copy at 0x41288) less 360
when above 180.0 (0x6ef60, 0x6ef64), times 0.6 (0x6ef68); swing tolerance
0, clamp 45.0 (0x6ef54), speed 0.15 (0x6ef6c, which Ghidra labels as a
function); the angle at record `+0x3b4` and its swinging flag at `+0x3b8`.
VERIFIED: the destination is 0 instead when `eFlags & 1` (dead), `eFlags &
0xc000` (mounted) or the record's movetype condition (`+0x410`, condition 3
of the name block at 0x6e424) has bit 16 or 17 set, which the movetype name
block at 0x6e260 makes `climbup` and `climbdown`.

VERIFIED, `BG_SwingAngles` (0x2ae00) is Q3's `CG_SwingAngles` with a new
step scale: it starts swinging when `AngleSubtract(angle, dest)` leaves
`±tolerance`; the step is `frametime * scale * speed` toward the
destination with `scale = max(|AngleSubtract(dest, angle)| * 0.05, 0.5)`
(0x6ef40, 0x6ef44), stopping on it when the step would pass it; the angle
goes through `AngleMod`; then it is clamped to `dest ∓ (clamp - 1)` when
more than `clamp` away. VERIFIED: the frametime is the int at `bg + 8`
(0x2aea9), which `G_RunFrame` (0x50478) writes as `levelTime - previous
levelTime`, 50 ms at `sv_fps` 20. INFERRED, from the arithmetic: a flip from
0 to 85 moves the torso pitch 19.1, 12.0, 7.5, 4.7, 2.9, 3.75 and 1.1
degrees over seven end frames, and 85 back to 0 the same steps the other
way.

VERIFIED, `cgame_mp_x86.dll`: `CG_PlayerAnimation` (vcod's name, 0x30004e40)
calls the same updater (0x300040e0) whose pitch call passes 0.6
(0x30069508), the 180 / 360 fold (0x30069370, 0x30069374), clamp 45
(immediate 0x42340000) and speed 0.15 (immediate 0x3e19999a) to the same
swing (0x30003ec0, scale constants 0.05 at 0x3006945c and 0.5 at
0x3006930c), stepping by `cg.frametime` (0x300f0318). VERIFIED: the input
pitch is the entity's lerped `apos` pitch, copied into the record at
0x3001d090 for an `eType` 1 entity, and `BG_PlayerStateToEntityState`
(`game.mp.i386.so` 0x2ccb4..0x2cd41) writes `ps.viewangles` whole into
`apos.trBase`, truncated to whole degrees on both of `ClientEndFrame`'s
arms (snap argument 1 at 0x41179 and 0x4118a). VERIFIED, a retail snapshot
of the target in this section's capture: `apos base [85.0,0.0,0.0]`.

**The split.** VERIFIED, `BG_Player_DoControllers` (0x2b7f8), off its
`.rodata`: for a body not mounted (it returns at once on `eFlags & 0xc000`,
0x2b804), with `P` the swing angle and `V` the view pitch, it bends
`back_low` by `0.2 P` (0x6efd4), `back_mid` by `0.3 P` (0x6efcc), `back_up`
by `0.5 P` (0x6efa8), `neck` (0x6ef95) by `0.3 (V - P)` and `head`
(0x6ef9a) by `0.7 (V - P)` (0x6efe8). Prone (`eFlags & 0x40`) first scales
`P` by 0.5 when positive and 0.25 otherwise (0x6efa8, 0x6efac), then bends
`back_up` by all of it and `back_low` and `back_mid` by none. Either way
`back_low` adds `AngleSubtract(fTorsoPitch, fWaistPitch)` and `pelvis`
takes `AngleSubtract(fWaistPitch, fTorsoPitch)` when either is non-zero
(`ent+0xe8`, `+0xec`). INFERRED: `cgame_mp_x86.dll` 0x30004710 carries the
same tag sequence and constants (0.2 at 0x300693d4, 0.7 at 0x300694f0, 0.5,
0.3), not traced line by line.

**The axis.** VERIFIED, `G_DObjSetControlTagAngles` (0x7721c): the control
quaternion is `yaw ⊗ pitch ⊗ roll` from `YawToQuaternion` (z),
`PitchToQuaternion` (y) and `RollToQuaternion` (x), half-angle π/360
(0x72b58). VERIFIED, `cod_lnxded`: game syscall 0x7f (0x080c5050) sets the
bone's bit in both the RotTrans and the control mask; `G_DObjCalcPose`
(game 0x67314) runs the anim calc (syscall 0x6f, 0x080bcef8), then the
controller hook, then the skeleton calc (0x70, 0x080c337c), whose rotation
pass takes a control bone's world rotation as the control quaternion times
the parent's world rotation, against parent times local for any other
bone. INFERRED: a control rotation replaces the bone's local rotation and
turns it about the model's axes, so pitch tips about the model's left axis
whatever the bone's own frame; `CoDMP.exe` 0x00481780 has the same two
product branches, operand order not resolved. In USAirborne3's frames a
turn about `back_up`'s own Y leans the body sideways (the trap).

**Measured.** VERIFIED, `crates/server/tests/fixtures/turret/
mp_carentan-dm-pitch-ease.txt`, retail: 16.2's `probe_pose`
recipe twice, the target in slot 0 and in slot 1, 8 s of rounds each. Per
flip to 85, the round's x less the level body's (k0 the frame whose snapshot
first read the new pitch), the range over the run's complete flips, and
ours from the same view and flips (a dash is a miss):

| target slot | | k0 | k+1 | k+2 | k+3 | k+4 | k+5 |
|---|---|---|---|---|---|---|---|
| 1 | retail, 6 flips | -0.08..0.02 | -1.75..1.73 | -4.37..-1.32 | -5.41..-3.09 | -5.87..-4.03 | -6.41..-4.63 |
| 1 | ours | 0.00 | -2.10 | -4.70 | -5.67 | -6.08 | -6.32 |
| 0 | retail, 8 flips | -2.11..1.23 | -4.68..-1.87 | -5.69..-3.50 | -6.31..-4.30 | -6.76..-4.88 | -7.59..-5.41 |
| 0 | ours | -2.12 | -4.69 | -5.66 | -6.07 | -6.30 | - |

INFERRED: the higher slot's flip-frame round meets the old pose (16.1) and
then the point follows the swing's steps, the lower slot one frame ahead;
some flips of either run first step 1.3 to 1.7 the other way, which ours
also does on some flips: the neck and head take what the torso has not yet,
and where the round lands then depends on the gunner's scatter.

Ours: `vcod_common::playerpose::{BodyAngles, Controllers,
apply_controllers}` (16.4), run by `ClientSim::commit_pose` (a 50 ms step
per end frame) and by the client
per drawn player per rendered frame, off the `apos` pitch the server now
sends whole and truncated. `a_round_meets_the_torso_easing_after_a_pitch_flip`
(`crates/server/tests/turret_ab.rs`) replays the capture's view and flips
and holds ours inside retail's per-frame spread, within 0.6 units. GAP: a
body at or a step short of the full `0.6 * 85` is mostly missed by the round
that retail lands about 1 unit further back on the torso; the gate skips the
two frames posed there.

### 16.4 The yaw and lean halves, and `tag_origin`

16.3 covers the pitch. This section covers the rest of the updater (0x2af78)
and of `BG_Player_DoControllers` (0x2b7f8). The constants are `.rodata`
floats, read out of the file at the addresses given.

**The record.** VERIFIED, `game.mp.i386.so`: 0x2af78 passes the angle and
flag pairs at `+0x37c`/`+0x380`, `+0x3ac`/`+0x3b0` and `+0x3b4`/`+0x3b8`
to `BG_SwingAngles` (0x2b27b, 0x2b189, 0x2b317). `BG_PlayerAnimation`
(0x2c1f4) passes `+0x37c` and `+0x3ac` as the legs and torso frame
records (0x2c307, 0x2c31f). INFERRED: those are the legs yaw, the torso yaw
and the torso pitch, each with its swinging flag. VERIFIED: `+0x3dc` is the
entity's `angles2[1]` (`ent+0x6c`, `ClientEndFrame` 0x41273), which is
`movementDir` (`docs/protocol-1.1.md`). Below, `M` is `movementDir` and
`Y` is `AngleMod(viewangles[1])` (0x2afc7).

**Conditions.** VERIFIED: `BG_PlayerAnimation` calls the updater (0x2c208)
and then 0x2b328 (0x2c215). 0x2b328 writes the record's `movetype`
condition (`+0x410`) from the legs anim's record `+0x54` when that word is
non-zero (0x2b6d4..). It writes `firing` (`+0x428`) from `eFlags & 0x400`.
The condition offsets are 8 bytes apart from `weapons` at `+0x3f8`, in the
enum order of the name block at 0x6e424. VERIFIED, `BG_ParseCommands`
(0x28650): every legs or both line in a state's movetype block ORs
`1 << movetype` into its anim's `+0x54` (0x287f1). A `strafing` condition
of value 1 or 2 ORs 0x10 or 0x20 into `+0x50` (0x286e4, 0x2885a). INFERRED:
the updater therefore reads the conditions the previous frame left. A
server's pmove also writes `movetype` each cmd, ahead of the end frame.

**Which swings start.** VERIFIED, 0x2afff..0x2b0fd: a body is mounted
(`eFlags & 0xc000`), climbing (`movetype & 0x30000`) or neither `idle` nor
`idlecr` (`movetype & 6` clear). Such a body sets all three swinging flags.
An idle one sets the torso's two only while `firing`.

**Torso yaw.** VERIFIED, 0x2b100..0x2b189, `BG_SwingAngles(dest, 0, clamp,
bg_swingSpeed, ...)`. The speed is the cvar's value (`bg_swingSpeed + 8`,
default 0.2 in `crates/server/src/cvars/registry.rs`). cgame registers its
own copy: the cvar table row at 0x30075420 is the vmCvar 0x301d94a0, name
`bg_swingSpeed`, default `"0.2"`, flags 0x200 (`CVAR_CHEAT`). VERIFIED. The
server's is cheat protected too: an rcon `bg_swingSpeed 1` on retail
printed `bg_swingSpeed is cheat protected.` VERIFIED by capture. vcod reads the level's value once per end frame
(`Cod1Module::run_frame_cod1`, into `commit_pose`) and the client's console value each
drawn frame (`EntityScene::swing_speed`). Destination and
clamp by case, first match:

| case | destination | clamp |
|---|---|---|
| dead (`eFlags & 1`) | `Y` | 90 (0x6ef4c) |
| climbing | `Y + M` | 0 |
| prone (`eFlags & 0x40`) | `Y` | 90 |
| `eFlags & 0x400` | `Y` | 45 (0x6ef54) |
| `eFlags & 0x200` | `Y` | 90 |
| otherwise | `Y + 0.3 M` (0x6ef50) | 90 |

INFERRED: at 0.2 and a 50 ms frame the step is `max(|d| * 0.5, 5)`
degrees, so the torso closes half the gap a frame.

**Legs yaw.** VERIFIED, 0x2b18e..0x2b280, clamp 150 (0x6ef58) on every
arm:

- dead: destination `Y`, tolerance 0;
- prone: no swing; the flag clears and the angle is set to `M + Y`;
- legs anim `+0x50 & 0x30`, a strafe anim: the flag clears, then
  destination `Y`, tolerance 0;
- swinging: destination `Y + M`, tolerance 0;
- otherwise: destination `Y + M`, tolerance 40 (0x6ef5c).

INFERRED: an idle body's legs stay put until the view is more than 40
degrees off them, then swing all the way. VERIFIED, retail, the movers doc's
`linkTo` capture (section 15): after `setPlayerAngles((0, 180, 0))` the
hand tag's yaw reads 179.47, 269.54, 44.62, 21.99, 10.79 and 5.18. INFERRED:
that is the legs' yaw off the view at 50 ms steps and `bg_swingSpeed` 0.2.
The legs read 0, then 90 (half of the 180 gap), and the view is back at 0
from the third frame, so they halve back: 45, 22.5, 11.25, 5.6. `linkto_ab` holds ours to it within 4 degrees. A moving one's always follow. A
strafe anim's legs face the view, because the anim already runs sideways.
VERIFIED, 0x2b283..0x2b2a6: a mounted body then sets both yaws to `Y`, a
climbing one to `Y + M`.

**What the controllers read.** VERIFIED, 0x2b81c..0x2b8c7. `H` is the
record's view (`+0x3e4..0x3ec`). `T` is `(P, torso yaw, 0)`, with `P` 0
while climbing and scaled when prone (16.3). `L` is `(0, legs yaw, 0)`.
Two `AnglesSubtract` calls then make `H = H - T` and `T = T - L`.

**Lean.** VERIFIED, `GetLeanFraction` (0x6ba64) is `(2 - |l|) * l`, the 2.0
at 0x7a340. With `f` that of the record's `leanf`, 0x2b8fb..0x2b9f5 sets
the following, and all three stay 0 when `f` is 0:

- `T[2] = 46.25 f` (50.0 at 0x6efb0 times 0.925 at 0x6efb4), times 1.5
  (`f > 0`, 0x6efb8) or 1.8 (0x6efbc) when crouched (`eFlags & 0x20`);
- `H[2]`: the same 46.25 f, with the same crouch scale, or times 0.5 when
  prone;
- the local tag's translation `y`: `-f`, times 1.5 crouched or 2.5 (0x6efc0)
  prone.

INFERRED: `H[2]` replaces the view's roll outright.

**`tag_origin`.** VERIFIED, 0x2b9f5..0x2ba6f:
`G_DObjSetLocalTag(obj, ?, "tag_origin", trans, L)` with:

- `trans = (0, lean shift, fTorsoHeight)` (`ent+0xe4`, 0x2b8e9);
- `L[1] = AngleSubtract(legs yaw, viewangles[1])`, except when dead;
- `L[2] += 3.75 f` (0.075 at 0x6efc4) when not prone;
- `L[0] += fTorsoPitch` (`ent+0xe8`) when prone.

VERIFIED, `G_DObjSetLocalTag` (0x67128) builds the same `yaw ⊗ pitch ⊗
roll` quaternion as the control tags (16.3; `QuatMultiply` 0x3e504 is
`b ⊗ a` in Hamilton terms). It stores the translation at entry `+0x14`
and goes through `trap_DObjSetRotTransIndex`, whose engine side
(`cod_lnxded` 0x080c5008) sets the bone's bit in the rotation-and-translation
mask (`skel+0`) and not the control mask (`skel+0x10`). VERIFIED,
`cod_lnxded` 0x080bcef8: the anim calc builds its skip mask from
`skel+0` and resets only bones outside it. INFERRED: the tag replaces `tag_origin`'s local
rotation and translation, so the whole body turns about the feet by the
legs' yaw off the view. VERIFIED: `ClientThink_real` sets the entity's
angles to `(0, viewangles[1], 0)` (16.1). INFERRED: the legs face the legs
yaw, and the yaw controllers below turn the spine back so the head faces
the view.

**The split.** VERIFIED, 0x2ba74..0x2bd70, `[pitch, yaw, roll]` per bone.
`s` is `AngleSubtract(fTorsoPitch, fWaistPitch)` when either is non-zero,
else 0.

| bone | standing or crouched | prone |
|---|---|---|
| `back_low` | `0.2 T0 + s`, `0.4 T1`, `0.5 T2` | `s`, `-1.2 T2`, `0.3 T2` |
| `back_mid` | `0.3 T0`, `0.4 T1`, `0.5 T2` | `0`, `0.1 T1 - 0.2 T2`, `0.2 T2` |
| `back_up` | `0.5 T0`, `0.2 T1`, `-0.6 T2` | `T0`, `0.8 T1 + T2`, `-0.2 T2` |
| `neck` | `0.3 H0`, `0.3 H1`, `0` | same |
| `head` | `0.7 H0`, `0.7 H1`, `-0.3 H2` | same |
| `pelvis` | `-s`, `0`, `0` | same |

The weights are 0.2 (0x6efd4), 0.3 (0x6efcc), 0.4 (0x6efe0), 0.5
(0x6efa8), -0.6 (0x6efe4), -1.2 (0x6efc8), 0.1 (0x6efd0), 0.8 (0x6efd8),
-0.2 (0x6efdc), 0.7 (0x6efe8) and -0.3 (0x6efec). INFERRED: standing, the
yaws sum to `(legs - view) + (torso - legs) + (view - torso) = 0`. The head
faces the view, the chest the torso yaw, the hips the legs. Prone, the lean
turns into a twist.

**Respawn.** VERIFIED, `ClientConnect` clears the whole `0x448`-byte
record (memset at 0x424da, keeping `+0x440`). No other store in the module
writes `+0x37c..+0x3b8`, by offset or by `bgs` absolute (`0x9ba68..
0x9baa4`), beyond the updater. The `+0x37c` and `+0x380` stores in
`ClientSpawn` (0x429a5) and `StopFollowing` (0x46ba7) are on the
playerstate. INFERRED: the swings carry across a death and a respawn. Ours
keeps `ClientSim::pose` across a spawn and starts it fresh at connect.

**The gunner's aim group.** A mounted body runs no controllers, and its
legs blend the turret anim's leaves by the placement's weights (turrets doc
7.2), on the client and in ours. The server's locational pose takes the
blend `turretpose::place_gunner` left on `ClientSim::gunner_leaves`; that
placement runs in the gunner's turret think, after its end frame, so ours
poses it one frame late. The fallback descent, with no placement, reads the
view pitch down positive (`descend_aim`; `-viewangles[0]` picks the `15up`
row for a gunner looking down).

Ours: `vcod_common::playerpose::{BodyAngles::step, update_conditions}`
for the updater and its conditions. `Controllers::new` and
`apply_controllers` do the split and the local tag. The anim record is
`AnimScript::anim_records`. `ClientSim::commit_pose` sets the movetype from
its own pmove selection, then steps and updates. The client steps
per drawn player per rendered frame, off the lerped `apos`, `angles2[1]`,
`eFlags` and the legs anim's record. A dying body runs them too (below,
"The dying body"). `fTorsoHeight`,
`fTorsoPitch` and `fWaistPitch` come from `BG_CheckProneValid`'s ground
samples and reach the entity through `BG_PlayerStateToEntityState`
(0x2ce55..0x2cea3), scaled by the eye's leg, only when prone
(cod11-mantle.md, "The ground samples"); ours sends them and poses its hit
bodies with the entity's copy. Player `eFlags` 0x400, which narrows the
torso clamp to 45, is 16.5.

**The dying body.** VERIFIED, `game.mp.i386.so`: `ClientEndFrame` arms the
controller hook (`ent+0x220` = 0x416c0, 0x41261) on every `eType` 1 entity,
a dead session's included, and `BG_Player_DoControllers` (0x2b7f8) returns
early only on `eFlags & 0xc000`; its one test of `eFlags & 1` (0x2b9f5..)
skips the `AngleSubtract(legs yaw, viewangles[1])`, so a dead body's
`tag_origin` yaw is the legs' world yaw. `G_DObjGetWorldTagMatrix`
(0x6746c) then turns the posed tag by `r.currentAngles` (`ent+0x140`), the
view yaw: the body is turned by the yaw twice. INFERRED: no trace meets
that pose, since a dead body's contents are 0 or CORPSE (5.1), so a tag
lookup is what reads it. VERIFIED, 1.5b's (0x1a389, the
`eFlags & 1` test beside the subtract) and UO's (0x1e761, with the rider's
0x100000 beside it) hooks skip the same subtract. VERIFIED, 1.5b's ease
(0x1ad71) steps toward that target with no dead test; so does UO's
(0x1f107, called from `BG_Player_DoControllers` at 0x1f2fe for each
control bone and the `tag_origin` angles).

VERIFIED (capture), retail 1.1d mp_carentan,
`client-probes/probe_dyingpose` (fixture
`crates/server/tests/fixtures/dyingpose/mp_carentan-dm.txt`): a body held
at world yaw 90 with a script_origin linked to each of eight bones, then
`suicide()` into the dead session. The ten live frames and the death frame
matched ours to the hundredth. From the second dead frame on (the death
clip, `legsAnim` 20, already at its last frame) retail's bones sat at ours
turned another 90 degrees about the origin: pelvis (-3.72, -0.04) off the
origin where ours read (-0.04, 3.72), head (20.48, 5.30) where ours read
(5.30, -20.48). Ours had skipped the controllers on `EF_DEAD`. A body facing
0 cannot tell the two apart: an earlier run at yaw 0 matched.

VERIFIED, `cgame_mp_x86.dll`: `CG_Player` (0x30028210) draws no `EF_DEAD`
player (cod11-movers.md 15) and the hook call at 0x3001d460 is for `eType`
1 only, so the client never poses a dying body; a corpse (`eType` 2) runs no
controllers.

VERIFIED (capture), the same probe on retail 1.5b and UO
1.51b (`mp_carentan-dm-1.5.txt`, `-uo.txt` beside the 1.1d fixture): the
deaths drew `legsAnim` 16 and 18, and every bone of the 60 frames, live and
dead, sits within the logs' rounding of ours.

VERIFIED (capture), all three servers: the death frame's playerstate
carries `legsTimer` and `torsoTimer` both at the clip's length + 50 (3116 on
1.1d, 2050 on 1.5b, 1983 on UO), and every later snapshot reads them run
down by the cmds since: `commandTime + timer` holds one value (14600 on
1.1d) until they reach 0. The suicide ran in a script thread, outside any
cmd, so the timer counts from the last processed cmd's serverTime, not the
frame's time. INFERRED: `BG_AnimScriptEvent`'s `both` line sets the two
timers to `duration + 50` (`BG_PlayAnim`, 1.1d 0x3c338) and each cmd's
pmove runs them down by its msec. The clip's length is the engine's
`XAnimGetLength` (syscall 0x5d, `cod_lnxded` 0x08088b3d..0x08088b5b,
VERIFIED disassembly): the part's frame count over its float framerate,
times 1000.0 (0x080d4ea8), stored under a 0xc00 control word, so
truncated: 1.1d's 92 frames at 30 read 3066, not a rounded 3067. INFERRED:
1.5b and UO store it the same way
(their clips here are exact or round down either way).

VERIFIED (capture), a death raised inside the packet pass on
all three servers (`mp_carentan-dm-hit-target-killtimers{,-1.5,-uo}.txt`,
the `--probe-kill-cmds 1,3` target with `ct`, `legsTimer` and `torsoTimer`
on each `!trace`): the `kill` runs ahead of its message's cmd, and the
probe's cmds behind it are 1 ms apart, so the death frame reads the
clip's length + 50 less 1 or less 3 (949 and 947 for a 900 ms clip; 27
deaths, each read against its own clip). The hold is set on the last cmd
ahead of the `kill`, and every cmd behind it runs it down, alive or dead.
VERIFIED (capture), 1.1d, a bullet death inside the shooter's cmd
(`mp_carentan-dm-hit-target-paintimers.txt`, ms 26759, index 21, 900 ms):
`ct + timer` reads 304200, so the hold was set at 303250, between the
victim's snapshot cmds 303233 and 303284; that is the frame's
`level.time` too, so this one capture does not split the two clocks; the
`kill` captures do. VERIFIED (capture), same file: its two surviving
hits (ms 26200, health 60; ms 26557, health 24) leave both timers at 0
and `legsAnim` on the idle or the knockback's run.

VERIFIED (disassembly), the event argument at every `BG_AnimScriptEvent`
call site: 1.1d `game.mp.i386.so` (13 `.rel.text` relocations, 0x2ee1a ..
0x49b85) passes 1, 2, 3, 4, 5, 6, 7, 10 or 13; 1.5b (16 PLT calls, 0x1fcb8
.. 0x46f5d) and UO (19, 0x247f7 .. 0x4fb93) the same set, UO adding 14 and
15. VERIFIED (data), the event name table (8-byte rows: 1.1d 0x7b478,
1.5b 0x80120, UO 0xa9120) starts `PAIN`, `DEATH`, `FIREWEAPON`, `JUMP`,
`JUMPBK`, `LAND`, `DROPWEAPON`, `RAISEWEAPON`, `CLIMBMOUNT`,
`CLIMBDISMOUNT`, `RELOAD`, `CROUCH_TO_PRONE`, `PRONE_TO_CROUCH`,
`MELEEATTACK` (UO then `LMG_DEPLOY`, `LMG_BREAKDOWN`). So no server game
module raises `PAIN` (0): a surviving hit sets no anim and no timer, and
`DEATH` (1) is raised only from `player_die` (1.1d 0x49b85, 1.5b 0x46f5d,
UO 0x4fb93).

Ours: `BodyPose::target_controllers` runs on a dead body, eased under 1.5
and UO like any other; `dyingpose_ab::a_dying_body_poses_like_{retail,15,uo}`
hold every bone of all 60 frames within 0.015 of retail's (the logs'
rounding). `AnimState::timers` writes `legsTimer` and `torsoTimer` off each
event's hold, `take_damage` stamps the death's on the client's last cmd
(`dyingpose_ab::a_death_runs_its_timers_down_like_*`, and inside the
packet pass `kill_cmds_ab::a_kills_death_timers_match_*_by_its_cmds`, which
fail with the frame clock), a surviving hit returns before the anim, and
`combat.rs::a_shot_takes_health_and_a_second_one_kills` holds a hit's and a
bullet death's timers on ours, `a_shot_sets_the_victims_timers_as_retail_does`
on the capture. `xanim::duration_ms` truncates in extended precision.

### 16.5 `eFlags` 0x400, the trigger held

VERIFIED, `game.mp.i386.so`, `PmoveSingle` (0x33dfc): byte `ps+0x82` ORed
with 4 (0x33f72) or ANDed with 0xfb (0x33f82) beside a test of the cmd's
buttons byte against 2 (0x33f6a); byte `ps+0x81` ANDed with 0xfb (0x33f91);
`pm_type` against 5 (0x33f9a); `pm_flags` byte 1 against 8 (0x33fa0);
`weaponstate` (`ps+0xb4`) against 0 and 3 (0x33fac, 0x33fb0);
`PM_WeaponAmmoAvailable(ps.weapon)` (0x33fbf) and a test of its return;
the buttons byte against 1 and 2 (0x33fd3, 0x33fd8); byte `ps+0x81` ORed
with 4 (0x33fdf); then `pm_type` against 5 (0x33fed), the buttons byte
against 1 (0x33ff8) and `pm_flags` byte 1 ANDed with 0xf7 (0x34000).
VERIFIED, `PM_WeaponAmmoAvailable` (0x3abe8): it returns
`ps+0x20c[def+0x1a8]`, the `ammoclip` entry at the weapon's `clipIndex`.
INFERRED, off those branches: every cmd clears `eFlags` 0x400 and sets it
again when the cmd holds attack (buttons 1) without talk (buttons 2), the
player is not at intermission (`pm_type` 5) and not under `PMF_RESPAWNED`
(`pm_flags` 0x800), the weapon is ready or firing as the cmd starts, and
its clip is not empty; the talk bit sets 0x40000 the same way, and a cmd
with attack released clears `PMF_RESPAWNED`. The bit runs ahead of the
move and `PM_Weapon` (0x34274 onward), so it reads the weapon state the
cmd starts from, and the snapshot carries the last cmd's.

VERIFIED, two `--save-combat` runs with `ps.eFlags` added to every trace, dm on mp_carentan, kept out of the fixtures: retail's carbine
taps read 1040 (0x410) on the snapshot whose `weaponstate` first reads 3 and
16 on every other, 1072 and 1104 the same crouched and prone; the run on
`vcod-server` reads the same shape (16550, 18750, 18950, 19300, 19500: 1040
with `weaponstate` 3). The tdm hit target capture's line 80 is one of these
(turrets doc 12.4). Turrets doc 12.4's gunner reads 0xC418 on every held
frame and 0xC018 once the trigger is up, which this rule gives too.

vcod: `spectate::attack_flag` per cmd in `ClientSim::step` (any `pm_type`
but the intermission, `PMF_RESPAWNED` clear), on `ClientSim::attacking`;
`eflags()` ORs it with the gunner's own `firing`. A dead body (`pm_type` 6)
passes the `pm_type` test, so its first cmd, which still holds the weapon,
can set the bit; the dead arm then zeroes the weapon and the next cmd
clears it.

VERIFIED live: `client-probes/probe_rise` on
`tools/run_probe.sh mp_carentan` with a `--probe-uo-weapon direct
--probe-cmd-ms 50` client (one cmd a frame) holding the carbine's trigger
through two `suicide()`s (fixture
`crates/server/tests/fixtures/playerstate/mp_carentan-dm-slope-50ms-rise-1.1.txt`).
Through a killed callback that only sets the session `dead`, the death's
snapshot (30000) read `pm_type` 6, `weapon` 12 and `eFlags` 0x410, the
first dead cmd's (30050) `weapon` 0 and still 0x410, the second's 0x10.
Through dm's own callback, whose `dropItem` takes the carbine and its clip
in the death's frame (the 25950 snapshot already lacks clip slot 10), the
first dead cmd's snapshot read 0x10: the ammo test fails. `rise_ab` holds
ours to both.

VERIFIED live: the same probe and client on `COD_VERSION=1.5`
and `COD_VERSION=uo tools/run_probe.sh mp_carentan` (fixtures
`mp_carentan-dm-slope-50ms-rise-1.5.txt` and `-rise-uo.txt`, same
directory) read the same on both deaths: the keeping body's death snapshot
and its first dead cmd's carry 0x400 and the second's clears it, and the
stock callback's first dead cmd already reads 0x10. `rise_ab`'s 1.5 and UO
rows hold ours to both.

---

## 17. What a hit, a quake and a lean do to the drawn view

The client's side of section 6's kick, the `earthquake` builtin and the
lean's own term in the viewmodel. Module `cgame_mp_x86.dll` unless named.

### 17.1 The damage kick on the view

VERIFIED, `CG_OffsetFirstPersonView` (vcod's name, `0x30032ae0`) at
`0x30032af7`-`0x30032b54` builds a frame of `{ps 0x3020715c, [0x3020c9a8]
- [0x30209228] or 0 when [0x3020c9a8] is 0, [0x30207148] - [0x30209228],
[0x3020c9e4], [0x3020c9e8], [0x3020c9f0]}` on a zeroed output and calls
`0x30012cb0` (`0x30032b58`) and `0x30012dc0` (`0x30032b65`); `0x30032b6a`-
`0x30032b9c` add the output to `refdefViewAngles` (`0x302095cc`-`0x302095d4`).
VERIFIED, the function returns at `0x30032af1` when the snapshot's
`pm_type` (`[0x301e2164] + 0x10`) is 5.

VERIFIED, `0x30012cb0`, the constants `0x30069304` 1.0, `0x30069308` 2.0,
`0x3006930c` 0.5, `0x30069310` 0.0, `0x30069524` 100.0, `0x30069520`
0.0025 and `0x300693f4` 0.01, and the reads `ps+0xb8` (`fWeaponPosFrac`)
and `weaponDefs[ps+0xb0]+0x228` (`adsOverlayReticle`). INFERRED, off its
branches: it is the game's `0x3a2d4` (15.4) with the rise as `dt * 0.01`
and the fall as `(dt - 100) * 0.0025`; nothing on a zero stamp, `f = 1 -
frac * 0.5`, times `frac * 0.5 + 1` with the fraction non-zero on a scope,
`out[0] += g * f * [0x3020c9e4]`, `out[2] += g * f * [0x3020c9e8]`. The
game tests `frac > 0` where the cgame tests `frac != 0`; the fraction is
never negative. INFERRED: `0x30012dc0` is the game's `0x3a3c4`, the sight's
own walk bob; it reads `eFlags & 0xc000`, `ps+0xb8`, `adsViewBobMult`
(`+0x238`) and `bobCycle` (`ps+8`) as 15.4 lists. The two kick angles and
the stamp are `0x300287f0`'s (`cod11-hud-protocol.md`, "Scope overlay"),
the same the gun's kick reads, so the view and the gun kick off one hit.

INFERRED: the kick's pitch tips the view along the hit and its roll across
it; it is never added to a cmd, so it moves the picture and not the aim
(the server keeps its own copy, section 6 step 7 and 8, for the aim block).

vcod: `pmove::aim::view_angles`, which the server's aim block and the play
camera share. `Hud::view_kick` feeds a new `damageEvent` into the gun's
tracker first, then evaluates on the playerstate the HUD draws (the replay
at its `commandTime`, a followed player's snapshot at the render clock),
and the camera adds the result before the lean offset, skipping the
intermission. A followed player's view kicks too: `CG_OffsetFirstPersonView`
runs on whichever playerstate is drawn.

### 17.2 The earthquake

VERIFIED, game.mp `0x5f3d8` (`functions[76]`, `earthquake`):
`Scr_GetFloat(0)` (scale), `Scr_GetFloat(1) * 1000.0 + 0.5` (`.rodata
0x78b04`, `0x78b08`) stored through `fistp` with the control word's
rounding set to truncate (`0x5f40c`-`0x5f41e`), `Scr_GetVector(2)`,
`Scr_GetFloat(3)` (radius), then `G_TempEntity(origin, 0xc3)` with
`s+0x68` (`angles2[0]`) the scale, `s+0x54` (`time`) the ms and `s+0x6c`
(`angles2[1]`) the radius. Nothing is range-checked. VERIFIED, `EV_EARTHQUAKE`
is 195 (`cod11-events-and-fx.md`).

VERIFIED, live: `probe_quake` (client-probes) on the retail
1.1d server, dm on mp_carentan, one `--net-probe --probe-team allies`
client; `earthquake(0.3, 2.5, origin, 850)` arrives as `angles2 [0.3,
850.0, 0.0] time 2500`, `earthquake(0.05, 0.0026, origin, 100)` as
`angles2 [0.05, 100.0, 0.0] time 3`, origins truncated (`144.12` reads
144). The run on `vcod-server` reads the same, entity numbers included.

VERIFIED, `CG_EntityPreEvent` has two calls to `0x30017dd0(scale, ms,
radius)` with `ecx` the entity's `lerpOrigin` (`cent+0x1f8`): case `0xc3`
(`EV_EARTHQUAKE`, `0x3001e99f`) passes `cent+0x158`, `+0x144` and `+0x15c`
(`currentState` at `cent+0xf0`, so `angles2[0]`, `time` and `angles2[1]`),
and case `0xa8` (`EV_FIRE_WEAPON_MG42`, `0x3001e936`) passes 0.05
(`0x3d4ccccd`), 100 and 100.0 (`0x42c80000`) before falling into the
fire-weapon arm. A mounted MG shakes the view a little within 100 units
of the gun on every shot.

VERIFIED, the slots: four of 0x24 bytes at `0x3020cff4` (start time, scale,
duration as a float, radius, origin, then two computed floats at `+0x1c`
and `+0x20`), and the phase at `0x3020d084`. `0x30017d40` (slot in `esi`):
`dt = cg.time - start`, 0 when negative or not under the duration;
otherwise `d = 1 - Distance(refdef.vieworg, origin) / radius` and `s =
scale * (1 - dt / duration)`, `+0x20 = s`, `+0x1c = d * s`, or `d / s`
when `d` is negative (`0x30017db2`), and it returns 1. `0x30017dd0`
returns at once for a scale that is not above 0 (`0x30017dd7`), fills a
slot on the stack at `cg.time` and runs `0x30017d40` on it, then takes the
first slot whose start is past `cg.time` or whose start plus duration is
not, else the slot with the smallest `+0x1c` below the new one's, else
drops it (`0x30017e30`-`0x30017eeb`).

VERIFIED, `0x30017f00`, constants `0x300694e0` 1/600, `0x300694dc` 8 PI,
`0x300694d4` 15 PI, `0x300694d0` 12 PI, `0x300694d8` 18.0, `0x30069468`
16.0, `0x300693e4` 10.0, `0x300693b4` 1/32768, `0x300693a8` PI: `t =
cg.time * (1/600)` as a float; the live slot with the largest `+0x1c` above
0 gives `c` and its `+0x20` gives `s`; with `c` not above 0 the phase is
redrawn as `(rand() / 32768 * 2 - 1) * PI` and nothing shakes; otherwise
`c` is capped at 1 and `refdefViewAngles` take `sin(t * 8PI + phase) * c *
s * 18` on pitch, `sin(t * 15PI + phase) * c * s * 16` on yaw and `sin(t *
12PI + phase) * c * s * 10` on roll, periods of 150, 80 and 100 ms.
VERIFIED, a floor at `0x3020d088` would replace `c` and `s` both, but its
only store is a zero (`0x3001bdf1`, `ebp` cleared at `0x3001bd1d`).
INFERRED: the amplitude goes as the scale squared; `earthquake(0.3, ...)`
at its source starts at 1.6 degrees of pitch, the MG's at most 0.045.

VERIFIED, the call site `0x30033da9`: after `CG_CalcViewValues` (`0x30033d9b`,
which runs `CG_OffsetFirstPersonView`), behind a test of `0x301e2150` being
0, then `AnglesToAxis` rebuilds `refdef.viewaxis` (`0x30033db8`). INFERRED:
`0x301e2150` is the stereo or view-mode argument `CG_DrawActiveFrame`
stores (`0x300339ce`), 0 in normal play; the shake applies to every view,
the intermission and a turret's included, and the viewmodel, drawn off
the shaken axis, moves with it.

VERIFIED, where the pre-event runs: `0x3002fc80` ends by calling
`0x3001eb10` once for every entity of the new snapshot (`0x300301ee`), and
`0x3001eb10` walks the entity's event ring into `CG_EntityPreEvent`
(`0x3001e820`, `0x3001eb48`, `0x3001ebdb`). `0x3002fc80` is called from
`0x300302c0` (`0x30030385`), which also calls `0x30030210`, the function
holding the `CG_ReadNextSnapshot: way out of range` string; `0x300302c0` is
called at `0x30033c14`, ahead of `CG_CalcViewValues` (`0x30033d9b`) and the
shake (`0x30033da9`). INFERRED: `0x300302c0` is `CG_ProcessSnapshots`, so a
quake starts on the frame its snapshot is read and shakes that frame's
view. Its strength at the start is measured from `refdef.vieworg` as the
last frame left it, since `CG_CalcViewValues` has not run yet, and a full
table compares against each slot's `+0x1c` as the last shake (or the
slot's own start) stored it.

vcod: `play::quake`, owned by `play::view::OnlineView`. The frame collects
the snapshot's and the prediction's events before it builds the view and
starts each quake there, at the render clock and the event body's drawn
origin; `Quakes` keeps the last shake's eye and each slot's stored
strength for `start`. The camera adds the shake after the turret override.
Difference: the phase's `rand()` is an msvcrt-shaped LCG with its own
seed. The server's `earthquake` builtin is
`game::builtins::fx::earthquake`.

### 17.3 The lean in the viewmodel

VERIFIED, `0x30036990` (output in `eax`, called at `0x300372c1` by the
viewmodel's `0x300371f0`), constants `0x30069674` 1.6, `0x30069678` -2.0,
`0x30069390` PI/180, `0x300693f0` -1.0: with the predicted `leanf`
(`0x3020719c`, `ps+0x40`) non-zero and `fWeaponPosFrac` (`0x30207214`)
under 1, `f = (2 - |leanf|) * leanf`, `d = (1 - frac) * f * 1.6`, and the
output takes `d` times `AngleVectors`' right vector of `(0, 0, f * -2)`,
which is `(0, -cos r, -sin r)`. VERIFIED, `0x30036840` turns the output
into the world as `vieworg + x * forward - y * right + z * up` off
`refdefViewAngles`, so the frame is (forward, left, up). INFERRED: a full
lean right moves the gun 1.6 units right and 0.056 up relative to the eye,
on top of the eye's own 19.2 (`bsp-ibsp59-format.md`, "Lean"), and none
once the sight is fully up.

VERIFIED, the gun's angles: `0x30012bf0` (15.5) opens with `out[2] =
GetLeanFraction(leanf) * -2.0` (`0x30012c01`-`0x30012c37`), the game's
first term in 15.2, and `0x300371f0` composes the gun's axis from those
angles and the view's (`0x30037480`-`0x300374aa`). INFERRED: the gun rolls
2 degrees against the lean at full lean, sighted or not.

vcod: `play::view::lean_offset` in `OnlineView::frame`'s position; the
roll comes with the rest of the gun's angles (17.4).

### 17.4 The view weapon's pose

Module `cgame_mp_x86.dll`. VERIFIED, `0x300371f0` (called once, at
`0x30033e04`, after the shake) returns without drawing when the
predicted playerstate's `pm_type` (`ps+4`) is 4 or 5 (`0x30037213`-
`0x30037228`), when `[0x30207158]` or `[0x3020cad4]` is non-zero
(`0x3003722e`, `0x3003725e`), or under `eFlags & 0xc000` (`ps+0x80`,
`0x3003726b`).

VERIFIED, `[0x30207158]`: its only stores are at `0x30033d3d` and
`0x30033d49` in `CG_DrawActiveFrame`, after `0x3002f7d0` and before
`CG_KickAngles`: 1 when `[0x301d884c]` is non-zero, else 1 when the
snapshot's `pm_type` (`[0x301e2164] + 0x10`) is 6 or more, else 0.
VERIFIED, `0x301d884c` is the `integer` of the vmCvar at `0x301d8840`,
which the cvar table row at `0x30074eb0` names `cg_thirdPerson`, default
`"0"`. VERIFIED, `PM_DEAD` is 6 (`net::flags`). INFERRED: this is Q3's
`cg.renderingThirdPerson`, so a dead player's gun does not step or draw,
and the sway and eased states hold still until the respawn.
VERIFIED, `[0x3020cad4]`: the read at `0x3003725e` is the only instruction
in the module that names it, and the cgame holds no `testgun` string.
It sits just before `0x3020cad8`, a 64-byte name that `0x30032360` copies
from a command argument (`0x3004b5e0`, 0x3f bytes) and hands syscall
`0xde`, and that `0x30033930` hands `0xde` again on a timer.
INFERRED: Q3's `cg.testGun` next to `cg.testModelName`, kept after the
command that set it was removed; it is always 0 and vcod does not model
it.

VERIFIED, `cg_drawGun`: after the gates, `0x300371f0` sets a draw flag
to 1, then to 0 when `[0x301dbaec]` is not 2 and is either 0 or
`0x30015f20` returns non-zero; the vmCvar at `0x301dbae0` is the table
row at `0x300749b0`, `cg_drawGun`, default `"1"`, flags `0x201`.
`0x30015f20` returns 1 when the weapon has an `adsOverlayShader` or
`adsOverlayReticle` (`+0x224`, `+0x228`) and `fWeaponPosFrac` is non-zero
and `frac - (1 - adsZoomInFrac)` (`+0x220`, or `adsZoomOutFrac` `+0x21c`
under `[0x30209458]`), divided by that zoom fraction when positive, is at least
0.01 (`0x300693f4`). The flag reaches `0x30036cf0` as its last argument,
which gates the `trap_R_AddRefEntityToScene` call (syscall `0x3d`) and the
`tag_flash` flash. INFERRED: everything above still runs; `cg_drawGun 0`
hides the gun, 2 keeps it under a scope overlay, and the pose, the scope
centre and the clips run on either way. With no weapon (`ps+0xb0` not above 0, `0x30037282`) it only
copies `refdefViewAngles` into `0x3020cb80`/`0x3020cb84` and zeroes
`0x3020cb88`-`0x3020cb90` (`0x300375cb`). INFERRED: nothing below runs
on a mounted gun, so the sway's last view and the eased states hold still
until the player dismounts.

VERIFIED, the order with a weapon: `0x300361f0` (the turn sway, 15.3, with
the shellshock scale), `0x30036bd0`, `0x30036990` (the position, into the
`refEntity_t`'s origin at `esp+0xbc`), then the `cg_gunX`/`Y`/`Z` adds,
`AnglesToAxis(refdefViewAngles)` (`0x30037399`), `0x30012bf0`
(`BG_CalculateWeaponAngles`, 15.2 and 15.5) on a frame built at
`0x3003739e`-`0x3003747c`, `AnglesToAxis` of its output (`0x30037490`)
and `MatrixMultiply(weaponAxis, viewAxis)` into the entity's axis
(`0x300374aa`), the scope's pair (15.5), and the draw (`0x30036cf0`).

VERIFIED, the sway's two outputs: `0x300361f0` calls `0x30012ef0` with
`esi` the predicted playerstate (`0x3020715c`), `0x302095d8` the last view,
`edi` `0x302095e4`, `eax` `0x302095f0`, the scale and `cg.frametime`
(`0x30207144`) as the msec. `0x30012ef0` writes its two
`AngleNormalize180` results through `edi` (`0x3001332e`-`0x3001333b`) and
the two lerped offsets through `eax`, copied to `ebx` (`0x3001316f`,
`0x300131d7`). INFERRED: the cgame keeps the angle lag at `0x302095e4`
and the position lag at `0x302095f0`, the reverse of the game's
`client+0x2284`/`+0x2290` order.

VERIFIED, the frame `0x30012bf0` reads (`esi` at `esp+0x20`): the
playerstate, the speed (`0x3020c9f0`), `cg.frametime * 0.001`
(`0x300693c0`), the tilt state `0x30209478`-`0x30209480`, the idle factor
`0x30209468`, `cg.time - [0x30209228]`, 17.1's stamp and two kick angles,
the gun-kick spring `0x3020cb94`-`0x3020cba8`, and the angle lag
`0x302095e4`-`0x302095ec`. `0x30037529`-`0x30037585` copy the frame's
states back to the same globals. INFERRED: the view weapon and the scope
draw off one step of the same state each frame.

VERIFIED, `0x30036990`'s position, built in the view's (forward, left, up)
frame (17.3) and then turned into the world by `0x30036840`
(`0x30036b36`): the output is zeroed (`0x30036993`-`0x3003699e`), takes
17.3's lean term, then `0x300367d0` adds `0x300362c0`'s movement term
(and copies it to `0x3020945c`), then `out[1] -= [0x302095f4]` and `out[2]
+= [0x302095f8]` (`0x30036b1d`-`0x30036b33`), the position lag's yaw and
pitch terms of 15.3.

VERIFIED, `0x300362c0`, the movement term, off the weapon file at
`[0x30209484]` with the field names from the cgame's weapon field table
(`0x30075828`) and the cvar names from the cvar table: the minimum speed
is `cg_gun_move_minspeed` (`0x301da588`) plus `proneMoveMinSpeed`
(`0x16c`) under `[0x30209234] & 0x40`, `duckedMoveMinSpeed` (`0x168`)
under `& 0x20`, `standMoveMinSpeed` (`0x164`) otherwise. With the speed
(`0x3020c9f0`) above it and `weaponstate` (`0x30207210`) not 5,
`f = (speed - min) / (ps->speed (0x302071a0) - min)` clamped to 0..1 and
the target is `f` times `proneMoveF/R/U` (`0x144`), `duckedMove*`
(`0x120`) or `standMove*` (`0xfc`), plus `f` times `cg_gun_move_f/r/u`
(`0x301e1188`, `0x301e0e28`, `0x30298ea8`); otherwise zero. With
`viewHeightTarget` (`0x30207228`) equal to `0x3020749c` the target adds
`duckedOfsF/R/U` (`0x114`), equal to `0x30207498` `proneOfsF/R/U`
(`0x138`), either one then `cg_gun_ofs_f/r/u` (`0x302997a8`,
`0x301e00a8`, `0x301da6a8`). Each axis of `0x3020946c`-`0x30209474`
moves toward its target by `(target - cur) * rate * cg.frametime * 0.001`
with `rate` `cg_gun_move_rate` (`0x301de7e8`) plus `posProneMoveRate`
(`0x160`) when `viewHeightCurrent` (`0x3020722c`) equals `0x30207498` and
`posMoveRate` (`0x15c`) otherwise, the step floored at `cg.frametime *
0.0001` (`0x300693bc`, `0x300693b8` its negative) toward the target and
clamped at it. The state is added whole at an `fWeaponPosFrac`
(`0x30207214`) of 0, times `1 - 2 * frac` below 0.5 (`0x3006930c`), and
not at all above. Every `cg_gun_*` cvar defaults to `"0"`. INFERRED:
`[0x30209234]` is the predicted entity's `eFlags` (`0x3020922c` is the
entity `0x30036cf0` draws, `entityState.eFlags` at `+8`), and
`0x3020749c`/`0x30207498` are the playerstate's crouch and prone view
heights (`ps+0x340`, `ps+0x33c`), so this is 15.2's tilt term moved onto
the position with its own keys, and the stance offsets hold even standing
still. On the kar98k a run eases the gun 0.6 units left and 1 down at 4 a
second, crouched it sits 2 back, 1 left and 0.2 down, and a prone crawl
pulls it 120 back and 80 down, out of the picture.

VERIFIED, after the world transform: `cg_gunX`, `cg_gunY` and `cg_gunZ`
(`0x301db548`, `0x301ad2a8`, `0x301d9808`, all `"0"`) add along
`0x302095a0`, `0x302095ac` and `0x302095b8` (`0x300372c6`-`0x30037390`),
and `0x30036900` (`0x30036b7a`, `0x30036bbc`) stores `(origin -
vieworg) * fWeaponPosFrac` into `0x3020cb88`-`0x3020cb90` for an
`aimDownSight` weapon with the fraction non-zero, zero otherwise. 17.5
has the landing term between the two.

vcod: `pmove::aim::GunMove` is `0x300362c0` (`PlayerState::speed`, the
wire's `ps->speed`, as in the tilt; no `cg_gun_*` cvars). `hud::scope::GunAim`
steps the sway, the angles and the movement term once a frame, called by
`Hud::step_gun` after the view is final and before the view weapon is
posed (not on pm_type 4 or 5, dead or on a mounted gun; vcod has no
`cg_thirdPerson`), and returns a `GunPose`; `OnlineView::frame` adds the lean, the landing's quarter and
draws the model with `AnglesToAxis(angles)` as its axes in the view's
frame. The scope overlay centres on the same step's angles. `--walk`
draws off `GunAim` too, without lean or landing. `OnlineView::draw_gun`
carries `cg_drawGun`.

### 17.5 The landing dip

VERIFIED, `CG_EntityEvent` (`0x3001dc10`) clamps `es.clientNum` (`es+0x90`,
the entityState field table's `clientNum` at offset 144) to 0..63
(`0x3001dc87`-`0x3001dc96`) and reads `eventParm` (`es+0xa0`, offset 160).
For `EV_LANDING_*` (93 to 115, `0x3001dd79`-`0x3001dd81`) with that client
the playerstate's (`0x30207208`, `0x3001dd9b`), it sets `0x302094e8 =
cg.time` and `0x302094e4 = -eventParm` (`0x3001dda3`-`0x3001ddb3`). For
`EV_LANDING_PAIN_*` (116 to 138), with the same test, `h =
bg_fallDamageMinHeight + (bg_fallDamageMaxHeight - Min) * eventParm *
0.01` (`0x3019d8e8`, `0x3019d2a8`, `0x300693f4`); nothing unless `h > 12`
(`0x300693fc`), then `n = ftol(((h - 12) * (1/26) + 1) * 4)`
(`0x300696c4`, `0x300693dc`), capped at 24, nothing unless above 0, and
the same two stores with `-n` (`0x3001de03`-`0x3001de7c`). INFERRED: the
pain arm turns the damage percent back into the fall height and runs the
pmove's own damage-free curve on it (`(h - 12) / 26 * 4 + 4`, at most 24,
`pmove::landing_event`), so both landings dip alike.

VERIFIED, the eye: `CG_OffsetFirstPersonView` at `0x30032ca1`-`0x30032db3`,
after the kick and before `0x30032860` and `AddLeanToPosition`: `dt =
cg.time - [0x302094e8]`; with `dt` negative the stamp becomes `cg.time -
450` (`0x30032d4b`); with `dt` under 150 (`0x30069670`) `vieworg[2] +=
change * dt / 150`, under 450 (`0x30069684`) `vieworg[2] += change * (1 -
(dt - 150) / 300)` (`0x3006966c`). VERIFIED, the gun: `0x30036b3b`-
`0x30036bb7`, after the world transform, adds `dt * change * 0.25 / 150`
(`0x3006950c`) under 150 and `(450 - dt) * change * 0.25 / 300` under 450
to the origin's world z. INFERRED: the eye sinks by the change over 150
ms and comes back over 300, and the gun sinks a quarter as much again,
along world up.

vcod: `play::view::land_change`, `OnlineView::land` off the frame's event
pass (17.2) for events whose `client_num` is the drawn playerstate's, and
`OnlineView::land_dip` on the camera before the lean, skipped at the
intermission; `OnlineView::frame` adds a quarter of the dip along world
up in the view's frame.
