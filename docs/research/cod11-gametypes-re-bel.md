# Retrieval (`re`) and Behind Enemy Lines (`bel`) against retail

`re.gsc` and `bel.gsc` (pak5, `maps/MP/gametypes/`) run through the same
script path as `dm`, `tdm` and `sd`. This file records the 2026-10-06 run of
both against the retail 1.1d Linux server and what it took to make ours
match. Addresses are `game.mp.i386.so` (1.1d) unless named otherwise.

How the runs were taken: `tools/run_server.sh <map> +set g_gametype <gt>`
for retail and `vcod-server <map> --gametype <gt>` for ours, each joined by
two `vcod --net-probe` clients, one answering the team menu with `allies`
and one with `axis` three seconds later, `--probe-secs 90` and 87. The
retail configstring tables went to
`crates/server/tests/fixtures/configstrings/mp_chateau-re.txt` and
`mp_brecourt-bel.txt` through `--save-configstrings`.

## 1. Which maps carry the entities

VERIFIED, from the entity lumps of the twelve stock MP BSPs (pak4/pak5):
every map ships `mp_retrieval_spawn_allied` (24 to 32 each), at least two
`retrieval_objective` targetnames and the `re`/`retrieval`
`script_gameobjectname` objects. `bel` needs only the `tdm` spawns
(`mp_teamdeathmatch_spawn`, 24 on mp_rocket, 32 elsewhere); only
mp_brecourt (4) and mp_harbor (1) carry objects named `bel` for
`_gameobjects` to keep.

VERIFIED, same lumps: the only `trigger_use` entities on the stock maps are
`re`'s pickup triggers, one or two per map, each with an `origin` key (an
origin brush), e.g. mp_carentan's `auto1` at `395 2137 245` (`*8`). No stock
map gives any entity a `cursorhint` or `hintstring` key. S&D's
`bombtrigger` is a `trigger_lookat`.

VERIFIED, ours: with the fixes below, every one of the 24 map/gametype pairs
loads and runs its first seconds with no script error. Before them, every
`re` map died in `retrieval_spawn_objective` on `setHintString`, and
mp_chateau's map script died on `setExpFog` under every gametype.

## 2. `setExpFog`

VERIFIED, retail mp_chateau `re` gamestate: configstring 12 is
`0 1 1e-05 0 0 0 0` for `mp_chateau.gsc`'s `setExpFog(0.00001, 0, 0, 0, 0)`
(the fixture, slot 12).

VERIFIED, `setExpFog` (0x5b774, builtin table slot 70): five
`Scr_GetFloat`s, the `va` format `%g %g %g %g %g %g %.0f` at 0x77da9, and
the pushes ahead of it at 0x5b8e1..0x5b8ea that put the double 0.0 and the
double 1.0 (`0x3ff00000:00000000`) in the first two slots. The fifth
argument is multiplied by the float at 0x77fa8, which reads 1000.0. The
result goes to `G_setfog` (0x48fa4), whose first call is
`trap_SetConfigstring(12, ...)` (0x48fb4). INFERRED from that: near is
always 0 and far always 1, the density takes the third slot, and the time is
milliseconds rounded by `%.0f`.

VERIFIED, the error strings the function reaches: `setExpFog: distance must
be greater than 0 and less than 1` (0x77f60), and through `va` with the name
`setExpFog` (0x77f9b) `%s: red/green/blue color components must be in the
range [0, 1]` (0x77d40) and `%s: transition time must be >= 0 seconds`
(0x77d80). INFERRED from the compares at 0x5b7e0..0x5b7f9: a density outside
the open interval (0, 1) is the first error. `setCullFog` (0x5b5a8) pushes
the same two `va` formats (0x5b6d9, 0x5b703), multiplies its time by the
float at 0x77e20, and formats through the same string (0x5b75f). VERIFIED,
the addresses; INFERRED, that its range checks match.

vcod: `builtins::env::set_exp_fog` and `set_cull_fog` share `write_fog`,
which formats the time as milliseconds. Before this, `setCullFog` wrote its
time argument as seconds; every stock map passes 0, so no stock table moved.

The density's `1e-05` is C's `%g` exponent form, which vcod's `format_g`
spelled `1e-5` before; `probe_concat_exp` measured the form and
`docs/research/cod11-gsc-language.md` has the values.

