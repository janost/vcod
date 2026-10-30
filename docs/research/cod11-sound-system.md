# CoD 1.1 sound system: aliases, channels, falloff, events, wire path

Everything below is read out of the CoD 1.1 binaries and stock assets. Each
claim names the module, the virtual address and the string, table or constant
it rests on. `VERIFIED` here means the behaviour was read directly from the
decompilation or disassembly of the code path that runs it (static
verification); `inferred` means it is deduced from surrounding code or lineage
and the next reader should test it. The events-and-fx doc reserves `VERIFIED`
for live observation. Section 9's wire path has been checked live
(2026-08-25; see "Live verification" at the end of that section).
Everything else here is static verification only.

## Read this first

- The engine plays sound through **Miles Sound System** (`_AIL_*` imports in
  `CoDMP.exe`). Positional sounds are Miles 3D samples, but the engine sets the
  Miles min and max distance to the same value (`AIL_set_3D_sample_distances(s,
  dist_max, dist_max)`), so Miles contributes only panning. The distance falloff
  is computed by the engine itself and it is **linear in amplitude** between
  `dist_min` and `dist_max` (section 3).
- `dist_max` blank or `0` does **not** mean "no cutoff": the loader replaces it
  with `dist_min * 5` (section 1). The csv legend is wrong about this.
- There is no own-entity rule in the engine: which sounds are unspatialized is
  decided by channel alone (`menu`, `local`, `music`, `announcer`, `shellshock`
  are 2D; everything else is 3D). The followed player's sounds are 3D sounds
  that happen to sit at the listener (section 5).
- Alias handles are name-string pointers; an alias name that does not exist
  registers as `NULL` and every later play of it is silent. There is no
  `_default` surface fallback in the client (section 7).
- `sequence` is a sort key, not a play order. Rows with the same name are picked
  by `probability` weight, and with three or more rows the variant played last
  is excluded from the next pick (section 1).

### Evidence sources

| File | md5 | Used for |
|---|---|---|
| `CoDMP.exe` (1.1 install) | `753fbcabd0fdda7f7dad3dbb29c3c008` | alias csv loader, pick, channel table, Miles playback, falloff, master/slave, listener |
| `cgame_mp_x86.dll` (1.1 install) | `4912169a9eb22b404f95c52863a5feb6` | per-event alias selection, whizby, weapon sound keys, server-command handlers, loop sounds |
| `game.mp.i386.so` (the 1.1d Linux dedicated server's game module) | `de8947beb6f86fbfb46f5adfaab3d3ed` | gsc `ambientPlay`/`playSound`/`playLoopSound`/`playLocalSound`/`musicPlay` handlers, `G_PlaySoundAlias*`, `G_SoundAliasIndex`, `G_Voice`. Has a symbol table |
| `pak1.pk3 : soundaliases/*.csv` | none | alias data, csv legend |
| `pak5.pk3 : maps/MP/gametypes/sd.gsc`, `pak4.pk3 : maps/MP/mp_carentan.gsc` | none | which gsc sound calls the stock MP scripts actually make |
| CoDExtended `src/shared.h`, `server.h` | none | `entityType_t`, `SVF_*` names |

Image bases: `0x30000000` cgame DLL, `0x00400000` `CoDMP.exe`, file-relative
for the Linux `.so`. The decompilations are Ghidra's (12, headless, exported
with `tools/re/ExportDecomp.java`) of `CoDMP.exe` and `cgame_mp_x86.dll`, plus
the disassembly of each. Where the decompiler lost register-passed arguments
(`__fastcall`/`__thiscall` with `in_EAX`), I read the disassembly instead and
cite it by instruction address.

The cgame reaches the engine through the syscall trampoline `DAT_30074898`;
the dispatcher is `CL_CgameSystemCalls` @ `CoDMP.exe 0x401df0`. Sound syscalls
used below (case numbers in that dispatcher, engine function in parentheses):

| Syscall | Engine function | Meaning |
|---|---|---|
| `0xbe` | inlined alias loader (same code as `0x433e90`) | load `soundaliases/*.csv` for a map name |
| `0xbf` | `0x432a80` | register alias by name, returns the shared name pointer (handle) or 0 |
| `0xc0` | `0x434160` | pick one row of an alias by name (`trap_Com_PickSoundAlias`) |
| `0xc1` | `0x434260` | alias by 1-based index into the map set |
| `0xc2` | `0x44d7d0` | start sound: `(record, entnum, origin, 0)`, returns length in ms |
| `0xc3` | `0x44d980` | start a blend of two alias records (shellshock loop, `eType 9` entities); United Offensive's counterpart, syscall 199, is `coduo-diff.md` 16 |
| `0xd0` | stores `0x143a988..990` | a listener position read by `FUN_00414ef0` @ `0x414ef0` (six-ray LOS test in the snapshot encoders, section 11c) |
| `0xd1` | `0x44bd60` | `S_Respatialize(clientNum, origin, axis)` |
| `0xd2` | `0x44e600` | end-of-frame: kill looping voices not refreshed this frame |
| `0xd3` | `0x44fa40` | stop all sounds |
| `0xd4`, `0xd5` | `0x44dc40`, `0x44db90` | music play / music stop with fade |
| `0xd6` | `0x44dc80` | ambient play with crossfade |
| `0xd7` | `0x44e0f0` | master fade (`soundFade`, section 10) |

---

## 1. Alias csv semantics as the engine parses them

### 1a. Loading

`VERIFIED` (decompilation). The map-time loader runs from the cgame syscall
`0xbe` (`CL_CgameSystemCalls` @ `0x401df0`, `case 0xbe`), which is an inlined
copy of `FUN_00433e90` @ `0x433e90`. It takes the map name, strips a leading
`maps/` and a trailing `.bsp` (`__strnicmp(.., "maps/", 5)`,
`__stricmp(.. - 4, ".bsp")`), lowercases it (`__strlwr`), then lists
`soundaliases/` through the filesystem (`FUN_0042b2a0("soundaliases")`, the
`FS_ListFiles` equivalent) and parses **every file in that directory across all
paks** with `FUN_004333d0` @ `0x4333d0` (`va("soundaliases/%s", name)` at
`0x402920`, format string `0x563304`). Parsed rows are collected in a linked
list, merge-sorted by `FUN_00433750` @ `0x433750`, packed into 68-byte records
by `FUN_00433900` @ `0x433900` (prints `"Sound alias strings use %.1f KB; %.1f
KB saved by string sharing\n"`), and their files opened by `FUN_00433c20` @
`0x433c20`. Two alias sets exist, index 0 (menu, loaded once with the name
`"menu"` from `0x418f18`) and index 1 (the map); the array base per set is
`(&DAT_00893dd8)[set]`, count `DAT_00893de4[set]`, 256 hash buckets per set at
`DAT_008931d8`.

The consequence for vcod is that the effective alias table for a map is the union of every
`soundaliases/*.csv` in every pak, filtered by `loadspec` against the lowercased
map name, then sorted. Later paks do not "override" earlier ones; a duplicate
name across files simply adds rows to that name's group (section 1e).

### 1b. Columns

`VERIFIED`. The header row is bound by name, case-insensitively
(`FUN_0044aa90`, a `stricmp`), against the 16-entry table at
`CoDMP.exe 0x57aca8` (index 1..16): `name, sequence, file, subtitle, vol_min,
vol_max, pitch_min, pitch_max, dist_min, dist_max, channel, type, loop,
probability, loadspec, masterslave`. Unknown column names are ignored; up to
256 columns. `name` and `file` columns are required (`"Sound alias file %s:
missing 'name' and/or 'file' columns\n"` @ `0x563534`). Data rows whose first
cell is empty or starts with `#` are skipped (`FUN_004333d0` tests the row's
first byte for `#`); a row with either name or file empty is rejected
(`0x5634f8`). Cells are parsed by `FUN_00432eb0` @ `0x432eb0`, **and only
non-empty cells reach it** (`FUN_004333d0` tests the cell's first byte before
the call), so a blank cell always leaves the per-row default in place. A column
appearing twice in one row is an error (`0x563774`).

### 1c. Defaults and validation

`VERIFIED`. Per-row defaults are written by `FUN_00432ad0` @ `0x432ad0` into
the parse struct (offsets in that struct in parentheses):

| Column | Default | Struct |
|---|---|---|
| `vol_min` | `1.0` | `+0x10c8` |
| `vol_max` | `1.0`, **overwritten by `vol_min` when that cell is parsed and the `vol_max` column has not been seen in this row** (`FUN_00432eb0` case 5 checks the column-seen flag at `+6`) | `+0x10cc` |
| `pitch_min` | `1.0` | `+0x10d0` |
| `pitch_max` | `1.0`, same rule as `vol_max` (case 7, flag `+8`) | `+0x10d4` |
| `dist_min` | `120.0` (written as the two bytes `0xf0` at `+0x10da` and `0x42` at `+0x10db`, i.e. `0x42f00000`) | `+0x10d8` |
| `dist_max` | `0.0`, then fixed up, see below | `+0x10dc` |
| `channel` | `0` = `auto` | `+0x10e0` |
| `type` | `1` = `loaded` | `+0x10e4` |
| `loop` | `0` = `nonlooping` | `+0x10e8` |
| `probability` | `1.0` | `+0x10f0` |
| `masterslave` | neither (`master` byte `+0x10e9` = 0, `slave` byte `+0x10ea` = 0, slave level `+0x10ec` = `1.0`) | |
| `loadspec` | flag = the loader's 4th argument, which the map-time path passes as `1` (`push 1` @ `0x40291b` before `call 0x4333d0` @ `0x402938`): **blank loadspec = row is loaded** | `+0x10f4` |
| `sequence` | `0` | `+0x1080` |
| `subtitle` | empty | `+0x80` |

After the last cell of a row, `FUN_00433190` @ `0x433190` fixes it up, in this
order:

1. if `pitch_max < pitch_min` swap them; warn if `pitch_min <= 0`
   (`0x5635f0`);
2. if `vol_max < vol_min` swap them; warn if `vol_min < 0` (`0x5635cc`);
3. **if `dist_max == 0.0` then `dist_max = dist_min * 5.0`** (constant
   `_DAT_005690f4 = 5.0`);
4. warn if `dist_max < dist_min` (`"sound alias '%s' has dist_min %g <=
   dist_max %g\n"` @ `0x563598`, text is backwards but the test is `max <
   min`); the values are **not** changed;
5. warn if `dist_min <= 0` (`0x563570`).

So a row with `dist_min` blank and `dist_max` blank plays full volume out to
120 units, fades linearly to silence at 600, and is not started at all beyond
600 (section 3). `MP_announcer_*` rows use `dist_min = 1000000` for this
reason.

Keyword parsers, all `stricmp`: `channel` (`FUN_00432b80` @ `0x432b80`) walks
the 10-entry table at `0x57acec` (section 2) and errors on anything else
(`"Unknown sound channel '%s'; should be %s"` @ `0x5638d0`, the list built from
the table itself). `type` (`FUN_00432c70` @ `0x432c70`): `"streamed"`
(`0x5638c0`) gives `2`, `"loaded"` (`0x5638b8`) gives `1`, else error
(`0x563868`). `loop` (`FUN_00432cd0` @ `0x432cd0`): `"looping"` (`0x56385c`)
gives `1`, `"nonlooping"` (`0x563850`) gives `0`, else error (`0x5637f8`).
`masterslave` (`FUN_00432e70` @ `0x432e70`): `"master"` sets the master byte;
**any other non-empty text** sets the slave byte and stores `atof(cell)` as the
slave level (section 4). Alias names must start with a letter or `_` and
contain only alphanumerics and `_` (`FUN_00432a20` @ `0x432a20`, warning
`0x5636fc`), max 63 chars; file names max 63; subtitles max 4095, 7-bit only.

### 1d. `loadspec`

`VERIFIED` from `FUN_00432d20` @ `0x432d20` (this-pointer = the lowercased map
name, argument = the cell). The cell is copied, lowercased, then:

- **Cell starts with `!`**. Skip the `!` and every following character `<= ' '`
  (so `! Airfield` and `!airfield` are the same), then compare the remainder
  with the map name for exact equality. If equal, the row is **excluded**. If
  not equal, the row is **included**, unless the remainder is exactly `all_mp`
  (7-byte compare against `0x5637bc`), in which case it is excluded. `!all_sp`
  therefore includes in MP, `!all_mp` excludes.
- **No `!`**. Search the cell for the map name as a whole word (every
  occurrence found by `FUN_00525760`, a `strstr`, is accepted only if the
  preceding character is the cell start or `< '!'` and the following character
  is `< '!'`). If found, the row is **included**. If not found, it is included
  only if the cell is exactly `all_mp` (7-byte compare from the last search
  position; with no partial matches that is the cell start). Everything else
  (`all_sp`, other map names, `menu`) is **excluded**.
- **Blank**. Never reaches this function; the default flag applies (included on
  the map path, section 1c).

Rows whose flag ends up false are dropped at parse time. `FUN_004333d0` tests
parse struct `+0x10f4` and only when it is set calls `FUN_00433190` (the
fix-up) and `FUN_004332f0` (append to the row list). Retail usage counts in `iw_sound.csv`
(1936 rows): blank 708 (plus 191 rows with the column missing), `!Truckride`
429, `Truckride` 429, `all_mp` 31, `all_sp` 17, `! burnville` 17, per-map names
for the rest. The `all_mp` exact-match rule means a cell like `all_mp
mp_carentan` would only load on `mp_carentan` (inferred edge case, no retail row
does this).

The menu set is loaded with the name `menu` (`FUN_00433e90(&"menu", 0)` from
`0x418f18`); which default the blank flag takes on that path was not pinned
(the call at `0x433fcb` passes a stack slot, `[esp+0x60]`, that the decompiler
did not name). vcod only needs the map set.

### 1e. Multiple rows per name, `sequence`, the pick

`VERIFIED`. `FUN_00433750` @ `0x433750` merge-sorts the row list by
(`name` via `stricmp`, then `sequence` numerically, then `file` via
`stricmp`); three-way ties warn `"sound alias file %s: duplicate alias '%s'\n"`
(`0x5634cc`). `sequence` is **not stored in the runtime record**
(`FUN_00433320` @ `0x433320` copies every field except `+0x1080`); it only
orders rows within a name group and catches duplicates, exactly as the csv
legend says. During packing, `FUN_00433900` shares one name string between
consecutive rows with equal names, and the pick relies on that pointer
equality.

