# Quick chat (`vsay`, the `j`/`k`/`l` server commands)

How CoD 1.1 MP carries quick chat end to end, and why it is dead code on
stock installs. Evidence comes from three modules: `game.mp.i386.so`
(1.1d Linux dedicated, full symbols - server side), `game_mp_x86.dll` 1.1
(same code, per `cod11-sound-system.md`), and `cgame_mp_x86.dll` 1.1
(image base `0x30000000`; md5s in `cod11-events-and-fx.md`). Addresses are
virtual. Nothing here has been observed on the wire from a stock server;
every fact is read from the binaries and labelled where it is only
inferred. The live captures so far (see `cod11-sound-system.md`) came from
modded servers that bypass this whole mechanism with `EV_SOUND_ALIAS`.

## Server side

`G_Voice` (`game.mp.i386.so`, symbol `G_Voice@@Base` @ `0x473b8`; Windows
`game_mp_x86.dll` at the same VA):

- Spam gate first: a per-entity accumulator at `ent+0x308` compared against
  level time (`0x1e8`), capped near 30 s (`cmp $0x752f`); exceeding it sends
  the `GAME_SPAMPROTECT` notice (string @ `0x73a86`) instead.
- The chat scope is the caller's mode argument (`0x10(%ebp)`): `1` selects
  the letter `"k"` (@ `0x73a48`), `2` selects `"l"` (@ `0x73a4a`),
  anything else `"j"` (@ `0x73a4c`).
- It then formats `"%s %d %d %d %s %i %i %i"` (string @ `0x73a4e`) and sends
  it as a reliable server command. Push order at `0x474db..0x4752f`: the
  letter, three ints taken from the entity fields, the category string
  argument, and three more ints that are the speaker origin xyz each loaded
  as float and converted with `fistpl` (`flds 0x20/0x1c/0x18(%ebx)`).

So one command looks like:

```
j <int> <int> <int> <category> <origin_x> <origin_y> <origin_z>
```

with every numeric field printed as a decimal integer and re-read by the
client through `atol`. The client uses argv 1 as the scope echo (its value
selects the chat-line printf format and the team-only gate), argv 3 as the
speaking client index, and argv 5-7 as the speaker origin; argv 2's meaning
was not pinned down.

## Client dispatch and handling

All addresses below are `cgame_mp_x86.dll` 1.1.

- Dispatch: console-string cases `'j'` `'k'` `'l'` (`0x6a/6b/6c`) call
  `FUN_3002d940(0/1/2)` (call sites `0x30026437..0x30026443` in the export).
- `FUN_3002d940` @ `0x3002d940`: reads argv 1-3 via trap_Argv (syscall `0xd`)
  through `atol` (@ `0x3004afde`), keeps argv 4 as the category string, and
  converts argv 5-7 back to floats (`fild`/`fstp` pairs). If global
  `DAT_301ad3cc` is set, it drops the auto-taunt categories early:
  `kill_insult` (@ `0x30062a54`), `taunt` (@ `0x30062a4c`),
  `death_insult`, `kill_gauntlet`, `praise`. What sets `DAT_301ad3cc` was
  not traced; the strings are the Q3 voice-chat category names.
- `FUN_3002d770` @ `0x3002d770` (`__thiscall`; ECX points at a vec3 the
  caller assembled on its own stack from argv 5-7 - the speaker origin from
  the command, not entity state): clamps argv 3 to `0..63` as the speaking
  client index,
  bails if `clientinfo[idx].field_0x00 == 0` (per-client structs stride
  `0x448` from `0x3018bc0c`), then picks the voice table:
  default `EBX = 0x302f7d80` (the axis table), switched to `0x30340ec8`
  (allies) when `clientinfo[idx].field_0x2c != 1` - i.e. team value `1`
  means Axis, matching the HUD convention (team 1 Axis / 2 Allies). A mode
  check passes when the mode argument is `1` or global `DAT_3029824c == 0`.
- Selection, `FUN_3002d390` @ `0x3002d390` (EBX = table base, stack args =
  category string plus three out-pointers): scans up to
  `count = *(table+0x44)` categories, records starting at `table+0x48` with
  stride `0x1244`; the match test is `FUN_3003e8f0(ECX = record)` returning
  0 on hit. On the first hit it draws `rand()` modulo the variant count at
  `record+0x40` and returns:
  - `record+0x44 + j*4` - the registered sound handle (array A),
  - `record+0x1144 + j*4` - the head-icon material handle (array B),
  - `record+0x144 + j*0x40` - pointer into an inline per-variant struct
    area whose purpose is unknown in 1.1 (the parser never writes it).
- Queueing: the entry `{pos.xyz, category copy char[149], client index,
  handles...}` lands in a ring at `DAT_305407c0` indexed by `DAT_3020d094`
  masked to 32 slots. Drain, `FUN_3002d5d0` @ `0x3002d5d0`: at most one
  entry per 1000 ms (`DAT_3020d08c = time + 1000`).
- Presentation, `FUN_3002d470` @ `0x3002d470`: copies the head-icon handle
  into the speaker's clientinfo (drawn over their head elsewhere) and
  prints `": <text>"` to the console. The text itself goes through the
  localizer `FUN_3002d670` @ `0x3002d670` (syscall `0x38` lookup), which
  falls back to wrapping the raw key in `#UNLOCALIZED# ... #`. The chat-line
  printf formats in `FUN_3002d770` are `"%s %s(%s): %c%c%s"` (global),
  `"(%s)%s(%s): %c%c%s"` (mode 1) and `"[%s]%s[%s]: %c%c%s"` (mode 2).

