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
`message::construct` the way `iPrintLn`'s is. The client draws `c` in the
bold message window (`docs/research/cod11-chat.md` 3.3).

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
- `addTestClient` is unimplemented; the stock scripts reach it only from
  their `scr_numbots` debug thread.

Sections 7 and 8 cover what this run left out: an `re` pickup, carry,
drop, return and capture, and a `bel` kill and the team swap it causes.

## 7. Retrieval: pickup, drop, return and capture

### 7.1 How the run was taken

Run 2026-10-07 against the retail 1.1d Linux server on mp_brecourt, which
has one objective (the code book, model entity 129 on both servers) with a
single spawn spot, so `retrieval_spawn_objective` has no random pick to
make. The gametype is `client-probes/probe_re.gsc`, which calls
`re::main()` and drives the run from a thread:

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=<p> SECS=190 \
    tools/run_probe.sh client-probes/probe_re mp_brecourt
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-team allies \
    --save-scripted attacker --probe-secs 150      # 4 s after the server
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-team axis \
    --save-scripted defender --probe-secs 147      # 3 s after the first
```

`--save-scripted` presses use as the probe's `setClientCvar("probe_use",
...)` says (`tap`: 100 ms every second; `hold`), keeps the view the server
sets, asks for `score` every 2 s, and records every server command and
every snapshot whose HUD, objective block, roster or entities moved
(`vcod_common::net::capture`). The fixtures are
`crates/server/tests/fixtures/gametypes/mp_brecourt-re-attacker.txt`,
`-defender.txt` and the server's `games_mp.log` as `-re-log.txt`;
`crates/server/tests/gametypes_ab.rs` replays the same probe on ours with
two clients through the same recorder and compares them view by view.

The script, after the match-start restart: round 1 puts the attacker on the
objective's spot (`setOrigin(trigger.origin)`, `setPlayerAngles((85, 0,
0))`) and taps use; moves it to `(1432, -988, -37)` and holds use until
`holduse` drops the book; waits out `objective_timeout`; picks the book up
again and calls `suicide()`. Round 2 puts the defender on the spot tapping
use for 3 s, then the attacker picks up and is set inside the goal's
trigger. All VERIFIED below is off the three fixtures.

### 7.2 What retail does

- Pickup. VERIFIED: the first use tap on the spot, looking down at the
  `trigger_use`, picks the book up inside 50 ms of the `probe_use "tap"`:
  `c "RE_OBJ_PICKED_UP_NOSTARS\x14RE_OBJ_CODE_BOOK\x15" 2` to both
  clients, the alias `re_pickup_paper` allocated (`d 530`) and played to the
  carrier alone (`s`). The use key reaches a `trigger_use` from 58 units
  above its box centre with the view pitched 85 degrees down, which closes
  the "not measured" note of 4.2.
- Carrier, own view. VERIFIED: a text hudelem (type 1) at `635,385`, label
  `RE_U_R_CARRYING`, text `RE_OBJ_CODE_BOOK`, in the non-archived array;
  objective slot 2 reads `entNum 0` (the carrier) and `teamNum 2`.
- Carrier, the defender's view. VERIFIED: entity 0 carries `iHeadIcon` naming
  `gfx/hud/headicon@re_objcarrier.tga` and `iHeadIconTeam` 2; slot 2 reaches
  the defender with state 0 (the team filter, object model doc 23.3). The
  book entity stays in both snapshots with `eFlags` 0x100 while carried
  (`hide()`), and reads 0 again at the drop.
- The hold-use drop. VERIFIED: `pm_type` 1 (linked to `holduse`'s
  `script_origin`) for the hold; two shader hudelems (type 3), `black`
  292x12 at `320,385` with colour `0x80ffffff` and `white` at `176,385`,
  8 high, 7 or 8 units wider each frame; after 2 s
  `RE_OBJ_DROPPED\x14RE_OBJ_CODE_BOOK`, the bar and the carry text gone, the
  book shown at `getPlant`'s point, slot 2 at `entNum 1023`, `teamNum 0`,
  origin the truncated plant point (7.4).
- The return. VERIFIED: 60 s after the drop,
  `RE_OBJ_TIMEOUT_RETURNING\x14` + `60`, the book and slot 2 back on the
  spot.
- Drop on death. VERIFIED: `suicide()` while carrying gives
  `RE_OBJ_DROPPED`, then `RE_ELIMINATED_ALLIES`, `d 5 1` (the axis team
  score) and `MP_announcer_axis_win`; the corpse is body-queue entity 64 and
  the carbine drops as an item; the book lands 11 units ahead of the body.
  VERIFIED, the log: the carrier's `objs_held` still reads 1 after the death
  (`drop_objective_on_disconnect_or_death` never decrements it) and 0 again
  after the restart.
- The defender's use. VERIFIED: `client_print`'s text hudelem at `320,200`,
  label `RE_PICKUP_ALLIES_ONLY`, text `RE_OBJ_CODE_BOOK`, for 3 s; nothing
  else moves.
- Capture. VERIFIED: setting the carrier inside the goal brush fires it on
  the next touch pass: `RE_OBJ_CAPTURED\x14RE_OBJ_CODE_BOOK`,
  `RE_OBJ_CAPTURED_ALL`, `d 6 1`, `MP_announcer_allies_win`, the round
  restart 5 s later. The last scoreboard of the run reads
  `b 2 1 1` with rows `1 0 _ 0 0` and `0 -1 _ 1 0`.

### 7.3 Head icons

VERIFIED, `game.mp.i386.so`: `.headicon`'s setter (0x41bd4) passes the
string through `GScr_GetHeadIconIndex` (0x5c840) and stores the result at
`gentity+0x94`; the getter (0x41c14) reads it back, 0 as `""` (0x72e60) and
`1..15` as configstring `0x1c + n`. `.headiconteam`'s setter (0x41c84)
compares `scr_const` slots and stores at `gentity+0x98`: `+0xf8` (`none`) 0,
`+0x4` (`allies`) 2, `+0x8` (`axis`) 1, and `+0x7c` (`spectator`) branches to
`Scr_Error` with `'%s' is an illegal head icon team string. Must be none,
allies, axis, or spectator.` (0x72e80). INFERRED, off the fall-through at
0x41cfe: any other string stores 3. The getter (0x41d3c) names 1 `axis`, 2
`allies`, 3 `spectator` and anything else `none`. VERIFIED, the netfield
table: `iHeadIcon` is `entityState+148` (0x94) and `iHeadIconTeam` `+152`
(0x98), so both setters write the wire fields directly.

VERIFIED, `GScr_GetHeadIconIndex`: `""` answers 0; otherwise configstrings
`0x1d..0x2b` are compared with `Q_stricmp` and the first match answers its
1-based position; no match reaches `Scr_Error("Head icon '%s' was not
precached\n")` (0x773a0).

VERIFIED, `.statusicon`'s getter (0x41b7c): 0 reads `""` and `1..8`
configstring `0x14 + n`.

### 7.4 `objective_position` detaches

VERIFIED: `objective_position` (0x5e128) opens with the same detach
`objective_onentity` makes: when the slot's `entNum` (`+0x10`) is not 0x3ff
it clears `eFlags` bit 0x10 on that entity (0x5e190) and writes 0x3ff
(0x5e197), then stores the origin. `re.gsc` relies on it: `drop_objective`
never touches the objective, and the `objective_position` at the top of the
`retrieval_think` it restarts is what pins the marker where the book fell.

### 7.5 What ours got wrong, and the fixes

Each was a difference the gate showed; all are fixed and the gate has no
gaps.

- No head icon on the wire: `.headicon` and `.headiconteam` were plain
  stored strings and the entity state never carried them. They are now
  indices in the client store, the getters answer as 7.3 says, and
  `ClientSim::to_entity` writes `iHeadIcon` / `iHeadIconTeam`, mirrored at
  the end frame. Before, the unset fields read `undefined` where retail
  reads `""` and `"none"`, and `.statusicon` the same.
- `objective_position` kept the attached entity, so the dropped book's
  marker stayed on the carrier's entity number.
- A client that never had a model sent `modelindex` 101, the first empty
  model slot (`client_model_index` matched `""` against the table); retail
  sends 0.

Left alone. VERIFIED, the log fixture and ours: the landing after
`setOrigin` 1.66 units over the 1.8-degree grade at the drop point settles
at y -987.99 on retail and -987.96 on ours; the gate compares the log's
coordinates to the unit. VERIFIED, two earlier retail runs that set the
player down 32 units up (not committed): one settled 22 units off in y, the
next 0.03. The fall does not repeat on retail itself, which is why the probe
now sets the player down on the ground.

## 8. Behind Enemy Lines: the team swap

### 8.1 How the run was taken

Same day, same map, `client-probes/probe_bel.gsc` around `bel::main()`, two
`--save-scripted` probes that both answer the team menu with `axis`
(`--probe-team axis --save-scripted first` / `second`, 130 s). The probe
moves every player that spawns into play to a fixed spot by team, 88 units
apart (`(1432, -988, -37)` allied, `(1432, -900, -39)` axis), so bodies and
corpses do not ride on `getSpawnpoint_MiddleThird`'s pick. It kills with
`victim finishPlayerDamage(attacker, attacker, 1000, 0,
"MOD_RIFLE_BULLET", attacker getCurrentWeapon(), ...)`, which reaches
`Callback_PlayerKilled` with the attacker as a rifle round would: kill 1
the axis player kills the allied one, kill 2 the same the other way round,
kill 3 the allied player kills the axis one. Fixtures:
`mp_brecourt-bel-first.txt`, `-second.txt`, `-bel-log.txt`.

### 8.2 What retail does

- Join. VERIFIED: the first probe, alone, answers `team_germanonly` with
  axis and `weapon_german`, and 2 s later is moved to allies: the blackscreen
  hudelems (`BEL_BLACKSCREEN_WILLSPAWN`, a timer, `black` 640x480),
  `MPSCRIPT_JOINED_ALLIES`, the `weapon_american` menu, a spawn with the
  carbine and the allied HUD a frame and a half later: `black` 130x35 at
  `505,382` colour `0x33ffffff`, a timer-up element (type 5) labelled
  `BEL_TIME_ALIVE` and a value element (type 2) `BEL_POINTS_EARNED` reading
  1. The compass marker is objective slot `entnum + 1`, `teamNum 1`, icon
  `gfx/hud/hud@objective_bel.tga`.
- Kill 1. VERIFIED: `BEL_KILLED_ALLIED_SOLDIER` with the killer's name, and
  both roster entries flip team on the kill's frame while each body model
  waits for its owner's next spawn. VERIFIED, the log: the killer respawns
  allied 2 s after the kill (`PROBE placed 1 allies`), with the carbine,
  the allied HUD and its own marker slot; the victim's view shows its
  killcam from 2 s after the death (`MPSCRIPT_KILLCAM`,
  `MPSCRIPT_PRESS_ACTIVATE_TO_RESPAWN`, a timer at `320,428`) and it spawns
  axis through `weapon_german` with the kar98k 7 s after the kill. The
  killer's own view shows the blackscreen with `BEL_BLACKSCREEN_KILLEDALLIED`
  until that respawn. Its score goes up by one at the marker's spawn
  (`make_obj_marker`) and by one every 10 s after (`bel_alive_tick`), the
  points element counting with it.
- Kill 2 is the same with the roles swapped. Kill 3 (axis victim) gives the
  allied killer a point and the victim a killcam and an axis respawn, with
  no swap.
- VERIFIED, both probes: a respawn empties the objective block, so a slot
  whose record went to state 0 before the spawn reads all zero after it,
  where before the spawn it kept its icon and origin under state 0.

### 8.3 `classname` before the first spawn

VERIFIED: `ClientConnect` calls `G_InitGentity` (0x4254b), which sets
`classname` to `scr_const+0x56` (0x6783c), allocated from `noclass`
(0x760ac) by `GScr_LoadConsts` (0x58963); `ClientSpawn` sets it to
`scr_const+0x62` (0x4271d), allocated from `player` (0x760e3, 0x589f3).
VERIFIED, the bel log: no state line ever lists a client still on
`Callback_PlayerConnect` (status icon `hud@status_connecting`), where ours
listed it with `pers["team"]` undefined. `cod11-items.md`'s
`PROBE other 0 noclass` reading is the same fact.

### 8.4 What ours got wrong, and the fixes

- A client was `classname` "player" from its connect; it is now "noclass"
  until `self spawn(...)` (`client_spawn`), so `getentarray("player",
  "classname")` agrees with retail through the connect callback.
- A spawn kept the client's objective copy. `ClientSpawn` zeroes the whole
  `gclient_t` (0x42804, 0x22c4 bytes), the copy at `client+0x3e8` with it;
  `client_spawn` now resets the copy (`GameHost::reset_client_objectives`).

VERIFIED, the gate before these two fixes: the swap itself, the menus, the
HUD, the roster, the scores and the log already matched retail.

Left out of the comparison: the marker's origin. INFERRED, off `bel.gsc`'s
`make_obj_marker`: it reads the allied player's origin inside
`spawnPlayer`, before the probe moves it, and follows a running mean of
where the player stood, so it carries the random spawn pick; the gate
compares the slot's state, icon, entity and team.