Runtime record, 68 bytes (`0x44`), written by `FUN_00433320`:

| Offset | Field |
|---|---|
| `+0x00` | name (shared `char*`, the alias handle) |
| `+0x04` | file (`char*`) |
| `+0x08` | subtitle (`char*` or 0) |
| `+0x0c` | loaded sound handle (set by `FUN_00433c20` via `FUN_0044fc10`) |
| `+0x10` | pick counter (see below) |
| `+0x14` / `+0x18` | `vol_min` / `vol_max` |
| `+0x1c` / `+0x20` | `pitch_min` / `pitch_max` |
| `+0x24` / `+0x28` | `dist_min` / `dist_max` |
| `+0x2c` | channel index |
| `+0x30` | type: 1 loaded, 2 streamed |
| `+0x34` | loop byte |
| `+0x35` / `+0x36` | master byte / slave byte |
| `+0x37` | streamed file exists (set by `FUN_00433c20`) |
| `+0x38` | slave level (float) |
| `+0x3c` | probability (float) |
| `+0x40` | hash chain next |

Lookup by name is `FUN_00432a80` @ `0x432a80` (syscall `0xbf`): hash
`FUN_004329e0` (`h = h * 105 + tolower(c)` in 8 bits), walk the bucket chain
with `stricmp`; chains are LIFO so the record found is the **last** row of the
name group in sorted order. It returns `record->name`, and that pointer is what
the cgame stores as the alias "handle". A missing alias returns `0`; every play
through a `0` handle is a no-op (`FUN_30021bf0` @ `cgame 0x30021bf0` returns when
syscall `0xc0` yields 0, and `FUN_00434160` returns 0 for a null name).

The pick, `FUN_00434160` @ `0x434160` (syscall `0xc0`; the server's
`trap_Com_PickSoundAlias` @ `.so 0x63b48` is syscall `0x3d` into the same code
on the dedicated server):

```
rec    = lookup(name)            // last row of the group
chosen = rec; sum = rec.prob; maxseq = rec.counter; n = 1
for prev in rows before rec while prev.name == rec.name:   // pointer compare
    n++; sum += prev.prob
    if rand() * sum < prev.prob * 32768: chosen = prev     // weighted reservoir pick
    maxseq = max(maxseq, prev.counter)
if n > 2 and chosen.counter == maxseq:                     // the most recently played one
    sum = 0
    for row in group (walking from the last):
        if row.counter != maxseq:
            sum += row.prob
            if rand() * sum < row.prob * 32768: chosen = row
chosen.counter = maxseq + 1
return chosen
```

`rand()` is MSVC's (0..32767, `_DAT_00568f28 = 32768.0`). So variants are
weighted by `probability`; with **three or more** rows the variant played most
recently cannot play twice in a row; with two rows it can. The counter lives in
the record, so the rule is per alias, not per entity.

### 1f. `null.wav`

`VERIFIED` (absence). Nothing in the loader, pick or play path tests for
`null.wav` (the only special-cased file name is `temp.wav`, in the
"used as streamed in alias '%s' and loaded in alias '%s'" warning of
`FUN_00433c20`). `sound/null.wav` is a real 22094-byte file in `pak1.pk3`
(as is `null2.wav`); a `null.wav` row is picked like any other, occupies a
voice, and plays silence. The `whizby` group uses this: `!airfield` rows 1-15
are real files and row 16 is `null.wav` with `probability 5`, so one whiz-by in
four is silent.

---

## 2. Channels

`VERIFIED`. The pointer table at `CoDMP.exe 0x57acec` holds 10 names, in this
order: `0 auto, 1 menu, 2 weapon, 3 voice, 4 item, 5 body, 6 local, 7 music,
8 announcer, 9 shellshock` (strings `0x563af4`, `0x563b90`, `0x560f78`,
`0x563aec`, `0x563ae4`, `0x563adc`, `0x563ad4`, `0x563acc`, `0x563ac0`,
`0x563ab4`). Index = record `+0x2c`.

Every voice-related decision keys on the index, and the test is always the same
pair: `channel == 1 || (6 <= channel && channel <= 9)`.

- **2D channels** `menu, local, music, announcer, shellshock`: played as
  plain Miles samples (`FUN_0044ced0` @ `0x44ced0` routes them to `FUN_0044c980`
  @ `0x44c980`) at pan `0.5`, no distance cull, no falloff, no entity tracking
  (`FUN_0044c040` @ `0x44c040` zeroes the entity offset for them).
- **3D channels** `auto, weapon, voice, item, body`: Miles 3D samples
  (`FUN_0044cb90` @ `0x44cb90`), distance-culled at start, linear falloff, and
  they follow their entity (section 3). Streamed aliases on 3D channels are 2D
  streams with engine-computed volume and pan (section 10).

Voice pools (init in `FUN_0044ba30` @ `0x44ba30` and `FUN_0044bb70` @
`0x44bb70`): 32 3D samples (`DAT_008e1b70 = 0x20`), 32 2D samples
(`DAT_008e1b6c = 0x20`, slot indices `0x2d..`), 13 streams (`DAT_008e1b74 =
0xd`, slot indices `0x20..0x2c`); stream slots 0-4
are reserved (music in slot 0, the two ambient crossfade slots 1 and 2
toggled by `DAT_008e083c = 3 - DAT_008e083c` in `FUN_0044dc80`), general
streams start at 5 (`FUN_0044c720` @ `0x44c720` starts its scan at 5).
Inferred from the slot arithmetic; I found no use of slots 3-4 (open item 7).

**Replacement rule.** `FUN_0044d670` @ `0x44d670` (the common start path)
calls `FUN_0044c7e0(entnum, channel)` @ `0x44c7e0` **only when the channel
index is non-zero**. That function ends every playing 3D sample, 2D sample and
stream whose stored entity number and channel index both match (a finished
sample is left alone). So a new `weapon` sound on entity N stops any `weapon`
sound still playing on entity N; `body` likewise; `local`/`announcer`/etc.
likewise (with entity = whatever the caller passed, `ps.clientNum` for the `s`
command). **`auto` never replaces anything.** Two world-space sounds both
started as `ENTITYNUM_WORLD` (1022) on the same non-auto channel replace each
other too (`bomb_tick` is `voice`, explosions and impacts are `auto`).

**Looping dedupe.** Before allocating a voice, `FUN_0044d250` @ `0x44d250`
scans all three pools for a voice with the same entity number, the same alias
name pointer and the alias `loop` byte set; if found it refreshes that voice's
volume, pitch and frame stamp and returns without starting anything. This is
what makes "play the loop alias every frame" (section 9) work without
restarts. Non-looping aliases are never deduped.

**Voice stealing.** When no voice is free, `FUN_0044c350` @ `0x44c350`
picks a victim among voices whose channel index is `<=` the new sound's
channel index (an `auto` sound can only steal from `auto`; `body` can steal
from `auto..body`). Among candidates it prefers a voice already owned by the
same entity, then the lowest channel index, then the earliest end time
(`DAT_008e089c`, start time + length). Miles reports a finished sample as
status 2, and the allocator (`FUN_0044c5b0` @ `0x44c5b0`) takes the first such
voice before stealing.

Per-channel volume multipliers exist (`DAT_008e079c[channel*3]`, multiplied
into every voice's volume). VERIFIED, the writers of the targets: `S_Init`
sets each to 1 (`0x450056`) and cgame syscall 0xd8 (`0x44e140`) sets them
with a ramp time, which the shellshock's sound half calls
(`cod11-shellshock.md`, "Sound"; the cgame's third 0xd8 site, the thunk at
`0x30031fe0`, has no direct caller); `S_Update` steps each toward its
target (`0x44e8b6`).

---

## 3. Distance falloff and panning

`VERIFIED` (decompilation of both the start and the per-frame paths).

**Listener.** cgame `FUN_30033980` @ `0x30033980` (view setup) calls syscall
`0xd1` with `(cg.snap->ps.clientNum, &cg.refdef.vieworg, &cg.refdef.viewaxis)`
(`DAT_301e2160 + 0xb8`, `&DAT_30209594`, `&DAT_302095a0`); the engine
(`FUN_0044bd60` @ `0x44bd60`) stores origin at `0x8e0850`, the 3x3 axis at
`0x8e085c` (`axis[0]` forward, `axis[1]` left, `axis[2]` up, Q3 convention) and
the entity number at `0x8e0880`. The entity number is never used by the
spatializer (section 5).

**Start-time cull.** `FUN_0044d670` @ `0x44d670`: for 3D channels, if
`|origin - listener| > dist_max` the sound is **not started** and the call
returns 0. There is no later cull; a sound that starts in range keeps playing
(at zero volume beyond `dist_max`) even if it moves away.

**Volume at start** (`FUN_0044cb90` @ `0x44cb90`) and **every frame**
(`FUN_0044eea0` @ `0x44eea0`, called per active 3D voice from `S_Update` =
`FUN_0044f210` @ `0x44f210`):

```
d      = |emitter - listener|                   // sum of squares, then FUN_00538fe0 = CRT _CIsqrt (asm 0x44cc34..0x44cc50)
scale  = 1                              if d <= dist_min
       = 1 - (d - dist_min) / (dist_max - dist_min)   if dist_min < d < dist_max
       = 0                              if d >= dist_max
vol    = s_volume * channelVol[ch] * scale * v
```

where `v` is fixed at start by `FUN_0044d5c0` @ `0x44d5c0`:

```
v     = (vol_min + rand01 * (vol_max - vol_min)) * 0.8     // _DAT_00568f18 = 0.8
pitch =  pitch_min + rand01 * (pitch_max - pitch_min)       // playback rate = file rate * pitch
```

(`rand01 = rand() / 32768`, `_DAT_00568e98 = 1/32768`). The `0.8` is
unconditional; a `vol_min 1` alias plays at 80% of Miles full scale before the
`s_volume` and per-channel multipliers. `s_volume` is `DAT_008e078c`.

Then `AIL_set_3D_sample_distances(sample, dist_max, dist_max)` (in
`FUN_0044cb90` @ `0x44cb90`) and `AIL_set_3D_distance_factor(provider,
0.0254)` at init (float constant `0x3cd013a9`, inches to metres) mean Miles applies **no**
rolloff of its own inside `dist_max`. The curve is the engine's linear one
above; there is no dB or inverse-square component.

Worked examples (scale only, `v` and multipliers excluded):

| Alias | `dist_min` / `dist_max` | d=0 | d=min | midpoint | d=max | beyond |
|---|---|---|---|---|---|---|
| blank/blank (defaults) | 120 / 600 | 1.0 | 1.0 | d=360: 0.5 | 0.0 | not started |
| `step_run_dirt` | 50 / 1000 | 1.0 | 1.0 | d=525: 0.5 | 0.0 | not started |
| `bullet_small_dirt` | 150 / 700 | 1.0 | 1.0 | d=425: 0.5 | 0.0 | not started |
| `whizby` | 100 / 700 | 1.0 | 1.0 | d=400: 0.5 | 0.0 | not started |
| `bomb_tick` (`1200`, blank) | 1200 / 6000 | 1.0 | 1.0 | d=3600: 0.5 | 0.0 | not started |
| `land_dirt` | 80 / 120 | 1.0 | 1.0 | d=100: 0.5 | 0.0 | not started |

**Position and pan for 3D samples.** `FUN_0044bf70` @ `0x44bf70` gives Miles
the emitter in listener space: `x = -(rel . axis[1])` (right), `y = rel .
axis[2]` (up), `z = rel . axis[0]` (forward), `rel = emitter - listener`. Miles
does the panning from that; the engine does not compute a pan for 3D samples.
The listener orientation is not separately set (Miles' default listener looks
down +z), which is why the engine rotates the emitter instead.

**Pan for positional streams** (`FUN_0044c240` @ `0x44c240`, asm
`0x44c27d..0x44c348`): `u = normalize(emitter - listener)`,
`pan = (1 - u . axis[1]) * 0.5` (`_DAT_00568e70 = 0.5`), so a source straight
left gives `0.0` (Miles full left), straight right `1.0`, ahead or behind `0.5`.
No front/back cue. Volume uses the same linear `scale`. Updated per frame by
`FUN_0044f100` @ `0x44f100`.

**Entity tracking.** When a 3D-channel sound starts with an entity number
below `0x400`, `FUN_0044c040` @ `0x44c040` asks the cgame for that entity's
origin and axis (`VM_Call(cgvm, 0xd, entnum, &origin, &axis)` =
`FUN_00460480`) and stores the start position as an offset in entity space; each
frame `FUN_0044bed0` @ `0x44bed0` recomputes the world position from the
entity's current origin and axis. So footsteps, fire sounds and loop sounds
follow their entity; `ENTITYNUM_WORLD` (1022) is a static frame. This matters
for vcod, since the retail client re-spatializes a sound against the entity's
*current* position, not the position it started at.

---

## 4. `masterslave`

`VERIFIED`. Once per `S_Update` (`FUN_0044f210` @ `0x44f210`), `DAT_008e088c =
FUN_0044ed60()` @ `0x44ed60`, which is 1 when **any** voice in any pool is
still playing an alias whose master byte (`+0x35`) is set. Then for every voice
whose alias has the slave byte (`+0x36`) set, the per-frame volume is
`min(scale * v, slaveLevel)` before the `s_volume` and channel multipliers
(`FUN_0044eea0` for 3D samples, `FUN_0044f070` @ `0x44f070` for 2D samples,
`FUN_0044f100` for streams). `slaveLevel` is record `+0x38`, the number from the
csv cell. A numeric row does nothing while no master plays. A `master` row is
not itself changed. When two aliases are blended (syscall `0xc3`), the level is
the blend of both records' levels, and `FUN_0044d800` @ `0x44d800` refuses blends
whose master/slave status, channel, type, loop or file differ (warnings
`0x55f6c0..0x55f9a0`).

Retail MP relevance: `iw_sound.csv` has 10 `master` rows, all SP ambients or
SP-only weapon loops (`ambient_dam_int`, `weap_nagant_sniper` on Stalingrad,
`flakpanzer_loop`...), and 445 numeric rows (`weap_mg42_loop 0.75`, ...). No
`master` row has an MP loadspec, so in MP the mechanism never engages
(inferred from the csv, not from code).

---

## 5. Own-entity handling and `channel=local`

`VERIFIED` (absence in the engine, presence in the cgame).

The engine has no own-entity rule. The listener entity number stored by
`S_Respatialize` (`0x8e0880`) is read by exactly one function,
`FUN_0044d9c0` @ `0x44d9c0` (play-by-name with the listener's entity number and
no origin, used by the UI), and by nothing on the spatialize path. Whether a
sound is spatialized is decided by its channel alone (section 2). A `body` or
`weapon` sound emitted by the followed player is a 3D sound positioned at (or
near) `cg.refdef.vieworg`; its distance is below `dist_min`, so `scale = 1`,
and Miles pans it from a position at the listener, which comes out centred.

What the cgame does differently for the body the view is attached to (the
test is always `(cg.snap->ps.pm_flags & 0x50000) != 0 && es.number ==
cg.snap->ps.clientNum`, see the events-and-fx doc section 7):

- `CG_FireWeapon` (`FUN_30038b70` @ `0x30038b70`): the fire sound's origin is
  the **view-model** `tag_flash` (`FUN_3001c390`) instead of the world-model
  muzzle (syscall `0xa2` + `FUN_3001c2c0`); both fall back to the entity
  origin via `FUN_30005470`. The sound is still started as a 3D `weapon` sound
  on the player's entity number.
- Brass (`FUN_30038a30` @ `0x30038a30`): `tag_brass` is looked up on the
  view-model entity `0x400 + weapon` for the own body, the world model
  otherwise. Brass has no sound; it is an effect (`shellEjectEffect` /
  `lastShotEjectEffect`, section 7).
- Footsteps, jumps, landings: **no** first-person suppression. The same
  alias plays on the own entity (section 7).
- `EV_STEP_VIEW` (143), `EV_STANCE_*` (140-142), the pickup HUD message and
  the landing view-bob are the only events gated to the own client, and they
  make no sound.

`channel=local` means "2D, pan centre, no distance cull, no falloff, not
entity-tracked", nothing more (`FUN_0044ced0` @ `0x44ced0` routes channels 1
and 6..9 to `FUN_0044c980`; `FUN_0044c040` zeroes the entity offset for them).
It does not suppress the entity-channel replacement rule; two `local` sounds
started with the same entity number replace each other.