## The `.voice` file format

Parsed by `FUN_3002cbd0` @ `0x3002cbd0`; called once at init from
`FUN_30020a00` @ `0x30020a00`:

```
FUN_3002cbd0("mp/axis_chat.voice",   &DAT_302f7d80, 0x40);
FUN_3002cbd0("mp/allies_chat.voice", &DAT_30340ec8, 0x40);
```

Mechanics, all VERIFIED against the decompilation and asm:

- Read through the fs traps (`0xf` open/size, `0x10` read, `0x12` free);
  files larger than `0x4000` bytes are rejected ("voice chat file too
  large").
- Tokens come from the shared tokenizer `FUN_3003da70` (`GetToken`,
  Q3-style). Category and brace comparisons are case-insensitive.
- Grammar:

```
<gender>                      "male" @0x30062be4 -> table+0x40 = 0,
                              "female" @0x30062bec -> 1; anything else that
                              still matches a third check -> 2, else the
                              parse aborts ("expected gender not found")
<category>                    repeated until 64 categories (arg 3)
{
    <soundAlias> [<headIconShader>]
    ...                       up to 64 entries, until "}"
}
```

The head-icon slot is a **same-line** read: every other token comes from
`GetToken(allowLineBreaks = 1)` but the slot after a sound alias uses
`GetToken(0)` (second call site in `FUN_3002cbd0`, flag 0 vs 1 elsewhere).
An entry therefore pairs within one line - `alias` alone, or
`alias headicon`; an `}` in that slot is pushed back (`UngetToken`) and the
default icon applies. Two aliases on one line make the second one the
first's head icon, not a new entry; tokens for categories and braces flow
across lines freely.

- Each `soundAlias` is registered with syscall `0xbf`, stored into array A.
  The same syscall registers known sound aliases like
  `mp_announce_g_twominutes` right next door in `FUN_30020a00`, so it is
  `S_RegisterSound` (INFERRED name, verified usage).
- The optional second token becomes array B via `FUN_30030b70(name, 2)`
  (INFERRED register-shader/material); missing or empty defaults to
  `headiconVoiceChat`. When the token after a sound alias is `}`, it is
  pushed back (`UngetToken`) and the default icon applies.
- Per-category record layout (stride `0x1244`): match data `[0x00..0x40]`,
  entry count u32 at `+0x40` (incremented in place while parsing),
  array A at `+0x44` (up to 64 dwords), inline structs `+0x144..0x1143`,
  array B at `+0x1144` (up to 64 dwords). `0x44 + 64*4 = 0x144` and
  `0x1144 + 64*4 = 0x1244` exactly.

The aliases these files name are the `american_*` / `british_*` /
`russian_*` / `german_*` rows of `dialog_mp.csv` (see
`cod11-sound-system.md` section on quick chat).

## Negative result: stock installs ship no `.voice` data

Every `.pk3` in both local installs (CoD 1.1 `main/`, Deluxe Edition
`main/` and `uo/`, including all `localized_english_pak*.pk3`) lists zero
`*.voice` entries (checked with a zipfile walk over every pak), and a
loose-file sweep finds none either. On a stock client `FUN_30020a00`
therefore logs "^1voice chat file not found: mp/axis_chat.voice" twice,
both tables stay empty, `FUN_3002d390` always returns 0, and `FUN_3002d770`
returns without touching the queue. Stock quick chat prints nothing, plays
nothing, shows no head icon. Any populated stock server can send `j/k/l`
commands all day; nothing happens client-side unless a mod ships `.voice`
files.

This makes "wire it like retail" an inert path on stock data, which vcod
accepts deliberately (full parity). The alternative - shipping our own
`.voice` files built from `dialog_mp.csv` - would be new game data, not RE,
and is out of scope.

A live sweep against three populated public servers (2026-08-26,
`51.195.89.86:28960` TDM, `167.235.192.175:23120`, `13.60.184.96:28962`
S&D, 75-90 s each) sent no `j`/`k`/`l` command at all. All three are
moddedicated servers (`g_scriptMainMenu` sets `team_*`, `i`/`f`/`v`/`t`/
`n`/`h`/`s` commands, `MPSCRIPT_*`/`scr_fm_*` text), which relay voice
lines through `s <idx>`/events instead. The stock `j`/`k`/`l` path remains
unobserved live and is not exercised by the current public-server
population.

## vcod behaviour and deliberate divergences

- Parser: `crates/common/src/voicechat.rs`, mirrors the grammar above
  (gender, categories, optional head icon, 64/64 caps, case-insensitive
  braces and category names). Loaded lazily from the game dir; a missing
  file warns once and leaves quick chat inert, exactly like retail.
- Table choice by speaker team: team 1 (Axis) -> axis file, everything
  else -> allies file, mirroring the `clientinfo[idx]+0x2c != 1` switch.
- Variant pick: random among the category's variants, like
  `rand() % count`.
- Cadence: at most one queued line per 1000 ms, like `FUN_3002d5d0`.
- Text: the raw category string is displayed, not localized - vcod has no
  `.str` localization engine, and retail's own fallback for a missing key
  is the raw key wrapped in `#UNLOCALIZED#`. Documented divergence.
- Head icon above the speaker: not implemented (needs renderer/entity
  work); the material name is parsed and kept.
- The suppression list (`kill_insult` etc. gated on `DAT_301ad3cc`) is not
  replicated; its enabling condition was not traced.