## 3. `announcement` and `clientAnnouncement`

VERIFIED, retail `re` on mp_chateau, both probes: when the second team
fills, each client receives the reliable command `c "RE_MATCHSTARTING\x15"
2`, and at the elimination after the axis probe left, `c
"RE_ELIMINATED_AXIS\x15" 2`.

VERIFIED, `announcement` (0x5f0f0, slot 85): `Scr_ConstructMessageString`
from parameter 0 (the call at 0x5f10a), `va` with `c "%s" 2` (0x77fb6) and
`trap_SendServerCommand(-1, 0, ...)` (0x5f121..0x5f125).
`clientAnnouncement` (0x5f134, slot 86) reads `Scr_GetEntity(0)`, constructs
from parameter 1 and sends to the entity's number (0x5f177). INFERRED: a
broadcast and a one-client form of the same command.

`re.gsc` calls `clientAnnouncement` only from inside a `/*REMOVED ... */`
comment in `spawnPlayer` and from the friendly-fire warning; `announcement`
carries every round message.

vcod: `builtins::io::announcement`, the message packed by
`message::construct` the way `iPrintLn`'s is. The vcod client does not
render the `c` command yet; that is a client HUD item.

## 4. `trigger_use`, `setHintString` and the use key

### 4.1 Hint strings

VERIFIED, retail mp_chateau `re` gamestate: configstrings 1214 and 1215 hold
`RE_PRESS_TO_PICKUP\x14RE_OBJ_ARTILLERY_MAP\x15` and
`RE_PRESS_TO_PICKUP\x14RE_OBJ_SPY_RECORDS\x15`, behind the engine's own
`CGAME_USEMG42` (1212) and `CGAME_USEPTRS41` (1213).

VERIFIED, `setHintString` (0x5dd58, method slot 27): the receiver's
classname word (`ent+0x176`) is compared against `scr_const+0x94`, which
`GScr_LoadConsts` fills with `trigger_use` (0x58c58, string at 0x761f1), and
a mismatch reaches `Scr_Error` with `The setHintString command only works on
trigger_use entities.` (0x76ea0). A parameter of type 1 compared
`Q_stricmp`-equal to the empty string (0x76629) stores 0xff at `ent+0xd8`
(0x5dde6). Otherwise `Scr_ConstructMessageString` from parameter 0 feeds
`G_GetHintStringIndex` (0x5de12), a zero return reaches `Too many different
hintstring values. Max allowed is %i different strings` (0x76ee0) with 0x20,
and the index byte is stored at `ent+0xd8` (0x5de3d). INFERRED: a plain `""`
clears the hint string and anything else is allocated in the 32-slot range at
1212; the capture's two slots are what that allocation gives.

### 4.2 How retail reaches a `trigger_use`

VERIFIED, `SP_trigger_use` (0x5742c): contents 0x200000 at `ent+0x118`
(0x5745a), cursor hint 2 at `ent+0xdc` (0x5749f), hint string 0xff at
`ent+0xd8` (0x57550).

VERIFIED, `G_TouchTriggers`' broad phase queries with contents mask
0x405c0008 (`docs/research/cod11-gsc-object-model.md` 22.1), which has no
0x200000 bit. INFERRED: no touch ever returns a `trigger_use`.

VERIFIED, `G_GetActivateEnt` keeps candidates whose contents byte `ent+0x11a`
has 0x20, which is 0x200000 (`docs/research/cod11-items.md` 2.1). VERIFIED,
`Cmd_Activate_f` (0x48468): after `G_CheckForCursorHints` (0x484c5) it reads
the chosen entity number from `cl+0x3b8` (0x484d3) and, when that entity's
classname is `scr_const+0x94` (`trigger_use`, 0x48527), calls
`Scr_Notify(ent, scr_const+0x92, 1)` (0x48547) with the player pushed by
`Scr_AddEntity` (0x48534); `scr_const+0x92` is `trigger` (0x58c40, string at
0x761e9). INFERRED: a `trigger_use` fires when the use key's aim pick lands
on it, under the same 128-unit reach and 0.76 cone as an item, and contact
plays no part; there is no `wait` gate on this path.