The `s` server command (`CG_LocalSound`, `FUN_3002e040` @ `0x3002e040`) plays
configstring `524 + idx` with entity `cg.snap->ps.clientNum` at
`cg.snap->ps.origin` (`DAT_301e2160 + 0x20`; inferred from the `ps` layout,
`ps` at `snap+0xc` with `pm_flags` at `+0xc` and `clientNum` at `+0xac` as
anchors); the alias's own channel decides
2D or 3D. Chat messages (`h`, `i`) play `player_talk` and bold game messages
(`g`) play `game_message` the same way (`FUN_30021ba0` @ `0x30021ba0` from
`CG_ServerCommand` = `FUN_3002e0d0` @ `0x3002e0d0`).

---

## 6. Whizby

`VERIFIED` from `CG_BulletWhizby` = `FUN_30038dc0` @ `cgame 0x30038dc0`
(asm `0x30038dc0..0x30038f29`). Inputs: `EBX = start`, `EAX = impact`
(register-passed; the decompiler shows them as `unaff_EBX`/`in_EAX`).
Constants: `0x300695a0 = 64.0`, `0x3006959c = -64.0`, `0x30069598 = 140.0`,
`0x30069468 = 16.0`. `DAT_30209594..9c` is `cg.refdef.vieworg`.

```
u = normalize(impact - start); L = |impact - start|     // FUN_30039ef0 returns L
t = (vieworg - start) . u
if 64 <= t and t + 64 <= L:                              // closest point at least 64 from both ends
    p = start + t * u
    if |p - vieworg| <= 140:
        play "whizby" (handle DAT_301d6120) as ENTITYNUM_WORLD (0x3fe) at p - 16 * u
```

The sound is placed 16 units back toward the shooter from the closest point,
so Miles pans it from the side the bullet passed and it is never exactly on
the listener. `whizby` is `auto`, `100/700`, so at 140 units it is at full
volume.

Call chain: `CG_EntityPreEvent` case 173/174 → `FUN_30039640` @ `0x30039640`
(impact sound + effect) → `FUN_30039590` @ `0x30039590` → `FUN_30039440` @
`0x30039440` computes `start` (own body: `ps.origin + ps.viewheight`; others:
world-model `tag_flash` via `FUN_3001c2c0`, falling back to the entity origin
plus a stance offset) → tracer decision (skipped for the own body, otherwise
`rand() < cg_tracerchance * 32768`, `cg_tracerchance` default `0.4`, vmCvar
`0x301df440`, cvar table `0x30074e50`; a flesh impact (`surfType == 7`) spawns
`FUN_30039340` instead of a tracer) → **`CG_BulletWhizby` is called
unconditionally afterwards** (the two `call 0x30038dc0` at `0x30039618` and
`0x3003962e`, both inside `FUN_30039590`). The whole chain is skipped only when `cg_tracerchance < 0`
or the shooter's start point cannot be computed (entity has no model, syscall
`0xa2` returns 0).

Flesh hits do **not** skip it: `CG_BulletHitFlesh` = `FUN_300396f0` @
`0x300396f0` (cases 175/176) plays `bullet_small_<surf>` / `bullet_large_<surf>`
at the impact as `ENTITYNUM_WORLD` and then calls the same `FUN_30039590`.
175/176 reach only the victim and whoever follows it; everyone else gets the
flesh hit as 173/174 with `surfType == 7` through `FUN_30039640`
(events-and-fx doc, section 2). vcod plays the same alias and runs the same
whizby test for all four.

Inferred: `FUN_30039590`'s "own body" test compares the **attacker** entity
(`es.otherEntityNum`, in `EBX`) with `ps.clientNum`, so when following a player
his own shots produce no tracer but still run the whizby test against his own
view origin (which fails the `64 <= t` margin for a shot starting at the eye,
so nothing plays).

---

## 7. Per-event alias names

### 7a. The per-surface tables

`VERIFIED`. The alias registration function `FUN_30020a00` @ `cgame 0x30020a00` (asm
`0x30020c2b..0x30020cb2`) fills nine 23-entry tables by calling `FUN_30020980`
@ `0x30020980` with `EDI = table`, `EBX = prefix`; that helper loops `i = 0..22`, asks the engine
for the surface name (syscall `0xc5` = `FUN_00401cc0` @ `CoDMP.exe 0x401cc0`,
table `0x571790`; index 0 and out-of-range give `"default"`), formats
`"%s_%s"` (prefix, surface) and registers it with syscall `0xbf`:

| Table (cgame) | Prefix | Used by |
|---|---|---|
| `0x301d5d7c` | `grenade_bounce` | `EV_GRENADE_BOUNCE` 177 |
| `0x301d5dd8` | `grenade_explode` | `EV_GRENADE_EXPLODE` 178 |
| `0x301d5e34` | `rocket_explode` | `EV_ROCKET_EXPLODE` 179, `_NOMARKS` 180 |
| `0x301d5eec` | `bullet_small` | 173, 175 |
| `0x301d5f48` | `bullet_large` | 174, 176 |
| `0x301d5fa4` | `step_run` | 1-23 and **70-92 (jump)** |
| `0x301d6000` | `step_walk` | 24-46 |
| `0x301d605c` | `step_prone` | 47-69 |
| `0x301d60b8` | `land` | 93-115 and 116-138 |

Surface order (engine table `0x571790`, matches the events-and-fx doc section
4): `default, bark, brick, carpet, cloth, concrete, dirt, flesh, foliage,
glass, grass, gravel, ice, metal, mud, paper, plaster, rock, sand, snow, water,
wood, asphalt`. **The engine spells it `asphalt`; every csv row spells it
`asphault`** (`step_run_asphault`, `Land_asphault`, ...; `grep -ic asphalt
iw_sound.csv` = 0). So `step_run_asphalt`, `land_asphalt`, `bullet_*_asphalt`,
`grenade_*_asphalt` register as `NULL` and **asphalt surfaces are silent in the
retail client**. There is no fallback to `_default` anywhere; a null handle is a
no-op (section 1e). vcod's surface table (`ALIAS_SURFACES` in
`crates/client/src/audio/cues.rs`) spells index 22 `asphault`, so asphalt
surfaces are audible in vcod and silent in retail. This is deliberate, and the
only place vcod knowingly diverges from the engine's alias names.

`canvas` is the other alias-side surface that does not exist in the engine:
the surface name table at `0x571790` has 23 entries (`FUN_00401cc0` returns
`"default"` for any index outside `1..22`, and each cgame table is `0x5c` =
23 pointers wide), and none of them is `canvas`. The 36 `*_canvas` rows in
`iw_sound.csv` (`bullet_small_canvas`, `bullet_large_canvas`, 18 each) are
therefore **unreachable in 1.1 MP**: no `surfType` maps to them and no code
path asks for them by name. vcod must not invent a canvas suffix.

Alias names are matched case-insensitively (section 1e), so csv `Land_bark`
satisfies `land_bark`.

### 7b. `CG_EntityEvent` (`FUN_3001dc10` @ `0x3001dc10`) range ladder

`VERIFIED` (asm `0x3001dc98..0x3001de10`). `EDI` = event id, `ESI` = cent,
`[ESI]` = entity number, `EBX` = `es.clientNum` (0 if out of range).
`FUN_30021bc0(entnum, handle)` @ `0x30021bc0` plays a handle at
`cg_entities[entnum].currentState.pos.trBase` (`&DAT_3020db98 + entnum*0x228`;
`DAT_3020db80` is `cg_entities[0].currentState.number` and `+0x18` is
`pos.trBase`), i.e. the snapshot origin rather than the interpolated one, with
that entity number, so the engine tracks the entity from there (section 3);
`FUN_30021bf0(entnum, origin)` with `EAX = handle` plays at an explicit origin.

| Ids | Group | Sounds |
|---|---|---|
| 1-23 | `EV_FOOTSTEP_RUN_*` | if `cg_footsteps` (vmCvar `0x301e0f40`, default 1): `step_run_<surf>` on the entity; then **always** `gear_rattle_run` (`DAT_301d6114`) at the entity origin |
| 24-46 | `EV_FOOTSTEP_WALK_*` | same path as run (`jmp 0x3001dced`) but the table index is the event id, so `0x301d5fa0 + id*4` lands in the `step_walk` table; then **always** `gear_rattle_walk` (`DAT_301d6118`) |
| 47-69 | `EV_FOOTSTEP_PRONE_*` | same path again: `step_prone_<surf>` (index arithmetic lands in the `step_prone` table), then `gear_rattle_walk` |
| 70-92 | `EV_JUMP_*` | `0x301d5e8c + id*4` = `0x301d5fa4 + (id-70)*4`: **`step_run_<surf>`**, unconditionally (no `cg_footsteps` test), then `gear_rattle_run`. There is no `jump_*` alias in the csv and the client never asks for one |
| 93-115 | `EV_LANDING_*` | `0x301d5f44 + id*4` = `land_<surf>`; own client only: view bob (`DAT_302094e4 = -eventParm`) |
| 116-138 | `EV_LANDING_PAIN_*` | `0x301d5ee8 + id*4` = `land_<surf>` (same table), then `land_damage` (`DAT_301d5d64`); own client only: damage kick |

For the walk and prone rows the decompiler shows all three groups reading
`&DAT_301d5fa0 + id*4`; since the three tables are contiguous (`step_run` at
`0x301d5fa4`, `step_walk` at `0x301d6000` = `+23*4`, `step_prone` at
`0x301d605c` = `+46*4`) that single expression indexes the right table for each
group. The jump group is the one anomaly; its base `0x301d5e8c` puts ids 70-92
back on the `step_run` entries.

### 7c. Switch cases (139-201)

`VERIFIED` from the same function unless marked. Alias handles come from the
`syscall(0xbf, "name")` calls in `FUN_30020a00` (`0x30020a00`); `w` = the
entity's weapon (`es.weapon`, entity-state offset `0xc8`), `wdef` = its weapon def.

| Id | Name | Sound |
|---|---|---|
| 139 | `EV_FOLIAGE_SOUND` | `movement_foliage` (`DAT_301d611c`) on the entity |
| 140-143 | stance / step view | none |
| 144 | `EV_WATER_TOUCH` | `player_water_in` (`DAT_301d6148`) on the entity |
| 145 | `EV_WATER_LEAVE` | `player_water_out` (`DAT_301d614c`) |
| 146 | `EV_ITEM_PICKUP` | `DAT_301dc3fc[eventParm]` (item index 1..69) on the entity: the item's pickup alias, see 7d |
| 147 | `EV_ITEM_PICKUP_QUIET` | none (HUD message only) |
| 148 | `EV_AMMO_PICKUP` | `DAT_301dc400[eventParm]`, which `FUN_30034cf0`'s item registration (`0x30034cf0`) sets **equal to** `DAT_301dc3fc`, so the same alias as 146 |
| 149 | `EV_NOAMMO` | `player_out_of_ammo` (`DAT_301d5d60`) unless `wdef.clipOnly` (`+0x2d4`) |
| 150 | `EV_EMPTYCLIP` | none |
| 151 | `EV_RELOAD` | `reloadSound`, else `reloadEmptySound` |
| 152 | `EV_RELOAD_FROM_EMPTY` | `reloadEmptySound`, else `reloadSound` |
| 153 | `EV_RELOAD_START` | `reloadStartSound` |
| 154 | `EV_RELOAD_END` | `reloadEndSound` |
| 155 | `EV_RAISE_WEAPON` | `raiseSound`, registered with fallback alias `weap_raise` when the key is empty |
| 156 | `EV_PUTAWAY_WEAPON` | `putawaySound`, fallback `weap_putaway` (which is a `null.wav` row) |
| 157 | `EV_WEAPON_ALT` | `altSwitchSound` |
| 158 | `EV_PULLBACK_WEAPON` | `pullbackSound` |
| 159, 160, 168 | fire | `CG_FireWeapon`: section 8 |
| 161 | `EV_FIRE_WEAPON_LASTSHOT` | `lastShotSound` if set, else `fireSound` |
| 162 | `EV_RECHAMBER_WEAPON` | `rechamberSound` (in `CG_EntityEvent` only when its first argument, the pre-event flag, is non-zero; `CG_EntityPreEvent` case `0xa2` plays it on the entity) |
| 163 | `EV_EJECT_BRASS` | none: `FUN_30038a30` spawns the `shellEjectEffect` / `lastShotEjectEffect` `.efx` at `tag_brass`. Any brass sound would come from a `Sound` block inside that effect |
| 164 | `EV_MELEE_SWIPE` | `melee_swing_small` (`DAT_301d6128`) when `wdef.rifleBullet` (`+0x2c0`) is 0, else `melee_swing_large` (`DAT_301d6124`), on the attacker |
| 165 | `EV_FIRE_MELEE` | none |
| 166 | `EV_MELEE_HIT` | `melee_hit` (`DAT_301d612c`) on **`es.otherEntityNum`** (entity-state offset 116), the victim |
| 167 | `EV_MELEE_MISS` | none |
| 169, 170 | quad-barrel | `CG_FireWeapon` twice (barrel 0 and 1) |
| 172 | `EV_SOUND_ALIAS` | configstring `524 + eventParm` (`FUN_30021b00(parm + 0x20c)`), played on the entity at `es.pos.trBase` (entity-state offset `0x18`) |
| 173, 174 | bullet hit small/large | pre-event `FUN_30039640`: `bullet_small_<surf>` / `bullet_large_<surf>` as `ENTITYNUM_WORLD` at the impact, then tracer/whizby (section 6) |
| 175, 176 | client hit | pre-event `FUN_300396f0`: same tables, `surfType` from the event (always 7) |
| 177 | `EV_GRENADE_BOUNCE` | `grenade_bounce_<surf>` (`0x301d5d7c[es.surfType]`, asm `0x3001e4a6`) as `ENTITYNUM_WORLD` at the event position; plus a per-surface effect table `DAT_301d6300` |
| 178 | `EV_GRENADE_EXPLODE` | `grenade_explode_<surf>` (`0x301d5dd8`, asm `0x3001e501`) as world; then the weapon's `projExplosionEffect` (`+0x324`, fx) and, if the weapon's `projExplosionSound` (`+0x328`, registered as an alias handle into slot `DAT_301a6a8c`) is non-empty, that alias as world at the same point (asm `0x3001e581..0x3001e58b`) |
| 179, 180 | rocket explode (`_NOMARKS`) | `rocket_explode_<surf>` (`0x301d5e34`, asm `0x3001e5be`) as world, then `projExplosionEffect` and `projExplosionSound` as for 178 |
| 181-186 | molotov, custom, railtrail, bullet | unhandled ("Unknown event") |
| 187, 188 | `EV_PAIN`, `EV_CROUCH_PAIN` | **none**: bare `break`, and `CG_EntityPreEvent` has no case for them. The MP client registers no pain alias at all (`FUN_30020a00` has none, and the decompilation of `cgame_mp_x86.dll` contains no `"player_pain"` or `"pain"` alias string). Whether the server plays a pain alias through `EV_SOUND_ALIAS` was not checked in the `.so` |
| 189 | `EV_DEATH` | `FUN_30021bc0(entnum, DAT_301d5d68)`, but **`DAT_301d5d68` is never written** (the only references in the cgame disassembly are reads at `0x3001e688`; it is not in `FUN_30020a00`), so the handle is 0 and nothing plays. Same for `DAT_301d5d6c/70/74` used by 199, 200, 197/198 |
| 191-193 | play fx | effects only; a `Sound` block in the `.efx` is the sound path |
| 194 | `EV_FLAMEBARREL_BOUNCE` | `flamebarrel_bounce` (`DAT_301d6180`) on the entity |
| 195 | `EV_EARTHQUAKE` | none in this function |
| 196 | `EV_DROPWEAPON` | none |
| 197, 198 | item respawn / pop | handle `DAT_301d5d74` (never registered: silent) |
| 199, 200 | teleport in / out | handles `DAT_301d5d6c` / `DAT_301d5d70` (never registered: silent) |
| 201 | `EV_OBITUARY` | none (HUD) |

Other registered handles for completeness (`FUN_30020a00`): `player_gib`,
`player_gib_bounce`, `player_talk` (chat), `game_message` (bold game message
`g`), `objective_complete`, `mp_announce_{g,a}_{twominutes,thirtyseconds}`
(round timer, played by the cgame from the clock, not by an event),
`player_grenade_pulse_0..3`, `debris_bounce`, `debris_hit_player`,
`flame_*`, `player_bone_bounce`, `spotlight_spark`.

### 7d. Pickup aliases

`VERIFIED` for the table shape, inferred for the weapon rows. `DAT_301dc3fc +
item*0x24` is filled by `FUN_30034cf0` @ `0x30034cf0` from
`bg_itemlist[item].pickup_sound` (`DAT_300762f4 + item*0x30`). In the DLL's
static data only items 65-69 carry a name: `grenade_pickup` (65, 66),
`health_pickup_small/medium/large` (67-69); the weapon items (1-64) are filled
at weapon registration (also in `FUN_30034cf0`): `pickupSound` (`+0x8c`)
with fallback alias `weap_pickup`, and `ammoPickupSound` (`+0x90`) with
fallback `weap_ammo_pickup` into the next field. Because `EV_AMMO_PICKUP`
reads the copy of the pickup column, **`ammoPickupSound` is never heard**
(inferred: `DAT_301dc400` has no other writer).

---

## 8. Weapon-file sound keys

`VERIFIED`. The cgame weapon field table (entries `{name, offset, type}` at
`0x300756d8..`) gives the weapon struct offsets, and `FUN_30034cf0` @
`0x30034cf0` registers them with syscall `0xbf` into
per-weapon slots (`0x198` bytes per weapon, base `DAT_301a6a08`):

| Key | Struct | Slot | Read by |
|---|---|---|---|
| `pickupSound` | `+0x8c` | item table | 146 (7d) |
| `ammoPickupSound` | `+0x90` | item table | never (7d) |
| `projectileSound` | `+0x94` | `6a08` | `FUN_3001b3f0` @ `0x3001b3f0` (`ET_MISSILE` add-entity): played **every frame** on the missile entity at its lerp origin; only sensible for looping aliases (the engine dedupes those, section 2). See "`projectileSound`" below |
| `pullbackSound` | `+0x98` | `6a0c` | 158 |
| `fireSound` | `+0x9c` | `6a10` | `CG_FireWeapon` |
| `loopFireSound` | `+0xa0` | **not registered** | nothing |
| `stopFireSound` | `+0xa4` | **not registered** | nothing |
| `fireEchoSound` | `+0xa8` | `6a14` | nothing (registered, never read) |
| `lastShotSound` | `+0xac` | `6a18` | `CG_FireWeapon` when event == 161 |
| `rechamberSound` | `+0xb0` | `6a1c` | 162 |
| `reloadSound` | `+0xb4` | `6a20` | 151, 152 fallback |
| `reloadEmptySound` | `+0xb8` | `6a24` | 152, 151 fallback |
| `reloadStartSound` | `+0xbc` | `6a28` | 153 |
| `reloadEndSound` | `+0xc0` | `6a2c` | 154 |
| `raiseSound` | `+0xc4` | `6a30` (fallback `weap_raise`) | 155 |
| `altSwitchSound` | `+0xc8` | `6a34` | 157 |
| `putawaySound` | `+0xcc` | `6a38` (fallback `weap_putaway`) | 156 |
| `noteTrackSoundA..D` | `+0xd0..0xdc` | `6a3c..6a48` | animation notetracks (registered in `FUN_30034cf0`, `0x30034xxx`): view-model reload animations, not events |
| `projExplosionSound` | `+0x328` | `6a8c` | 178, 179, 180 (7c) |

`CG_FireWeapon` = `FUN_30038b70(event, barrel)` @ `0x30038b70`:

```
handle = fireSound[w]
if event == 161 (EV_FIRE_WEAPON_LASTSHOT) and lastShotSound[w] != 0: handle = lastShotSound[w]
origin = own body ? viewmodel tag_flash : world-model muzzle (fallback: entity origin)
play(handle, entnum, origin)          // 3D, channel from the alias (weapon)
if wdef.boltAction (+0x2c8) == 0: eject brass effect
```

So 159 `EV_FIRE_WEAPON`, 160 `EV_FIRE_WEAPONB`, 168 `EV_FIRE_WEAPON_MG42` play
`fireSound`; 161 plays `lastShotSound` when the weapon file sets it, else
`fireSound`; 169/170 `EV_FIRE_QUADBARREL_1/2` call it twice with barrel 0 and
1 (two `fireSound` plays at the two `tag_flash` tags). The 1.1 MP client
**never uses `loopFireSound` or `stopFireSound`**; the eight looping weapons'
fire loops in the csv are SP-only in practice.

VERIFIED, the origin's arms in 1.1's asm: the own-body test at
0x30038c32..0x30038c43, the viewmodel tag (0x3001c390, name 0x30062088)
at 0x30038c55, syscall 0xa2 on `es.number` at 0x30038c83 and the world
tag (0x3001c2c0, same name) at 0x30038c9d, the fallback 0x30005470 on
`es.pos` at `cg.time` (0x30207148) at 0x30038ccf, and the start
0x30021bf0(`es.number`, origin) at 0x30038cde. The tag name is the
pointer at 0x30062088 on both quad-barrel calls, unlike the flash's
per-barrel array. VERIFIED: 1.5's `CG_FireWeapon` (0x3003a2a0) has the same
arms (`cod11-events-and-fx.md` 6). INFERRED, UO's (0x30047d20, decompile):
syscall 0xa5 then 0x3001fdf0 for another body, the viewmodel's tag
0x3001fec0 for the own one when its viewmodel is loaded, the fallback
0x30005f30, then the start 0x3002ca80(`es.number`).

The start is entity-tracked (section 3): the engine keeps it as an
offset in the frame cgame `vmMain` 13 returns. VERIFIED, 0x30020320
(1.1): it copies three floats at `0x3020dd78 + 0x228 * entnum` as the
origin, has 0x3003a790 build the axis from the three floats after them,
and negates axis[1]. VERIFIED: 0x3020dd78 is `cg_entities` (0x3020db80) plus
0x1f8, `lerpOrigin`, and the three floats after it are `lerpAngles`
(+0x204), the fields `vmMain` 12 (0x30020260) reads
(`cod11-events-and-fx.md` 6). INFERRED: the engine takes the offset with
the axis it was handed and rebuilds the position with the current one
(section 3), so the offset turns with the body's `lerpAngles`: for a
player, its view angles, pitch included.

vcod: `audio::cues` starts another body's fire sound as a
`Source::Tag` at its posed `tag_flash` (`BuiltScene::muzzles`, the gun's
own for a turret), else at the event's position, on all three
protocols. The ridden body's starts on its own number at the viewmodel's
`tag_flash` (the muzzle map's view entry), or at the listener under a
scope, where the view keys no muzzle. `VoiceTable::update` keeps a tag
voice's start as an offset in its entity's frame: `AnglesToAxis` of
`BuiltScene::entity_angles` (the interpolated angles, not the yaw a
player draws with), and the view's basis for the ridden body, whose
position is the listener's. One gap: retail's ridden body sits at its
predicted origin, ours at the eye; the offset moves with either, so the
voice lands in the same place.

### `projectileSound`

- VERIFIED, 1.1 cgame disassembly 0x3001b40a..0x3001b456: the `ET_MISSILE`
  case of the add-entity switch (`FUN_3001d5f0`, case 4) returns at once
  when `eFlags & 0x100` (`EF_NODRAW`) is set. It resets a `weapon` above the
  weapon count (`0x300eef38`) to 0. When the weapon's `projectileSound`
  handle (`0x301a6940 + 0x198 * weapon + 0xc8`, the `6a08` slot) is set,
  it calls `FUN_30021bf0(entnum, &cent->lerpOrigin)` (`+0x1f8`).
- VERIFIED, 1.1 cgame decompile: `FUN_30021bf0` is also what the
  `es.loopSound` add-entity (`FUN_3001af20`) plays through: syscall 0xc0
  picks the alias, syscall 0xc2 starts it on the entity.
- VERIFIED, 1.5 cgame decompile: the same case at `0x3001c670`. Its
  `projectileSound` slot is `0x301aa5a8`, registered from weaponDef
  `+0x94`. It plays through `0x30022ed0`, which the `loopSound` add-entity
  `0x3001c0c0` also calls.
- VERIFIED, uo cgame disassembly 0x3001edc3..0x3001ee0d and decompile: the case at
  `0x3001edb0` tests UO's `EF_NODRAW` (0x80, the sign bit of the low
  `eFlags` byte), clamps the weapon against `0x30134cd4`, and reads weaponInfo
  `0x30413580 + 0x1c4 * weapon + 0xdc`. It calls `0x3002ca80(entnum)` with
  `&cent->lerpOrigin` (`+0x208`) in `ecx`; the `loopSound` add-entity
  `0x3001e7f0` calls the same function.
- VERIFIED, uo cgame decompile and its weapon field table: the `+0xdc` slot
  is registered from weaponDef `+0xa4` (`0x300442c8..`). The table's entry
  at `0x30084594` names `projectileSound` (`0x300762ec`) at offset `0xa4`.
- INFERRED from the above and section 2: the looping dedupe holds one voice
  per missile, and the end-of-frame reaper ends it on the first frame the
  missile is not added. That frame is the one where it leaves the snapshot,
  gets `EF_NODRAW`, or blows: the explode rides the missile's own entity as
  `eType` 0, so the `ET_MISSILE` case stops running.
- VERIFIED, the paks: the only stock weapon files that set the key are
  `panzerfaust_mp` (1.1 `pak0`, 1.5 `pak0`/`pak8`) and UO's `bazooka_mp`,
  `panzerfaust_mp` and `panzerschreck_mp` (`pakuo04`/`pakuo07`), all with
  `weap_panzerfaust_projectile`.
- VERIFIED, the paks: that alias's only row is `null.wav`, `auto`,
  `7`/`500`, looping (1.1 `pak1` `iw_sound.csv`; UO `pakuo03`/`pakuo07`
  `generic_mp_sound.csv` and `pakuo03` `iw_sound.csv`). So a stock rocket
  flies silent on all three versions: the voice is real but plays the
  silent file (section 1f). UO's tank and gun shells are heard in flight
  through `projectileSoundBlend1/2` instead (coduo-diff.md 16).
- VERIFIED, live (`*-uo-bazooka.bin`, coduo-diff.md 19 "Capture recipe"):
  one bazooka round rides entity 158 as `eType` 4 for eight snapshots and
  blows on it as `eType` 0, 400 ms after the fire event.