VERIFIED, `G_CheckForCursorHints`' `trigger_use` arm (0x4f69d..0x4f6dd):
when the chosen entity's `eType` is 0 and its classname is `trigger_use`, the
hint is `ent+0xdc`, a zero hint skips the entity, and the string is
`ent+0xd8` unless it reads 0xff. INFERRED: a stock `re` pickup trigger shows
`HINT_ACTIVATE` with the objective's hint string.

vcod, since this change: `item::activate_ent` scores `trigger_use` rows
beside items and turrets, centred on their absolute box; the use key's
`Activate::Trigger` queues the `trigger` notify; `trigger::touched` skips
`trigger_use`; `cursor_hint_pass` returns the trigger's hint and string;
`setHintString` lives in `builtins::entity`. Before it, a `trigger_use` fired
on contact with the use key held, which on the stock maps only `re` used.

Not measured: no capture of a client aimed at an `re` pickup has been taken,
so the hint fields a retail client reads there, and whether the pickup
trigger's absolute box centre sits where vcod puts it, are unchecked against
the wire.

## 5. `setTimerUp` takes a zero

VERIFIED, retail `bel` on mp_brecourt: the allied probe's
`allied_hud_element` ran past `self.hud_clock setTimerUp(0)` (bel.gsc line
1535) to `give_allied_points`, whose `A;1;allies;vcod;bel_alive_tick` line
reached `games_mp.log` every 10 s from 0:23 to 1:33. Ours aborted the thread
on that line with "a timer's time must be above zero" and logged no tick.

VERIFIED, the four timer methods: `setTimer` (0x4b8e4), `setTenthsTimer`
(0x4baf4) and `setTenthsTimerUp` (0x4bc14) each hold a `jg` (0x4b977,
0x4bb87, 0x4bca7) ahead of a `va` with `time %g should be > 0` (0x747e7);
`setTimerUp` (0x4ba04) goes from the round-up at 0x4ba6c straight to the
stores ending at 0x4bae5, with no compare. INFERRED: only `setTimerUp`
accepts a time at or below zero.

## 6. Round flow, live

`re`, mp_chateau. VERIFIED, both servers, the same server-command sequence
per probe up to the match start: `f MPSCRIPT_CONNECTED`, the
`team_britishgerman` menu, `cg_objectiveText` `RE_OBJ_CHATEAU_OBJ_SPECTATOR`,
then `ATTACKER` for allies and `DEFENDER` for axis, `RE_MATCHSTARTING`, the
restart (`n`), the move-in sounds, and after the axis probe left,
`RE_ELIMINATED_AXIS` with `MP_announcer_allies_win`. The script's
`logPrint` lines matched (`J`, `Q`, `W;allies`, `L;axis`). The objective models are
entities 85 and 86, `eType` 8, on both servers along a `--probe-pvs` walk
from the allied spawn.

`bel`, mp_brecourt. VERIFIED, both servers: the `team_germanonly` menu, the
`BEL_SPECTATOR_OBJS` text, the second client moved to allies with
`weapon_americangerman` and `BEL_OBJ_ALLIED`, and, once 5 was fixed, the
same 10-second `bel_alive_tick` lines.

Differences that remain, none specific to these gametypes:

- Ours played the allied move-in sound once before the restart as well as
  after it; retail played it only after. VERIFIED, the two command
  sequences. INFERRED, a run artefact rather than a divergence: the first
  `sayMoveIn` (`_teams.gsc`) waits two seconds from level start, ours' debug
  build spent the four seconds before the first probe loading the map (its
  snapshots read `t=1950` three seconds after the connect, retail's
  `t=11100`), so on ours the probes were already in when it ran.
- The restart rebroadcasts configstrings 3, 12 and 13 on retail and only 3
  on ours, with `t\0` in 3: `docs/research/cod11-map-cycle.md` 4.5.
- `sayAll`, `sayTeam` and `pingPlayer` (`_teams.gsc`'s quick chat, in every
  gametype) are still unimplemented: retail routes the first two through
  `G_Say` (0x4590c, 0x459a8), and vcod's server has no chat.
- `addTestClient` is unimplemented; the stock scripts reach it only from
  their `scr_numbots` debug thread.

Not exercised against retail: an `re` pickup, carry, drop and capture; a
`bel` kill and the team swap it causes. Those need a scripted shooter, and
are hand-check items.