vcod: `audio::missile_sounds` collects every `ET_MISSILE` without
`EF_NODRAW` in the newest snapshot, and `AudioSystem::set_projectile_sounds`
holds one entity voice per missile on its weapon's alias through the same
reconcile as `es.loopSound`. Two differences from retail: like the loop
sounds it runs once per snapshot on the newest one, where retail adds
`cg.snap`'s entities every frame, and a `null.wav` pick starts no voice
(divergences below). `the_uo_bazooka_capture_plays_its_projectile_sound_in_flight`
gates it on the capture.

---

## 9. Wire path (server side, `game.mp.i386.so`)

All function addresses are the symbol table's. Call targets were resolved
through `readelf -r` (`R_386_PC32` names), gsc command names through the
method tables at `.so 0x7e7c0` (`ambientplay`, 12-byte entries) and `0x7ea80`
(`playsound`, `playloopsound`, `stoploopsound`, 8-byte entries) and the player
method table at `0x73544` (`playlocalsound`).

Shared plumbing, `VERIFIED`:

- `G_SoundAliasIndex(name)` @ `0x675c0`: linear scan of configstrings
  `0x20c + 1 .. 0x20c + 255` (`CS_SOUNDS = 524`; index 0 is "none"), adding the
  name at the first empty slot via `trap_SetConfigstring`; 256 in use is a
  fatal `"G_FindConfigstringIndex: overflow"`. The client side agrees: both
  `EV_SOUND_ALIAS` and `CG_LocalSound` add `0x20c`.
- `G_PlaySoundAlias(ent, idx)` @ `0x67b30`: if `ent->client`, appends event
  `0xac` (172, `EV_SOUND_ALIAS`) with `eventParm = idx` to
  `ps.events[ps.eventSequence & 3]` (offsets `0x88`/`0x98`/`0x84` in the
  playerState); else to the entity's `es.events` ring (`0xa8`/`0xb8`/`0xa4`).
  Sets `ent->eventTime` (`+0x180`) and `+0x150` to `level.time`.
- `G_PlaySoundAliasAtPoint(point, idx)` @ `0x67a2c`: `G_Spawn`, `eType = 0xb8`
  (`ET_EVENTS 12 + 172`), origin from the rounded point, `+0x184 = 1`
  (inferred: `freeAfterEvent`), links, `eventParm = idx`. In MP only
  `hurt_touch` calls it.
- `G_SetClientSound(ent)` @ `0x41614`: `es.loopSound = 0` (offset 132) and
  nothing else. **No caller** exists in the binary (no `R_386_PC32` reloc, no
  direct `call 41614`), so it is dead code; the Q3 water/lava loop is gone.

Per gsc call:

| gsc | Handler | What goes on the wire | Reaches a spectator? | Expected observable |
|---|---|---|---|---|
| `ambientPlay(alias [, fade])` | `0x5adc4` | `trap_SetConfigstring(3, va("n\\%s\\t\\%i", alias, level.time + fade_ms))` (format `0x77865`); `fade` is seconds, `* 1000 + 0.5` | yes, configstring 3 is in the gamestate and updated by `d 3 ...` | at connect on `mp_carentan`: CS 3 = `n\ambient_mp_carentan\t\<time>`. Client: `CG_ConfigStringModified` (`FUN_3002c6b0` @ `0x3002c6b0`, the index 3 case) and `CG_Init`/map restart (`FUN_30022f90`, `FUN_3002ca60`) call `FUN_30021b30` @ `0x30021b30`: `pick(Info_ValueForKey(cs, "n"))` (keys `0x300636a0` = `"n"`, `0x3006369c` = `"t"`), `fade = max(0, atol(t) - cg.time)`, syscall `0xd6` = `FUN_0044dc80` crossfades into the ambient stream slot; the alias is `local`/`streamed`/`looping`, so a 2D loop |
| `ambientStop([fade])` | `0x5ee40` | CS 3 = `t\\%i` (`0x778e5`), no `n` | yes | inferred: the client picks the empty name, gets 0, and `FUN_0044dc80` returns early on a null record, i.e. **`ambientStop` does nothing on the client**. Not used by stock MP scripts anyway |
| `<ent> playSound(alias)` | `0x5d910` (`Scr_MakeGameMessage+0xc38`) | `G_SoundAliasIndex` → `G_PlaySoundAlias(ent, idx)`; also `r.svFlags = (svFlags & ~SVF_NOCLIENT) \| 8` (`+0xf4`; bit 8 unnamed in CoDExtended) | yes, `EV_SOUND_ALIAS` on the entity; for a player it rides `ps.events`, which `BG_PlayerStateToEntityState` copies to `es.event` for everyone else (inferred, Q3 lineage) | SD: `other playsound("MP_bomb_plant")` → event 172 on the planter's entity, `parm = idx`, CS `524+idx` = `MP_bomb_plant`; `level.bombmodel playSound("Explo_plant_no_tick")` → 172 on a non-player entity |
| `<ent> playLoopSound(alias)` | `0x5d980` | `es.loopSound = idx` (offset 132, 8 bits) | yes, it is a netfield | after a plant, the bomb entity's `loopSound` = idx of `bomb_tick` (`voice`, `1200/6000`, looping) until defuse/explosion sets it back to 0 via `stopLoopSound` (`0x5d9d8`: `es.loopSound = 0`). Client: `FUN_3001af20` @ `0x3001af20` (add-entity) plays CS `524 + loopSound` **every frame** at the entity's lerp origin (brush models: origin + model centre, `DAT_301d39c0[modelindex]`), and the engine's looping dedupe (section 2) plus the end-of-frame reaper (syscall `0xd2`, `FUN_0044e600`: looping voices not refreshed this frame are ended) turn that into one continuous loop. A non-looping alias in `loopSound` would restart every frame |
| `<player> playLocalSound(alias)` | `0x45800` (`PlayerCmd_pingPlayer+0x75c`) | `trap_SendServerCommand(entnum, 0, va("s %i", idx))` (`0x733d4`) to **that client only**; errors if not a player | only when addressed to the spectator itself | SD round end: `players[i] playLocalSound("MP_announcer_allies_win")` iterates `getentarray("player","classname")`, which includes spectators, so the spectator receives `s <idx>` with CS `524+idx` = `MP_announcer_allies_win`; the same for `MP_announcer_bomb_planted/defused`, `MP_announcer_round_draw`. Sounds addressed to the followed player are **not** forwarded. Client: `CG_LocalSound` (section 5), alias channel `announcer` = 2D |
| `musicPlay(alias)` | `0x5eb5c` | `trap_SendServerCommand(-1, 1, va("o %s", alias))` (`0x77510`) to everyone | yes | none expected; no stock MP gametype or map script calls it (grep of `sd.gsc`, `tdm.gsc`, `_utility.gsc`, `mp_carentan.gsc`). Client `o`: pick + syscall `0xd4` |
| `musicStop([fade])` | `0x5eb8c` | `p %i` (`0x77563`), the fade `* 1000 + 0.5` (`0x78af0`, `0x78af4`) truncated, 0 with no argument; a negative result is the script error `"musicStop: fade time must be >= 0"` (`0x77540`), a second argument `"USAGE: musicStop([fadetime]);"` (`0x77520`) | yes | none expected. Client `p`: syscall `0xd5` |
| `soundFade(volume [, time])` | `0x5ec40` | `trap_SendServerCommand(-1, 1, va("q %f %i\n", volume, ms))` (`0x77568`, the newline is in the format); `ms` is `time * 1000` (`0x78af8`) truncated (`fldcw` with `0xc00`), 0 with one argument; nothing is range-checked | yes | none expected. Client `q`: `FUN_30031fc0` -> syscall `0xd7` (section 10, "`soundFade`") |
| quick chat (`vsay` / `vsay_team` client commands) | `ClientCommand` @ `0x487ec` → `Cmd_Voice_f` (`0x476c0`) → `G_Voice` @ `0x473b8` | `trap_SendServerCommand(target, 0, va("%s %d %d %d %s %i %i %i", letter, ...))` (`0x73a4e`) to every client (or teammates via `OnSameTeam`) | yes for `vsay`; `vsay_team` only if on a team, so never | client `j`/`k`/`l` → `FUN_3002d940(0/1/2)` @ `0x3002d940`: argv 1-3 ints, argv 4 the voice id string, argv 5-7 ints; `FUN_3002d770` @ `0x3002d770` looks the id up (`FUN_3002d390`) and queues text + alias (`FUN_3002d470`). The aliases are the `american_*`/`british_*`/`russian_*`/`german_*` rows of `dialog_mp.csv` (channel `voice`, streamed, `all_mp`). The id-to-alias table source is an open item |

Announcer timer: `mp_announce_{g,a}_{twominutes,thirtyseconds}` are registered
by the cgame and played from the round clock; no server message is involved
(trigger times unread, open item 8).

### playSound makes the entity broadcast

VERIFIED, `game.mp.i386.so` 0x5d957..0x5d961: before `G_PlaySoundAlias`
the builtin loads `r.svFlags` (`ent+0xf4`), clears bit 1 (`SVF_NOCLIENT`)
and sets bit 8 (`SVF_BROADCAST`, the bit the snapshot builder sends past
the PVS, combat doc 11.1). It does this for every receiver, a player
included. INFERRED: nothing clears the bit again before the slot is freed,
since `ClientSpawn` only ORs `SVF_NOCLIENT` in (0x42746) and
`ClientConnect` is what stores 0x200 over the whole word (0x4258f).

VERIFIED, live 2026-10-12, `client-probes/probe_originsound` on
`mp_carentan` dm with a plain probe spectator at the intermission spot.
The probe spawned three `script_origin`s at 1 s (entities 170, 171, 172).
170 was at (6000, 6000, 3000), out of every PVS, and was sounded at 8 s and
10 s. 171 was beside the probe and was sounded at 12 s. 172 was beside the
probe and never sounded. Results:
- 170 entered the snapshot on the frame of its first sound (8050), with
  `eType` 8, `eventSequence` 1, `events[0]` 172 and `eventParms[0]` 4.
- The second sound made it sequence 2.
- A later `origin` write moved it on the wire.
- It stayed in every snapshot to the end of the run.
- 171 entered at 12050 the same way. 172 never rode a snapshot.

Before this, vcod never sent a `script_origin` or a trigger. Now
`GEntity::broadcast` is set by `playSound`; `crate::game::wire` builds
either kind once it is set; and the snapshot pass moves every broadcast
entity, a player's included, into the unculled set beside the missiles.
The same probe against vcod-server printed the same lines.

VERIFIED, live 2026-10-12, `client-probes/probe_playersound` on
`mp_carentan` under `tools/run_probe.sh`, two `--probe-edge watch` clients
spawned by the probe at (976, -864, 0) (the viewer, slot 0) and
(-992, 2328, 0) (the sounder, slot 1), out of each other's PVS. This is
the S&D planter's case, sd.gsc's `other playsound("MP_bomb_plant")`. Each
step ran 3 s apart; the viewer's entity list changed on these frames:
- quiet: entity 1 absent.
- `self playSound("MP_bomb_plant")`: entity 1 (`eType` 1) entered on the
  step's frame (19550).
- `setOrigin` 64 units away: still there.
- `suicide()`: gone on the step's frame. No corpse rode either.
- `spawn` back as a player, no new sound: back on the step's frame.
- `sessionstate "spectator"`: gone. Back as a player: back.
- `map_restart(true)`: gone from the first snapshot after the restart and
  never back while both stayed in the world.

INFERRED from the asm: the death and spectator frames hide it through
`SVF_NOCLIENT`, which `ClientEndFrame` sets for those session states
(0x40ef0, 0x41083) and `SpectatorClientEndFrame` (0x40778), and which the
cull reads before `SVF_BROADCAST`; the restart clears it because
`SV_MapRestart` reruns `ClientConnect`, which stores 0x200 over the whole
word (0x4258f). So an S&D planter rides every client's snapshot from the
plant's first frame until the round's `map_restart`, alive or not.
vcod-server gave the same list on the same frames;
`a_sounded_player_rides_every_snapshot_until_the_restart` pins it.

1.5 and United Offensive differ. Their `playSound` writes
`r.broadcastTime` = level time + 300 and leaves `svFlags` alone,
`playLoopSound` writes -1 (for as long as it loops) and `stopLoopSound`
the level time + 300 again; the snapshot builder sends an entity whose
time is negative or not yet passed (VERIFIED decompile, addresses in
`cod15-protocol-diff.md` 15). So there a sounded entity rides 300 ms, a
looping one for good, and a `script_origin` never. VERIFIED live, UO
1.51b on mp_berlin, `client-probes/probe_uocull`: a parked Panzer out of
the viewer's PVS rode the seven snapshots from the sound's frame to 300
ms past it; a `script_origin` sounded beside it never came; mp_berlin's
looping fire script models rode every snapshot. vcod-server keeps
`GEntity::broadcast` for 1.1 and `GEntity::broadcast_time` for the other
two (`uocull_ab`).

### Minefields

The stock minefield scripts, read out of the paks (VERIFIED). 1.1's
`maps/MP/_minefields.gsc` (`pak5.pk3`, 904 bytes) has the player play
both sounds: `self playsound("minefield_click")`, a 0.5 to 1 s wait, then,
still touching the trigger, `self playsound("explo_mine")`, the
`fx/impacts/newimps/minefield.efx` effect and `radiusDamage(origin, 300,
2000, 50)` on the same frame. 1.5's (`pak8.pk3`, 1088 bytes) and UO's
(`pakuo01.pk3`, 1978 bytes) keep the click on the player but play the
explosion through `level thread playSoundinSpace("explo_mine", org)`:
`spawn("script_model", origin)`, `wait 0.05`, `playsound`, `wait 6`,
`delete`. Their `radiusDamage` range is 4, UO's vehicle path 25. The
aliases are `iw_sound.csv` rows: `minefield_click` is `auto`,
`explo_mine` is `voice`, 750/6000, volume 1.5. The effect carries no
sound.

VERIFIED, live 2026-10-15, `client-probes/probe_minefield` on mp_pavlov
with two plain net probes, a victim and a watcher 400 units away:

- 1.1, the stock script (`probe_field 1`, the victim put inside `*5`):
  the victim's playerstate ring carries the click, then on the death
  frame `EV_LANDING` (the fall from the placement), `explo_mine` and
  `EV_DEATH`. The watcher gets the click on entity 1 and never the
  explosion: entity 1 leaves its snapshots on the death frame, as on
  the `suicide()` step above, and the corpse (entity 64) enters with
  neither sound in its ring. INFERRED from those lines: on 1.1 only the
  dying player hears a minefield go off.
- 1.5 and UO, the same placement: the watcher and the victim both get
  `explo_mine` on a new entity (300) one frame after the death frame.
- `probe_model 1` on 1.1: a `script_model` spawned with no model rides
  every snapshot in the PVS from its spawn frame, `eType` 8, `index` 0,
  no other field set but its origin, until its `delete`. Its
  `playSound` reaches the watcher on it. UO sends the same entity the
  same way.

vcod-server sent no `script_model` without a model before 2026-10-15, so
on a 1.5 or UO server nobody heard a minefield explode (issue #4).
`crate::game::wire` now sends it with `index` 0; the three runs above
give the same lines against vcod-server, and
`a_modelless_script_model_is_on_the_wire_and_sounds` pins it.

### Live verification

Captured with `--net-probe`, which prints CS 3, the whole `CS_SOUNDS` block at
gamestate, every `d` update inside the block, `EV_SOUND_ALIAS` out of both
event rings, `loopSound` transitions, and every server command the client does
not consume (`s <idx>` with its alias resolved). The probe logs are not
committed.

| Capture | Date | Server | Map / gametype | Players | Length |
|---|---|---|---|---|---|
| A | 2026-08-25 | a public S&D server | `mp_harbor` / `sd`, ~5 rounds | ~5 | 300 s |
| B | 2026-08-25 | a public TDM server | `mp_harbor` then `mp_depot` / `tdm` | ~15 | 103 s (dropped on the map change) plus a second 300 s run |

Both are 1.1 servers running mods, so a row that reads "not observed" may mean
the mod's gsc does not make that call, not that the wire path is wrong.

| gsc | Result | Evidence |
|---|---|---|
| `ambientPlay` | **VERIFIED live** | Capture A gamestate: `cs 3 (ambient): "n\\ambient_mp_harbor\\t\\1498539900"`. Re-sent as `d 3` on every round restart (5 times in 300 s) with the same alias and a new `t`, e.g. `...\\t\\1498566000`. Capture B: `n\\ambient_mp_harbor\\t\\1003953550` |
| `ambientStop` | not observed | CS 3 always carried an `n` key in both captures |
| `<ent> playSound` | **VERIFIED live** | 29 `EV_SOUND_ALIAS` in capture A. Plant: `EV_SOUND_ALIAS entity 2 clientNum 2 parm 40 (cs 564) = "MP_bomb_plant" at [-9171,-6495,0]`. It rides the **planter's own client entity**, which is the `ps.events` -> `es.event` copy the table above calls inferred, now confirmed from a third-party observer. Also `MP_bomb_defuse` (parm 43, on the defuser) and `generic_pain_german_1/3`. No event on a non-player entity in this capture: the mod's `sd.gsc` does not call `Explo_plant_no_tick`, so that half of the row is still unverified |
| `<ent> playLoopSound` | **VERIFIED live** | One second after each plant: `entity 263 loopSound 0 -> 6 (cs 530) = "bomb_tick" at [-9191,-6360,0]`. The bomb is a fresh entity number per round (263, 263, 223), and the loop **stop was never seen as `loopSound -> 0`**: the entity leaves the snapshot at defuse/round end instead, so a client has to silence a loop when its entity disappears, not only when the field clears |
| `<player> playLocalSound` | **VERIFIED live**, spectator forwarding included | Capture A, addressed to our spectator: `serverCommand: ["s", "42"]` -> `cs 566 = "MP_announcer_bomb_planted"`, and `["s", "11"]` -> `cs 535 = "MP_announcer_allies_win"` (3x). Consistent with the `getentarray("player","classname")` expectation in the table above, though the mod's `sd.gsc` was not read, so which enumeration it used stays an inference. Capture B also shows the **lazy alias registration**: `d 528` creating `MP_announcer_allies_win` immediately before `s 4`, i.e. `G_SoundAliasIndex` filling the first empty slot at first use |
| `musicPlay` / `musicStop` / `soundFade` | not observed live; **VERIFIED on the local retail server** | No `o`, `p` or `q` server command in either capture, as predicted. `client-probes/probe_soundfade.gsc` on the retail 1.1d server (2026-10-13, mp_carentan, one probe client) delivered `o codtheme`, `q 0.200000 3000`, `q 1.000000 2000`, `p 10000`, `q 0.000000 2000`, `q 1.000000 0`, each `q` ending in a newline and `o`/`p` not; vcod's server sends the same bytes |
| quick chat (`j`/`k`/`l`) | not observed; this mod uses `playSound` instead | No `j`/`k`/`l` command arrived. The voice lines came through as `EV_SOUND_ALIAS` with `russian_*`/`german_*` alias names, e.g. `EV_SOUND_ALIAS entity 2 clientNum 2 parm 17 (cs 541) = "russian_suppressing_fire" at [-7607,-7244,0]` (16 of them), plus `russian_yes_sir`, `russian_great_shot`, `russian_enemy_down`, `russian_need_reinforcements`. Stock `G_Voice` stays unverified live |

The alias block is filled lazily and per server script, not preloaded. Capture
A had 60 of the 256 slots set at gamestate (`bomb_tick`, `MP_announcer_*`, the
`generic_pain_*`/`generic_death_*` set, the quick-chat lines), capture B only
3 (`world_hurt_me`, `MP_hit_alert`, `bullet_impact_headshot`). A client has to
resolve `CS_SOUNDS + idx` at play time and follow `d` updates inside the
block, never snapshot it at connect.

---

## 9a. Footstep and landing cadence (`game.mp.i386.so`, pmove side)

Everything here is read off the disassembly (`objdump` of the .so, virtual addresses ==
file offsets); none of it is live-verified against a running server, because a
footstep event is indistinguishable in a spectator capture from any other
entity event of the same id. The client-side half (which alias each id plays)
is section 7b.

The exported name `PM_ShouldMakeFootsteps` (@0x3221c) belongs to the *audibility
predicate*, not the driver: effective stance prone or crouched -> 0, walk key
(pm_flags bit 0x80) set -> 0, otherwise 1. Its result gates the grounded step
event only; water steps bypass it and the ladder branch hardcodes audible.
The driver is the next function (@0x322c8-0x328ca), called once per
`PmoveSingle` on the normal path; I call it `PM_Footsteps` below.

**State.** The phase counter is `ps->bobCycle` (ps+0x8, networked byte). Each
frame: `new = (int)(old + pml.msec * rate) & 0xff`, truncated toward zero
(`fistp` under control word `| 0xc00`, 0x32806-0x32819; VERIFIED), and one
step event fires per crossing of the `(old + 64) ^ (new + 64)` sign
boundary - i.e. at phase 64 and 192, twice per 256-tick ring, never at the
wrap itself. An earlier read here said rounded to nearest; Ghidra prints the
truncating store as `ROUND`.

**Rate** (ticks/ms). `rate = xyspeed / W * K` (0x323f0-0x327cc, VERIFIED
constants): `W` is `ps->speed` (190) times the keys' factor (a lone strafe
`1 + 0.75 * (strafeSpeedScale - 1)`, a lone back key `backSpeedScale`, both
keys `0.5 * (2 + 0.75 * (strafe - 1))` and for a back diagonal also
`0.5 * (1 + back)`; rodata 0x70c20 0.75, 0x70c24 0.5), the walk or run scale
by `pm_flags` 0x80, and the stance's speed scale, blended across an eye leg
between crouch and prone (0x308cc, the walk speed's own blend); no weapon or
wade factor. At full speed `xyspeed / W` is 1 and the rate is bare K; from a
stop it climbs with the speed, which is where an accelerating start's phase
comes from. VERIFIED, vcod measurement (2026-10-13, `moves_ab.rs`): with this
rate and the truncation the predicted `bobCycle` matches every snapshot of
both moves captures (146 and 207 misses before), and `turret_ab` matches the
strafe's footstep events.

| stance | walking | K | backpedal only affects stand |
|---|---|---|---|
| stand | no | 0.335 | 0.36 |
| stand | yes | 0.305 | 0.325 |
| crouch | no | 0.34 | - |
| crouch | yes | 0.315 | - |
| prone | no | 0.25 | - |
| prone | yes | 0.24 | - |

(rodata 0x70c28-0x70c44, VERIFIED.) "Backpedal" is pm_flags bit 0x40, set by
`PmoveSingle` when `cmd.forwardmove < 0`. The stance is `PM_GetEffectiveStance`'s
(inlined, the flags and the eye's leg), and "walking" is `pm_flags` 0x80, the
ADS walk (`PM_UpdatePlayerWalkingFlag`), INFERRED from the reads at 0x32490.
At a full run with fixed 8 ms frames the 2.68-tick advance truncates to 2, a
step every 512 ms (INFERRED from the formula).

**Gates**, in driver order:

1. `pm_type > 5` produces nothing.
2. Airborne without the ladder flag: nothing (the cycle freezes).
3. Airborne on a ladder: interval `vz / 95.25 * k` with k = 0.35 walking /
   0.45 running (rodata 0x70c10-18), same crossing rule, audible regardless of
   the predicate. The surface comes from a probe box (mins/maxs shrunk 6 per
   side, z floored at 8) swept from the feet along `-vLadderVec * 31`; a miss
   or material 0 substitutes **material 13 (metal)**, and the event is always
   run-numbered (`1 + mat`) (@0x32039-0x3214c). There is also a 299 ms quiet
   window after `ps->jumpTime` is stamped (ladder push-off).
4. Grounded under speed 10: nothing; under speed 1 the cycle resets to 0,
   between 1 and 10 (a creep) it keeps its phase (`fld1; fcompp` at
   0x324af-0x324ba, VERIFIED; an earlier read here had the two the other way
   round).
5. Grounded at speed >= 10: timer advances always; the step event needs move
   keys held (`cmd.forwardmove | cmd.rightmove != 0` as a word test @0x3282a)
   **and** the predicate. Keys-released glide updates the timer and plays no
   events, whatever the speed (@0x32831-0x328c1).

**Event choice** (`PM_FootstepEvent` @0x31fe4), first match wins:

- water level 1 or 2: fixed ids 67 / 44 / 21 (prone / walk-key / run; base +
  material 20), bypassing the predicate; level 3: nothing (@0x3200a, 0x321b0).
- ground trace from the movement code (`pml + 0x50`): `sf & 0x2000` or
  material 0 -> EV_NONE; else `base + mat` with bases run 1, walk 24 (walk
  key), prone 47 (@0x32156-0x321a3).

**Jump**: every jump, ground or ladder push-off, is `PM_CheckJump` (0x2eb98,
`cod11-mantle.md`, "Jumps"), and every one emits `EV_JUMP_*` (70 + mat from
the then-current ground-trace flags; material 0 or `sf & 0x2000` -> nothing)
(@0x2ed90-0x2edb9). VERIFIED on the wire: the motion captures' takeoffs add
75 (carentan) and 74 (pavlov). An earlier read here said the ground jump
emits none; it was reading the prone dive at 0x31CC0.

**Landing** (`PM_CrashLand` @0x2fd68, called by the ground-trace helper on an
air -> ground transition): water level 3 skips; damage-free impact ladder on
the fall height: <= 4 silent, < 8 a walk-step id (24 + mat), < 12 a run-step
id (1 + mat), >= 12 an `EV_LANDING_*` (93 + mat) plus a x0.67 velocity damp;
`sf & 0x2000`/material 0 silence the two step ids but play the land event on
material 0 (`cod11-player-clip.md` 8.6). Damage routes to
`EV_LANDING_PAIN_*` (116 + mat, parm = damage percent) instead - sound-wise
that alias pair resolves through section 7b like the rest.

VERIFIED, `game.mp.i386.so`: the function solves the landing speed off
`pml.previous_velocity[2]` (pml+0x7c) and `pml.previous_origin[2]`
(0x2fdb4..0x2fe15), squares it and divides it by twice `ps.gravity`
(ps+0x3c, 0x2fe1e..0x2fe2e), and hands both to the debug print `"landing vel:
%.1f fall height: %.1f\n"` (string 0x70940, pushed at 0x2fe49). VERIFIED: the
ladder's compares are `fcom` against 4.0 (0x70a10, at 0x30130), 8.0 (0x70a3c,
0x30141) and 12.0 (0x70a18, 0x30170). INFERRED: the value the ladder compares
is that fall height in units, not the speed, so a one-unit drop (a turret
release, `cod11-turrets.md` 12.7) lands in silence where a 32-unit one plays
the landing. An earlier read of this paragraph said "landing speed", and vcod
compared a speed in units a second against the 4/8/12 until 2026-09-25.

VERIFIED, the anim at the head of the function: a compare of `ps+0x6c`
(`legsTimer`) against 0 (0x2fd84), an `fcomp` of -220.0 (rodata 0x70a08)
against `pml.previous_velocity[2]` (0x2fd8a..0x2fd9b), and a call to
`BG_AnimScriptEvent(ps, 5, 0, 1)` (0x2fd9d..0x2fda4). INFERRED: the land anim
(event 5) plays only when the legs timer has run out and the move began
falling faster than 220 units a second; any slower landing keeps whatever the
legs were playing. VERIFIED on the wire: the turret capture's two releases
each end in a one-unit drop and go from `standMG42_aim` straight to
`pb_stand_alert` with no land anim between, the stand release at fixture
lines 1274-1281 and the crouch release at 1424-1436 (`cod11-turrets.md`
12.7).

Other movement emitters seen while in there, for completeness: stair-step 143
(parm = clamped step delta + 128), foliage rustle 139 (cvar-driven interval
trace @0x328cc), water enter/leave 144/145, forced stance 140/141/142.

### vcod port notes and divergences

- Implemented in `pmove.rs` (`footsteps`, `ground_step_event`,
  `ladder_step_event`, `landing_event`), emitting wire ids straight into the
  shared cue resolver; `collision::sound_material` extracts `(sf >> 20) &
  0x1f`, now threaded through triangle hits as well as brushes.
- The rate uses bare K (see weight collapse above); `ps.speed` int truncation
  is not replicated.
- Stances are instant in vcod, so the viewheight-lerp classification and the
  transition blend between speed scales have no equivalent.
- Every jump's `EV_JUMP_*` event reads the last ground trace's material like
  retail (`pmove::check_jump`), which on a ladder means push-offs from the
  base are audible and mid-wall push-offs are not (the downward trace of a
  climb hits nothing). The 299 ms quiet
  window after a push-off gates the climb steps.
- The fall height is retail's kinematic value off the move's start
  (`cod11-player-clip.md` 8.2). The land anim is gated in
  `ClientSim::update_anims` on `PlayerState::land_anim` (the move's starting
  vertical speed below -220) and on no event anim holding the legs.
- `pmove::crash_land` is all of `PM_CrashLand`'s pmove half: the damage
  percent, the landing stun, the x0.67 damp (the prone dive measured it,
  `cod11-mantle.md`, "The landing damp"), the land event's view-bob parm and
  `EV_LANDING_PAIN_*` with the damage as its parm (`cod11-player-clip.md`
  8). The server applies no fall damage.
- Foliage rustle (139) is not ported.

### Ambient without a server

Local fly/walk modes derive the ambient track offline: every stock MP map has
an `ambient_<mapname>` row in `soundaliases/iw_sound.csv` (all twelve verified
in pak1.pk3), so vcod plays `ambient_<map>` at load and stays silent when a
custom map has no such row. This mirrors what a gametype script's
`ambientPlay` would have put in configstring 3.

---

## 10. Streamed playback

`VERIFIED`. `type=streamed` changes loading (no decode at map load; only a
file-exists check, `FUN_00433c20`, warning `"streamed sound 'sound/%s' not
found"`) and the voice pool (a Miles stream, `FUN_0044cf50` @ `0x44cf50`,
opened from `sound/<file>` on every play, `AIL_open_stream`). It does **not**
change spatialization; a streamed alias on a 3D channel stores its origin
(`DAT_008e18a0 + slot*0x10`) with the positional flag (`DAT_008e189c`) set for
channels other than 1 and 6..9, and `FUN_0044f100` @ `0x44f100` updates its
volume (same linear falloff) and pan (section 3 formula) every frame; it is
entity-tracked like a 3D sample (`FUN_0044bed0`). A streamed alias on a 2D
channel is a plain stream at pan `0.5`. Streams are subject to the same
replacement and master/slave rules. Only the stream pool is different: 8
general slots (section 2), and a stream whose file is not found at load time is
refused at play time (`"Tried to play streamed sound '%s' from alias '%s', but it
was not found at load time.\n"`).

Every `dialog_mp.csv` row and every `ambient_mp_*` row is streamed; the
announcer lines are 2D (`announcer`), the quick-chat lines are 3D (`voice`),
the ambients are 2D loops (`local`).

### Mixer rate and `snd_restart`

- A stream mixes at `mss_khz`'s rate like a loaded sample. VERIFIED,
  `FUN_0044cf50` @ `0x44cfd9`: `AIL_open_stream` (IAT `0x53d394`) takes
  the digital driver at `0x8e0760`, the one `FUN_0044ba30` opens at 11025,
  22050 or 44100 Hz from `mss_khz` (`AIL_open_digital_driver`, the only
  call). The stream's own rate is read back (`AIL_stream_playback_rate`,
  `0x44d00f`) and only scaled by the alias pitch, so Miles converts the file
  to the driver's rate. VERIFIED, the files (`ffprobe` over every
  `type=streamed` row in the stock pk3s): 28 of the 29 `music` files and 29
  of the 30 `local` ones (the ambients) are 44100 Hz; the dialog and
  quick-chat lines are almost all 22050 Hz. So `mss_khz` 22 or 11 reaches
  music and ambients.
- `snd_restart` (`0x40fe0c`) saves the stream slots, restarts Miles and
  restores them. VERIFIED, the calls: save `0x4510f0` at `0x40fe2f`, shut
  down `0x450130` at `0x40fe93`, start `0x44fe60`, `vid_restart` `0x40fbe0`,
  restore `0x451330` at `0x40fea7`. The per-slot save `FUN_00450c80`
  returns without writing for slots `0x21` and `0x22` (`0x450ca7`), the
  two ambient crossfade slots; every other playing stream (music slot 0,
  slots 3-4, the general slots) is written with its alias, entity, origin,
  volume, pitch and play position as a fraction of its length
  (`AIL_stream_ms_position`). The restore loop at `0x451520` skips slots 1
  and 2 and reopens the rest through `FUN_00450ea0`, which calls
  `FUN_0044cf50` with that fraction as its start (`param_8`), so they go
  on where they were. VERIFIED, the decompilation.
- The ambient comes back from the top. INFERRED: `vid_restart` restarts
  cgame, whose `CG_Init` replays configstring 3 (section 9) through
  syscall `0xd6`; `FUN_0044da00` @ `0x44db24` passes a start of 0 to
  `FUN_0044cf50`. The main menu's `music_mainmenu` also starts over:
  with no game up `SCR_DrawScreenField` stops every sound before it
  reopens the main menu (front-end doc, section 1), whose `onOpen` plays
  it again. INFERRED from that order.
- The save (`0x4510f0`) also writes the ten channel volume ramps
  (`0x8e079c`, 30 dwords), `0x8e0840..0x8e084c`, and the (target, per-ms
  rate) pairs of the music slot (`0x8e0814`, `0x8e0818`) and slots 3 and 4
  (`0x8e082c`..`0x8e0838`); the restore (`0x451330`) writes them back.
  The ambient pairs (`0x8e081c`..`0x8e0828`) are skipped. VERIFIED, both
  decompilations. A stream's saved volume is its Miles volume divided by
  the master (`0x8e078c`), or the slot's own volume when the master is 0
  (`0x450c80`); `FUN_00450ea0` reopens the stream at that volume
  (`FUN_0044cf50`'s fifth argument). VERIFIED. So a `musicStop` fade goes
  on from where it was. INFERRED: the music slot's ramp (`0x44e7a0`) reads
  the restored pair on the next update.
- The master fade (`0x8e0790..0x8e0798`, `soundFade`) is not in the save,
  which starts one dword past it, and `S_Init` (`0x44fe60`, at `0x450029`)
  writes 1, 1, 0 into it, so `snd_restart` ends a fade at full volume.
  VERIFIED, the save's range and the three stores.
- vcod: a stream goes through `audio/stream.rs`, which takes a file above
  the mixer rate down to it chunk by chunk with the linear interpolation
  the bank uses for loaded files. `snd_restart` restarts the ambient from
  the top, as above, and reopens every other stream (dialog, quick chat,
  `musicPlay`, a stream an in-game menu played) with its picked row, its
  current volume and its pitch at kira's playback position in seconds,
  which `stream::Resampled` keeps on the file's timeline at any mixer rate.
  Retail saves a fraction of the length; for the same file the two land on
  the same spot. A `musicStop` fade goes on at its rate, and `soundFade`'s
  master fade is back at full. With no game up nothing is saved: the main
  menu stops every sound and replays its music, as above. Loaded
  (non-streamed) sounds end, as in retail, which saves only stream slots.

### `musicPlay` and `musicStop`

- `CG_ServerCommand` (cgame `0x3002e0d0`) case `o` picks the alias
  through syscall `0xc0` and hands the record to syscall `0xd4`; case `p`
  passes `atol(argv1)` to syscall `0xd5`. VERIFIED, the decompilation.
- Syscall `0xd4` (CoDMP.exe `0x4027a0` loads the record into EDI,
  `0x44dc40`): with a digital driver open and a non-null record, it asks `FUN_0044c6d0` with ESI = `0x20` whether
  the music slot is free, and only then calls `FUN_0044da00(0, 0)`, the
  same opener the ambient uses, into slot `0x20` with no fade. VERIFIED,
  the disassembly at `0x44dc4e`-`0x44dc62`. `FUN_0044c6d0` reads a slot as
  free when it holds no stream, or when its fade flag (`0x8e08c4 +
  slot * 0x38`) is clear and `AIL_stream_status` reports done (2), closing
  that stream. VERIFIED. So a second `musicPlay` while a track plays, or
  while it fades out, is dropped. INFERRED from that check.
- `FUN_0044da00` warns but still plays a record that is not streamed
  (`"alias %s sound %s played as an ambient / music track is not
  streamed; type must be 'streamed'"`, `0x55f5e9`) or that sits on a 3D
  channel, i.e. not 1 or 6..9 (`"... uses a 3D channel type; should
  probably be channel 'local'"`, `0x55f649`). VERIFIED.
- Syscall `0xd5` (`0x4027ad` pushes the fade with EDI = 0, so slot `0x20`,
  `0x44db90`): a done stream with a clear fade flag is closed; a fade of
  0 closes the stream at once; otherwise, while the slot's target volume
  (`0x8e0814`) is above `0x568e64` (0.0), the target goes to 0 and the
  per-ms rate (`0x8e0818`) to `-target / fade`. VERIFIED. A second `p N`
  during a fade finds the target at 0 and changes nothing. INFERRED from
  that check.
- `FUN_0044e7a0` @ `0x44e7a0`, called for slots `0x20`..`0x24` from the
  stream update (`0x44f100`) every frame: the slot's volume (`0x8e08a0 +
  slot * 0x38`) moves by `rate * msec`; with a rate of 0 or less, a volume
  below the target is set to the target and, when that is 0, the stream is
  closed and the update skipped; with a positive rate it is capped at the
  target. VERIFIED, the decompilation. The volume then feeds the Miles
  volume through the master (`0x44f100`). VERIFIED.
- vcod: `o` plays on a reserved entity (`cues::MUSIC_ENTITY`) that no cue
  replaces and the stream pool does not steal, only while the last track's
  voice is gone; `p 0` stops it, `p N` ramps the voice's volume down at
  `-volume / N` per ms of wall time and stops it once below 0, and a
  second `p N` leaves a ramp alone. A negative fade (the server refuses to
  send one) stops at once. Stock MP scripts never send `o` or `p`
  (section 9).

### `soundFade`

- `CG_ServerCommand` (cgame `0x3002e0d0`) case `q` passes `atol(argv2)`
  in ECX and `atof(argv1)` as a float to `FUN_30031fc0`, which calls
  syscall `0xd7` with (volume, ms). VERIFIED, the disassembly at
  `0x3002e367`..`0x3002e391` and `0x30031fc0`. Nothing else in cgame
  reaches `0xd7`. VERIFIED, the only call to `0x30031fc0`.
- Syscall `0xd7` (`0x4027d4`) = `FUN_0044e0f0`, the master fade: target
  (`0x8e0794`) = volume, rate (`0x8e0798`) = `(volume - level) / ms`, with
  the level at `0x8e0790`. A time of 0 sets the rate to the whole
  difference, and a volume of 0 then calls `FUN_0044fa40(0)`, which ends
  every sample, 3D sample and stream, the ambient and the music included.
  VERIFIED, the decompilation of both.
- `S_Update` (`0x44f9cc`) calls `FUN_0044e890` with the milliseconds since
  the last update off the wall clock. VERIFIED, the call; the clock source
  (`0x53d31c`) is INFERRED to be `timeGetTime`. While the rate is not 0,
  `level += rate * msec`, clamped to the target in the rate's direction,
  where the rate goes back to 0; a level of 0 with a rate of 0 then calls
  `FUN_0044fa40(0)` again. With a rate of 0 the update returns early unless
  the volume cvar changed. Then the master `0x8e078c` = clamp(cvar, 0, 1)
  `* level`. VERIFIED, `0x44ec28`..`0x44ed52`. The level itself is never
  clamped, so `q 2 0` doubles the master (INFERRED: Miles' handling of a
  volume above 1 is unread).
- The master multiplies every voice: 3D samples (`0x44eea0`), 2D samples
  (`0x44f070`), streams (`0x44f100`, `0x44cf50`) and the cinematic sample
  stream (`0x44dd90`, `0x44df40`). VERIFIED. The per-frame loops call
  those for every live voice (`0x44f258`, `0x44f2c5`, `0x44f31c`), so a
  fade reaches sounds already playing. VERIFIED.
- Sounds started after a fade to 0 play at volume 0 until a `q` raises
  the level. Writers of the fade are `0x44e0f0` (from cgame `0xd7` and UI
  syscall 40, `0x4186a1`), `S_Init` (1, 1, 0), the function that opens
  the cinematic handle `0x57c118` (`0x4081de`, rate `1 - level`, target 1;
  INFERRED to be the cinematic command) and `CL_PlayLogo_f` (`0x411ce3`,
  the same; its name string `0x566b20`), and `0x40ee60`, reached only from `SV_SpawnServer`
  (`0x458860`), which sets target 0 and rate `-level` and stops every
  sound. VERIFIED, every store to `0x8e0790..0x8e0798` in the disassembly.
  So a remote client's fade survives map changes and disconnects. INFERRED
  from that list. `ui_mp_x86.dll`'s wrapper for syscall 40 (`0x400193f0`)
  has no caller. VERIFIED, no call, jump or pointer to it.
- vcod: `q` sets the same target and rate; `AudioSystem::step` advances the
  level by the wall milliseconds since the last step and multiplies it into
  the master gain of every voice. A landing on 0, or `q 0 0`, stops every
  sound. `snd_restart` puts the fade back at full. The server's
  `soundFade`, `musicPlay` and `musicStop` send retail's text to every
  client (`crates/server/src/game/builtins/sound.rs`).

---

## 11. Doppler and occlusion: absent in 1.1 MP

Both questions were asked because vcod had neither and the README lists them
as gaps. The binary answers "retail had neither" for doppler, and "not in the
sound path" for occlusion.

### 11a. Doppler: no velocity ever reaches Miles

`VERIFIED` (import surface). The complete set of `_AIL_*` imports of
`CoDMP.exe` contains every spatial API the engine uses —
`AIL_set_3D_position_16`, `AIL_set_3D_sample_distances_12`,
`AIL_set_3D_distance_factor_8`, `AIL_set_3D_sample_volume_8`,
`AIL_set_3D_sample_playback_rate_8`, `AIL_set_3D_room_type_8` — and **no
velocity or doppler entry point**: there is no
`AIL_set_3D_sample_velocity`, no `AIL_set_3D_doppler_factor`, nothing with
velocity in its name. MSS derives doppler solely from emitter/listener
velocities; `S_Respatialize` (`FUN_0044bd60`, section 3) stores origin and
axis only. A sound therefore never shifts pitch with relative motion in
retail, and vcod builds no doppler.

### 11b. No occlusion term in the audio paths

The per-frame gain for every voice class is read out in full in sections 2-4
(`FUN_0044cb90` start, `FUN_0044eea0` 3D samples, `FUN_0044f070` 2D samples,
`FUN_0044f100` streams): gain = `s_volume * channelVol * scale * v` with
`scale` pure distance falloff, plus the master/slave cap. No ray test, no
wall factor, no lowpass appears anywhere in those paths or in the start path
`FUN_0044d670`. Retail 1.1 MP plays a sound behind a wall at the same gain
as one in the open.

### 11c. What syscall `0xd0` actually is

`VERIFIED` (decompilation), closing the "(trace-based check, purpose
unknown)" note under the syscall table. Syscall `0xd0` stores cgame's
vieworg at `DAT_0143a988..990`; the only reader is `FUN_00414ef0` @
`0x414ef0`: a six-ray line-of-sight test from that stored listener position
to a register-passed entity record's origin (`+0x18..0x20`). It traces via
`FUN_00424530` to six points around the entity's upper body and returns 1
only when **all** rays are blocked:

| Ray | Target | Constants |
|---|---|---|
| 1 | centre, z +16 (or +40 when `+0xe0 == 0`) | `_DAT_00568e64`=0, `local_70` |
| 2 | centre, z +32 (or +56) | `_DAT_00568ec8` = 16 |
| 3 | centre, z −24 (or +0) | second `_DAT_00568ec8`, `local_70` again |
| 4 | xy ± perp·18, z + (−24 or 0) + 8 | `_DAT_00569114` = 18, `_DAT_00568ef4` = 8 |
| 5 | xy ∓ other-perp·10, z + (−24 or 0) + 52 (or 28) | `_DAT_00569118` = 10, `_DAT_00569110`=28 / `_DAT_0056910c`=52 |
| 6 | mirrored side, z + (−24 or 0) + 36 (or 16) | `_DAT_00568e9c` = −1.0 mirrors the offsets; `_DAT_00569108` = 36 |

The `+0xe0` flag selects between two height sets, so the fan covers head and
torso for either stance. Its two callers are both snapshot delta encoders:
`FUN_00415350 @ 0x415350` (single entity, called for "baseline" and "delta"
entities) and `FUN_00415460 @ 0x415460` ("unchanged" carry-over loop; debug
strings `%3i: baseline: %i`, `delta`, `unchanged`, `Entities in packet: %i`).
The gate is `DAT_015ce990 != 0 && entnum < 0x40` — players only, behind a
numeric option parsed into `DAT_015ce990` from a command handler near
`0x410a4d`. On the hidden verdict the encoder sets bit `0x100` of the
entity-state word at record `+0x08` (the `eFlags` slot); on any visible ray
it clears the bit. `DAT_01617360[entnum]` timestamps the last visible test,
and a newly-hidden entity keeps its old bit until it has stayed hidden for
over 600 ms (`now - 600 <= last_seen` skips the update) — hysteresis against
flicker.

So the flag rides the network stream on player entities; what any consumer
does with `eFlags & 0x100` is an open item (below). What matters here: the
mechanism never touches a gain, a pool decision or the mix, and it exists in
the listen-server snapshot writer, not the sound system.

---

## 12. Module md5s and image bases

Same conventions as `cod11-events-and-fx.md`:

| File | md5 | Base |
|---|---|---|
| `CoDMP.exe` (1.1) | `753fbcabd0fdda7f7dad3dbb29c3c008` | `0x00400000` (`.text` `0x401000`, `.rdata` `0x53d000`, `.data` `0x570000`) |
| `cgame_mp_x86.dll` (1.1) | `4912169a9eb22b404f95c52863a5feb6` | `0x30000000` |
| `game_mp_x86.dll` (1.1) | `25e2fcfe02ca0c46f4e9ad2530d50691` | `0x20000000` (not used here; the `.so` has symbols) |
| `game.mp.i386.so` (1.1d Linux dedicated server) | `de8947beb6f86fbfb46f5adfaab3d3ed` | file-relative; second `LOAD` maps file `0x7a3a0` to vaddr `0x7b3a0` (data addresses above are vaddrs) |
| `cgame_mp_x86.dll` (1.5) | `075e2af18aeaf2aeaf2a75ce22db683a` | `0x30000000` (not diffed for sound) |

---

## Where the engine differs from the original design

The original design of the audio subsystem was drafted from the csv legend and Q3/RTCW lineage before this reading of the
binaries. Each item below states what the engine does, with the section that
holds the evidence, and what the design doc or the csv legend had assumed.

1. **`dist_max` blank or 0 is `dist_min * 5`** (section 1c), so the default
   pair is `(120, 600)` and an explicit `0` is replaced after the swap and
   defaults are applied. The csv legend, and the design doc with it, read `0`
   as "no cutoff".
2. **Falloff is linear in amplitude between `dist_min` and `dist_max`**, a
   sound farther than `dist_max` at start is never started, and every alias's
   random volume carries a constant `0.8` factor (section 3). The design doc
   had left the curve open between linear and an RTCW-style fallback.
3. **`vol_max` and `pitch_max` default to `vol_min` and `pitch_min`** when
   their cell is blank (section 1c); `1.0` applies only when both cells are
   blank. The design doc had blank = `1.0`.
4. **Channel rules are not Q3's.** Replacement is by (entity, channel) and
   `auto` never replaces; looping aliases dedupe by (entity, alias name);
   voice stealing prefers lower channel indices; `menu, local, music,
   announcer, shellshock` are the only 2D channels (section 2). There is no
   per-entity channel slot table of the Q3 kind.
5. **No own-entity special case** (section 5). Sounds from the followed
   player are 3D sounds at the listener, which come out centred and at full
   volume on their own. The design doc had stated that outcome as a rule; a
   local (2D) emitter is right only for aliases whose channel is 2D and for
   the `s` command with a 2D alias.
6. **`masterslave` caps slave rows at the csv number while any master plays**
   (section 4); the level is a float gain cap, and no MP row is a master. The
   design doc had read the number as a percentage.
7. **`null.wav` picks occupy a voice** and count for the replacement rule and
   the no-repeat rule (section 1f). The design doc had treated them as no
   pick at all; the pick counter must advance even when nothing plays.
8. **The pick is weighted by `probability`**, and with three or more rows the
   last-played row is excluded (section 1e). `sequence` orders rows within a
   name group and has no other effect. The design doc had no no-repeat rule.
9. **Alias names match case-insensitively; `loadspec` matches whole words,
   `!x` and `! x` are equivalent, `all_mp` matches only as the whole cell**
   (section 1d). Every csv in every pak is merged and none overrides another.
10. **Footstep aliases** (section 7b): `EV_JUMP_*` plays `step_run_<surf>`,
    since no jump alias exists; `EV_LANDING_PAIN_*` plays `land_<surf>` plus
    `land_damage`; every footstep group also plays `gear_rattle_run` (run,
    jump) or `gear_rattle_walk` (walk, prone) at the entity, ungated by
    `cg_footsteps`. There is **no `_default` fallback**, the engine's
    `asphalt` never matches the csv's `asphault`, and **`canvas` is not a
    surface**, so the 36 `*_canvas` rows are unreachable (section 7a). The
    design doc had an extra `canvas` suffix.
11. **Fire events** (section 8): `loopFireSound` and `stopFireSound` are never
    used by the MP client; 161 uses `lastShotSound` with a `fireSound`
    fallback; 169/170 play `fireSound` twice. `EV_EJECT_BRASS` has no alias,
    only the effect. `ammoPickupSound` is dead; 148 plays the item's pickup
    alias (section 7d). The design doc had listed brass and loop-fire sounds.
12. **Pain, death, item respawn/pop and teleport have no sound** in the 1.1 MP
    client (section 7c). The design doc had listed pain and death sounds.
13. **Grenade and rocket explosions play the per-surface alias and the
    weapon's `projExplosionSound`** at `ENTITYNUM_WORLD`; bounces play
    `grenade_bounce_<surf>` only (section 7c). The design doc had this right.
14. **`EV_SOUND_ALIAS` plays on the entity at `es.pos.trBase`** and tracks the
    entity afterwards (section 9). Copies off the `ps` ring (entity `u32::MAX`
    in vcod) are the followed player's own `playSound`s and belong on that
    player's entity. The design doc had emitted them as local 2D sounds.
15. **Wire path** (section 9): `ambientPlay` is configstring 3
    (`n\alias\t\time`); `playLocalSound` is the `s <idx>` server command,
    addressed per client; `playLoopSound` is `es.loopSound`; quick chat is
    `j`/`k`/`l`. Configstrings 524..779 are `CS_SOUNDS` with index 0 unused.
    The design doc had the ambient as a server command.
16. **Streamed aliases are positional when their channel is 3D**, with the
    pan formula of section 3; only the channel decides (section 10).
17. **Positional sounds started on an entity follow it** (section 3). An
    entity-keyed emitter is the right shape; a fixed point belongs only to
    `ENTITYNUM_WORLD` starts.
18. **Whizby placement and trigger** (section 6). With
    `u = normalize(impact - start)`, `L = |impact - start|` and
    `t = (vieworg - start) . u`, the cue fires only when `64 <= t <= L - 64`
    **and** `|start + t*u - vieworg| <= 140`, and the emitter is
    `start + (t - 16) * u` (16 units back toward the shooter), as
    `ENTITYNUM_WORLD` on channel `auto`. The design doc had put the emitter at
    the closest point itself and left the radius open; the 140 radius and the
    two 64-unit end margins are that radius.
19. **Pan model.** The engine's own stream pan (section 3) is
    `pan = (1 - u . left) * 0.5`, with `u` the unit vector from listener to
    emitter and `left = viewaxis[1]` (0 = full left, 0.5 = centre, 1 = full
    right, no front/back cue). For Miles 3D samples the engine hands Miles a
    listener-space position instead and Miles pans (open item 3), so the
    stream formula is the closest thing the binary states outright, and vcod
    uses it for every voice.

### vcod divergences from retail

Places where vcod knowingly does something other than what the sections above
describe, with the reason.

- **`null.wav` picks start no voice.** Retail plays the silent file on a real
  voice (section 1f), so it can end a same-channel voice on the same entity.
  vcod advances the pick counter and starts nothing. Cost: a same-channel voice
  ends about a second later than in retail.
- **Followed-player sounds.** Retail has no own-entity rule (section 5) and
  neither does vcod: 2D is a property of the alias row's channel, and there is
  no local source variant. Events off the `ps` ring resolve to the ridden
  entity (`ps_entity`), whose body is never drawn, so `AudioSystem::step`
  injects the listener position for that entity each frame. Retail spatializes
  those sounds against the entity's interpolated origin; the two only differ if
  the camera and the entity drift apart.
- **Ducking.** Retail caps every slave voice at its `slaveLevel` before the
  `s_volume` and channel multipliers while any master plays (section 4). vcod
  applies the same cap as an absolute limit on `volume * spatial` per voice,
  with no sub-mix tracks. Stock MP data has no master row, so the
  simplification costs nothing there.
- **World-space voices and voice stealing.** Fixed-point voices
  (`Source::Point`) carry entity 1022 (`ENTITYNUM_WORLD`) for the (entity,
  channel) replacement rule, as in retail, and `auto` never replaces. Pools
  and stealing follow section 2 (32/32/8 with the `FUN_0044c350` preference
  order); kira's own track capacity only backstops. Two documented
  approximations: stream lengths are unknowable before decode, so streamed
  voices never win the earliest-end tiebreak, and the ambient is unstealable,
  extending its reserved-slot divergence below.
- **Occlusion.** Retail has none (section 11), vcod does: a zero-extent trace
  per spatial voice every few frames (staggered by voice id) multiplies the
  distance term by 0.25 when the path to the listener is blocked, approached
  over a few frames to avoid zipper noise, and disabled where no collision
  world exists (fly mode). The factor sits inside the distance scale, so the
  master/slave cap still bounds ducked voices.
- **Ambient on a reserved entity.** Retail keeps the ambient in reserved
  stream slots (section 2), outside the (entity, channel) rule. vcod plays it
  as a `local` voice on a reserved entity identity (`AMBIENT_ENTITY`) and
  restarts it if its voice vanishes. On a shared identity it would be exposed
  to replacement: `player_out_of_ammo` has a `local` row in retail data, so a
  dry-fire click from the followed player would end the ambient for the rest
  of the map. Cost: an ambient that should stop keeps playing until the alias
  changes.

---

## Open items

1. **Menu-set `loadspec` default.** `FUN_00433e90("menu", 0)` passes a stack
   slot as the loader's default flag; not pinned whether blank rows load for
   the menu set. Irrelevant to vcod.
2. **Per-channel volumes.** Closed: the shellshock's mix
   (`cod11-shellshock.md`, section 4).
3. **Miles panning law for 3D samples.** Section 3 gives the position handed
   to Miles; the actual pan curve inside the Miles 3D software provider was not
   examined. The stream pan formula `(1 - u . left) / 2` is the engine's own and
   is the safe model for both.
4. **`ambientStop` client no-op** is inferred from the null-record early return
   in `FUN_0044dc80`; not observed live.
5. **Quick-chat id to alias table**: `FUN_3002d390` @ `0x3002d390` resolves the
   voice id string without any string literal in reach; the table it walks is
   filled elsewhere (inferred: from a script file at init). Needed only when
   vcod wants to play `vsay` lines.
6. **`ps.events` to `es.event` copy** for `playSound` on a player entity is
   assumed from Q3's `BG_PlayerStateToEntityState`. Capture A showed the
   planter's `MP_bomb_plant` arriving as an entity event on the planter
   (section 9), so the behaviour is confirmed; the server code path itself is
   unread.
7. **Stream slots 3-4** remain unread. The music slot's ramp is in section
   10.
8. **Announcer timer** aliases (`mp_announce_*`) are cgame-clock driven; the
   trigger times were not read.
9. **`svFlags` bit 8** set by `playSound` is `SVF_BROADCAST`. Settled
   2026-10-12; see "playSound makes the entity broadcast" in section 9.
10. **Consumer of `eFlags & 0x100`** (section 11c): the hidden-from-host bit
    the listen-server snapshot encoder sets on player entities. Whether any
    cgame or engine code reads it back, and for what, is unread; no audio
    path reads it.
11. **Listen-server fade.** `0x40ee60`, run from `SV_SpawnServer`, fades the
    master to 0, and none of the writers in section 10 ("`soundFade`")
    brings it back on that path. Something else must, or a retail listen
    server would load silent. Irrelevant to vcod, which has no listen
    server.
