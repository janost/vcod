# Retrieval (`re`) and Behind Enemy Lines (`bel`) against retail

`re.gsc` and `bel.gsc` (pak5, `maps/MP/gametypes/`) run through the same
script path as `dm`, `tdm` and `sd`. This file records runs of both against
the retail 1.1d Linux server and what made ours match. Addresses are
`game.mp.i386.so` (1.1d) unless named otherwise.

The first runs: `tools/run_server.sh <map> +set g_gametype <gt>` for retail
and `vcod-server <map> --gametype <gt>` for ours, each joined by two
`vcod --net-probe` clients, one answering the team menu with `allies` and
one with `axis` three seconds later, `--probe-secs 90` and 87. The retail
configstring tables went to
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
origin brush), e.g. mp_carentan's `auto1` at `395 2137 245` (`*8`). No
stock map gives any entity a `cursorhint` or `hintstring` key. S&D's
`bombtrigger` is a `trigger_lookat`.

VERIFIED, ours: every one of the 24 map/gametype pairs loads and runs its
first seconds with no script error. Without sections 2 and 4, every `re`
map dies in `retrieval_spawn_objective` on `setHintString`, and
mp_chateau's map script dies on `setExpFog` under every gametype.

## 2. `setExpFog`

VERIFIED, retail mp_chateau `re` gamestate: configstring 12 is
`0 1 1e-05 0 0 0 0` for `mp_chateau.gsc`'s
`setExpFog(0.00001, 0, 0, 0, 0)` (the fixture, slot 12).

VERIFIED, `setExpFog` (0x5b774, builtin table slot 70): five
`Scr_GetFloat`s, the `va` format `%g %g %g %g %g %g %.0f` at 0x77da9, and
the pushes ahead of it at 0x5b8e1..0x5b8ea that put the double 0.0 and the
double 1.0 (`0x3ff00000:00000000`) in the first two slots. The fifth
argument is multiplied by the float at 0x77fa8, which reads 1000.0. The
result goes to `G_setfog` (0x48fa4), whose first call is
`trap_SetConfigstring(12, ...)` (0x48fb4). INFERRED from that: near is
always 0 and far always 1, the density takes the third slot, and the time
is milliseconds rounded by `%.0f`.

VERIFIED, the error strings the function reaches: `setExpFog: distance
must be greater than 0 and less than 1` (0x77f60), and through `va` with
the name `setExpFog` (0x77f9b) `%s: red/green/blue color components must
be in the range [0, 1]` (0x77d40) and `%s: transition time must be >= 0
seconds` (0x77d80). INFERRED from the compares at 0x5b7e0..0x5b7f9: a
density outside the open interval (0, 1) is the first error. VERIFIED, the
addresses: `setCullFog` (0x5b5a8) pushes the same two `va` formats
(0x5b6d9, 0x5b703), multiplies its time by the float at 0x77e20, and
formats through the same string (0x5b75f). INFERRED: its range checks
match.

vcod: `builtins::env::set_exp_fog` and `set_cull_fog` share `write_fog`,
which formats the time as milliseconds (trap: `setCullFog`'s time is not
seconds on the wire; every stock map passes 0, so no stock table shows it).

The density's `1e-05` is C's `%g` exponent form, not `1e-5`;
`probe_concat_exp` measured the form and `cod11-gsc-language.md` has the
values.

## 3. `announcement` and `clientAnnouncement`

VERIFIED, retail `re` on mp_chateau, both probes: when the second team
fills, each client receives the reliable command
`c "RE_MATCHSTARTING\x15" 2`, and at the elimination after the axis probe
left, `c "RE_ELIMINATED_AXIS\x15" 2`.

VERIFIED, `announcement` (0x5f0f0, slot 85): `Scr_ConstructMessageString`
from parameter 0 (the call at 0x5f10a), `va` with `c "%s" 2` (0x77fb6) and
`trap_SendServerCommand(-1, 0, ...)` (0x5f121..0x5f125).
`clientAnnouncement` (0x5f134, slot 86) reads `Scr_GetEntity(0)`,
constructs from parameter 1 and sends to the entity's number (0x5f177).
INFERRED: a broadcast and a one-client form of the same command.

`re.gsc` calls `clientAnnouncement` only from inside a `/*REMOVED ... */`
comment in `spawnPlayer` and from the friendly-fire warning;
`announcement` carries every round message.

vcod: `builtins::io::announcement`, the message packed by
`message::construct` as `iPrintLn`'s is. The client draws `c` in the bold
message window (`cod11-chat.md` 3.3).

## 4. `trigger_use`, `setHintString` and the use key

### 4.1 Hint strings

VERIFIED, retail mp_chateau `re` gamestate: configstrings 1214 and 1215
hold `RE_PRESS_TO_PICKUP\x14RE_OBJ_ARTILLERY_MAP\x15` and
`RE_PRESS_TO_PICKUP\x14RE_OBJ_SPY_RECORDS\x15`, behind the engine's own
`CGAME_USEMG42` (1212) and `CGAME_USEPTRS41` (1213).

VERIFIED, `setHintString` (0x5dd58, method slot 27):

- The receiver's classname word (`ent+0x176`) is compared against
  `scr_const+0x94`, which `GScr_LoadConsts` fills with `trigger_use`
  (0x58c58, string at 0x761f1). A mismatch reaches `Scr_Error` with `The
  setHintString command only works on trigger_use entities.` (0x76ea0).
- A parameter of type 1 that is `Q_stricmp`-equal to the empty string
  (0x76629) stores 0xff at `ent+0xd8` (0x5dde6).
- Otherwise `Scr_ConstructMessageString` from parameter 0 feeds
  `G_GetHintStringIndex` (0x5de12). A zero return reaches `Too many
  different hintstring values. Max allowed is %i different strings`
  (0x76ee0) with 0x20, and the index byte is stored at `ent+0xd8`
  (0x5de3d).

INFERRED: a plain `""` clears the hint string and anything else is
allocated in the 32-slot range at 1212; the capture's two slots are what
that allocation gives.

### 4.2 How retail reaches a `trigger_use`

VERIFIED, `SP_trigger_use` (0x5742c): contents 0x200000 at `ent+0x118`
(0x5745a), cursor hint 2 at `ent+0xdc` (0x5749f), hint string 0xff at
`ent+0xd8` (0x57550).

VERIFIED, `G_TouchTriggers`' broad phase queries with contents mask
0x405c0008 (`cod11-gsc-object-model.md` 22.1), which has no 0x200000 bit.
INFERRED: no touch ever returns a `trigger_use`.

VERIFIED, `G_GetActivateEnt` keeps candidates whose contents byte
`ent+0x11a` has 0x20, which is 0x200000 (`cod11-items.md` 2.1). VERIFIED,
`Cmd_Activate_f` (0x48468): after `G_CheckForCursorHints` (0x484c5) it
reads the chosen entity number from `cl+0x3b8` (0x484d3). When that
entity's classname is `scr_const+0x94` (`trigger_use`, 0x48527), it calls
`Scr_Notify(ent, scr_const+0x92, 1)` (0x48547) with the player pushed by
`Scr_AddEntity` (0x48534); `scr_const+0x92` is `trigger` (0x58c40, string
at 0x761e9). INFERRED: a `trigger_use` fires when the use key's aim pick
lands on it, under the same 128-unit reach and 0.76 cone as an item.
Contact plays no part, and there is no `wait` gate on this path.

VERIFIED, `G_CheckForCursorHints`' `trigger_use` arm (0x4f69d..0x4f6dd):
when the chosen entity's `eType` is 0 and its classname is `trigger_use`,
the hint is `ent+0xdc`, a zero hint skips the entity, and the string is
`ent+0xd8` unless it reads 0xff. INFERRED: a stock `re` pickup trigger
shows `HINT_ACTIVATE` with the objective's hint string.

vcod: `item::activate_ent` scores `trigger_use` rows beside items and
turrets, centred on their absolute box. The use key's `Activate::Trigger`
raises the `trigger` notify with the cmd's other engine notifies, outside
`G_RunFrame`'s trigger queue. `trigger::touched` skips `trigger_use`;
`cursor_hint_pass` returns the trigger's hint and string; `setHintString`
lives in `builtins::entity`. Trap: firing a `trigger_use` on contact with
the use key held is wrong (on the stock maps only `re` would notice).

The use reach is measured in 7.2 (the pickup bullet). Still unchecked
against the wire: the hint fields a retail client reads while aimed at an
`re` pickup.

## 5. `setTimerUp` takes a zero

VERIFIED, retail `bel` on mp_brecourt: the allied probe's
`allied_hud_element` ran past `self.hud_clock setTimerUp(0)` (bel.gsc line
1535) to `give_allied_points`, whose `A;1;allies;vcod;bel_alive_tick`
line reached `games_mp.log` every 10 s from 0:23 to 1:33. A server that
refuses a zero ("a timer's time must be above zero") aborts the thread on
that line and logs no tick.

VERIFIED, the four timer methods: `setTimer` (0x4b8e4), `setTenthsTimer`
(0x4baf4) and `setTenthsTimerUp` (0x4bc14) each hold a `jg` (0x4b977,
0x4bb87, 0x4bca7) ahead of a `va` with `time %g should be > 0` (0x747e7).
`setTimerUp` (0x4ba04) goes from the round-up at 0x4ba6c straight to the
stores ending at 0x4bae5, with no compare. INFERRED: only `setTimerUp`
accepts a time at or below zero.

## 6. Round flow, live

`re`, mp_chateau. VERIFIED, both servers, the same server-command sequence
per probe up to the match start: `f MPSCRIPT_CONNECTED`, the
`team_britishgerman` menu, `cg_objectiveText`
`RE_OBJ_CHATEAU_OBJ_SPECTATOR`, then `ATTACKER` for allies and `DEFENDER`
for axis, `RE_MATCHSTARTING`, the restart (`n`), the move-in sounds, and
after the axis probe left, `RE_ELIMINATED_AXIS` with
`MP_announcer_allies_win`. The script's `logPrint` lines matched (`J`,
`Q`, `W;allies`, `L;axis`). The objective models are entities 85 and 86,
`eType` 8, on both servers along a `--probe-pvs` walk from the allied
spawn.

`bel`, mp_brecourt. VERIFIED, both servers: the `team_germanonly` menu,
the `BEL_SPECTATOR_OBJS` text, the second client moved to allies with
`weapon_americangerman` and `BEL_OBJ_ALLIED`, and (with section 5) the
same 10-second `bel_alive_tick` lines.

Differences seen in that run, none specific to these gametypes:

- Ours played the allied move-in sound once before the restart as well as
  after it; retail played it only after. VERIFIED, the two command
  sequences. INFERRED, a run artefact, not a divergence: the first
  `sayMoveIn` (`_teams.gsc`) waits two seconds from level start, and ours'
  debug build spent the four seconds before the first probe loading the
  map (its snapshots read `t=1950` three seconds after the connect,
  retail's `t=11100`), so on ours the probes were already in when it ran.
- The restart rebroadcasts configstrings 3, 12 and 13 on retail, with `t\0`
  in 3. vcod's restart now replays the level's write log and sends the
  same burst in retail's order (`cod11-map-cycle.md` 4.5,
  `tests/mapchange_ab.rs`).
- The stock scripts reach `addTestClient` only from their commented-out
  `scr_numbots` debug thread (`cod11-gsc-object-model.md` 25).

Sections 7 and 8 cover what this run left out: an `re` pickup, carry,
drop, return and capture, and a `bel` kill and the team swap it causes.

## 7. Retrieval: pickup, drop, return and capture

### 7.1 How the run was taken

Retail 1.1d Linux server on mp_brecourt, which has one objective (the code
book, model entity 129 on both servers) with a single spawn spot, so
`retrieval_spawn_objective` has no random pick to make. The gametype is
`client-probes/probe_re.gsc`, which calls `re::main()` and drives the run
from a thread:

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=<p> SECS=190 \
    tools/run_probe.sh client-probes/probe_re mp_brecourt
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-team allies \
    --save-scripted attacker --probe-secs 150      # 4 s after the server
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-team axis \
    --save-scripted defender --probe-secs 147      # 3 s after the first
```

`--save-scripted` presses use as the probe's
`setClientCvar("probe_use", ...)` says (`tap`: 100 ms every second;
`hold`), keeps the view the server sets, asks for `score` every 2 s, and
records every server command and every snapshot whose HUD, objective
block, roster or entities moved (`vcod_common::net::capture`). Fixtures:
`crates/server/tests/fixtures/gametypes/mp_brecourt-re-attacker.txt`,
`-defender.txt`, and the server's `games_mp.log` as `-re-log.txt`.

`crates/server/tests/gametypes_ab.rs` replays the same probe on ours with
two clients through the same recorder and compares them view by view. Ours
ticks in simulated time, so the gate hands it retail's host time off the
first role's capture (`RetailClock`): each restart's first snapshot past
the 350 ms a restart owing no frame takes, and each in-level stall (two
snapshots more than 1100 ms apart), run as owed frames back to back before
one snapshot, as retail's frame loop catches up (cod11-map-cycle.md 4.7).
VERIFIED, the `re` attacker capture: the three restarts' first snapshots
came 450, 1350 and 850 ms after their level starts (13850, 91450, 106600),
so 100, 1000 and 500 ms owed; no view moved with them. The UO `bas` runs
are where it shows (coduo-diff.md 22, "Gaps").

The script, after the match-start restart. Round 1 puts the attacker on
the objective's spot (`setOrigin(trigger.origin)`,
`setPlayerAngles((85, 0, 0))`) and taps use; moves it to
`(1432, -988, -37)` and holds use until `holduse` drops the book; waits out
`objective_timeout`; picks the book up again and calls `suicide()`. Round
2 puts the defender on the spot tapping use for 3 s, then the attacker
picks up and is set inside the goal's trigger. Every VERIFIED claim in 7.2
rests on the three fixtures.

### 7.2 What retail does

- Pickup. VERIFIED: the first use tap on the spot, looking down at the
  `trigger_use`, picks the book up inside 50 ms of the `probe_use "tap"`:
  `c "RE_OBJ_PICKED_UP_NOSTARS\x14RE_OBJ_CODE_BOOK\x15" 2` to both
  clients, the alias `re_pickup_paper` allocated (`d 530`) and played to
  the carrier alone (`s`). The use key reaches a `trigger_use` from 58
  units above its box centre with the view pitched 85 degrees down.
- Carrier, own view. VERIFIED: a text hudelem (type 1) at `635,385`, label
  `RE_U_R_CARRYING`, text `RE_OBJ_CODE_BOOK`, in the non-archived array;
  objective slot 2 reads `entNum 0` (the carrier) and `teamNum 2`.
- Carrier, the defender's view. VERIFIED: entity 0 carries `iHeadIcon`
  naming `gfx/hud/headicon@re_objcarrier.tga` and `iHeadIconTeam` 2; slot
  2 reaches the defender with state 0 (the team filter, object model doc
  23.3). The book entity stays in both snapshots with `eFlags` 0x100 while
  carried (`hide()`), and reads 0 again at the drop.
- The hold-use drop. VERIFIED: `pm_type` 1 (linked to `holduse`'s
  `script_origin`) for the hold; two shader hudelems (type 3), `black`
  292x12 at `320,385` with colour `0x80ffffff` and `white` at `176,385`,
  8 high, 7 or 8 units wider each frame. After 2 s:
  `RE_OBJ_DROPPED\x14RE_OBJ_CODE_BOOK`, the bar and the carry text gone,
  the book shown at `getPlant`'s point, slot 2 at `entNum 1023`,
  `teamNum 0`, origin the truncated plant point (7.4).
- The return. VERIFIED: 60 s after the drop,
  `RE_OBJ_TIMEOUT_RETURNING\x14` + `60`, the book and slot 2 back on the
  spot.
- Drop on death. VERIFIED: `suicide()` while carrying gives
  `RE_OBJ_DROPPED`, then `RE_ELIMINATED_ALLIES`, `d 5 1` (the axis team
  score) and `MP_announcer_axis_win`. The corpse is body-queue entity 64,
  the carbine drops as an item, and the book lands 11 units ahead of the
  body. VERIFIED, the log: the carrier's `objs_held` still reads 1 after
  the death (`drop_objective_on_disconnect_or_death` never decrements it)
  and 0 again after the restart.
- The defender's use. VERIFIED: `client_print`'s text hudelem at
  `320,200`, label `RE_PICKUP_ALLIES_ONLY`, text `RE_OBJ_CODE_BOOK`, for
  3 s; nothing else moves.
- Capture. VERIFIED: setting the carrier inside the goal brush fires it on
  the next touch pass: `RE_OBJ_CAPTURED\x14RE_OBJ_CODE_BOOK`,
  `RE_OBJ_CAPTURED_ALL`, `d 6 1`, `MP_announcer_allies_win`, the round
  restart 5 s later. The run's last scoreboard reads `b 2 1 1` with rows
  `1 0 _ 0 0` and `0 -1 _ 1 0`.

### 7.3 Head icons

In `game.mp.i386.so`:

- VERIFIED: `.headicon`'s setter (0x41bd4) passes the string through
  `GScr_GetHeadIconIndex` (0x5c840) and stores the result at
  `gentity+0x94`. The getter (0x41c14) reads it back, 0 as `""` (0x72e60)
  and `1..15` as configstring `0x1c + n`.
- VERIFIED: `.headiconteam`'s setter (0x41c84) compares `scr_const` slots and stores
  at `gentity+0x98`: `+0xf8` (`none`) 0, `+0x4` (`allies`) 2, `+0x8`
  (`axis`) 1. `+0x7c` (`spectator`) branches to `Scr_Error` with `'%s' is
  an illegal head icon team string. Must be none, allies, axis, or
  spectator.` (0x72e80). The getter (0x41d3c) names 1 `axis`, 2 `allies`,
  3 `spectator` and anything else `none`.
- VERIFIED, the netfield table: `iHeadIcon` is `entityState+148` (0x94) and
  `iHeadIconTeam` `+152` (0x98), so both setters write the wire fields
  directly.

INFERRED, off the fall-through at 0x41cfe: any other team string stores 3.

VERIFIED, `GScr_GetHeadIconIndex`: `""` answers 0. Otherwise configstrings
`0x1d..0x2b` are compared with `Q_stricmp` and the first match answers its
1-based position; no match reaches
`Scr_Error("Head icon '%s' was not precached\n")` (0x773a0).

VERIFIED, `.statusicon`'s getter (0x41b7c): 0 reads `""` and `1..8`
configstring `0x14 + n`.

#### The client draw (`cgame_mp_x86.dll` 1.1)

- VERIFIED: the player draw (0x30028210) calls the sprite pass 0x300274d0
  (call at 0x300282de) after it has skipped any entity whose `eFlags` has
  0x100 or 0x1 (tests at 0x3002822e and 0x30028249).
- VERIFIED: the pass needs `infoValid` (clientinfo `+0x0`, table
  0x3018bc0c, stride 0x448) for both the entity's `clientNum` (`es+0x90`)
  and the viewed client (`cg.snap->ps.clientNum`, snap `+0xb8`); the team
  is clientinfo `+0x2c` (0x3018bc38).
- VERIFIED, 0x30027523..0x3002753e: the head icon draws when `iHeadIcon`
  (`es+0x94`) is non-zero and `iHeadIconTeam` (`es+0x98`) is 0, the
  viewer's team is 3 (spectator), or the two teams match. The material is
  configstring `0x1c + iHeadIcon` registered at 0x30027549 (type 5).
- VERIFIED, the sprite (0x30027310, called with height 0 and no distance
  scaling): when the entity has a model and the `Bip01 Head` tag (string
  0x300630a8) resolves, the centre is that tag plus `18.0` up
  (0x300694d8); otherwise `lerpOrigin` (`cent+0x1f8`) plus `72.0` up
  (0x30069784). The refEntity is type 4 with radius `6.66` (0x3006977c),
  rotation 0 and shaderRGBA `ff ff ff ff`, handed to the renderer through
  trap `0x3d`. `renderfx` is 2 only for the viewed client's own body in
  first person.
- VERIFIED: the same pass then stacks the other player sprites, raising
  the height by `16.0` (0x30027565) when a head icon drew:
  `headiconYouInKillCam` for the killcam's subject,
  `headiconDisconnected` on `eFlags` 0x1000, a voice-chat icon while
  `cent+0x224 > cg.time`, and `headiconTalkBalloon` on `eFlags` 0x40000
  at `height - 5.0` (0x30069414); the last two only for a teammate or a
  spectating viewer. The killcam icon alone passes the distance flag:
  radius `6.66 * max(0.6, d * 1/256 + 0.2)` and centre raised
  `8 * scale - 8` (0x30027404..0x30027435).

#### The second sprite

- VERIFIED, 0x30027565..0x30027629: the height starts at 0 (0x300274e8)
  and becomes `16.0` only after a head icon's sprite call. The four arms
  after it are exclusive, each returning after its draw:
  1. the entity's number is `cg.clientNum` (0x301e2144) and
     `snap->ps.clientNum` is not: `headiconYouInKillCam`, distance flag 1,
     no team test;
  2. `eFlags & 0x1000`: `headiconDisconnected`, no team test;
  3. only when the two teams match or the viewer's is 3,
     `cent+0x224 > cg.time`: the voice-chat material at `cent+0x220`;
  4. same team test, `eFlags & 0x40000`: `headiconTalkBalloon`.

  Each height goes through `_ftol` (0x3005a890) as the sprite's integer
  parameter, so the talk balloon's is 11 over a head icon and -5 without
  one: its centre is the head bone plus 29 or 13, or the origin plus 83 or
  67.
- VERIFIED: the materials register at map load: `headiconDisconnected` and
  `headiconYouInKillCam` type 5 (0x301d5aac, 0x301d5ab0),
  `headiconVoiceChat` and `headiconTalkBalloon` type 2 (0x301d5aa4,
  0x301d5aa8). pak0's shader file maps them to `gfx/hud/disconnected.tga`,
  `gfx/hud/talkballoon.tga` and `gfx/hud/killcamindicator.tga` (the last
  `sort nearest`, `nofog`), each `blendFunc blend` with vertex colour and
  alpha.
- VERIFIED: the sprite's `renderfx` is 2 when `snap->ps` `pm_flags` has
  0x50000 (own view or follow), the entity is `snap->ps.clientNum` and
  `0x30207158` is 0 (0x30027331..0x3002734e). INFERRED: that hides every
  sprite of the body the camera sits in; vcod never draws that body, so it
  never builds its sprites.
- VERIFIED, game.mp.i386.so `ClientEndFrame` (0x40e98): a client's
  `s.eFlags` gets 0x1000 when `level.time` minus `gclient+0x21e4` (the
  last cmd's `level.time`, stored by `ClientThink` at 0x41592) is over
  1000, and the playerstate copy that follows overwrites it. VERIFIED by
  capture on 1.1d and UO: a player 5 s without a cmd is never sent the
  bit, so arm 2 never fires for a lagging player on a stock server
  (handshake doc, "A quiet client"). vcod-server does not set it either.

#### 1.5 and UO

- VERIFIED, 1.5 `cgame_mp_x86.dll` (sprite pass 0x30028840, sprite
  0x30028680) and UO `uo_cgame_mp_x86.dll` (0x30032ac0, 0x30032910): the
  same arms in the same order. The constants read the same: `5.0` (1.5
  0x3006b670, UO 0x3007bde0), `18.0`, `72.0`, `6.66`, `1/256`, `0.2`,
  `0.6`, `8.0`, and the `16.0` stack (UO 0x30032b55). 1.5's own-view mask
  is 0x30000 and UO's 0xc0000, their renumbered `pm_flags`. UO tests
  disconnected on 0x800 and the balloon on 0x20000, which
  `eflags_v22_to_v1` maps to 1.1's 0x1000 and 0x40000; 1.5 tests 1.1's
  bits.

vcod (`crates/client/src/head_icon.rs`) draws the head icon and arms 1, 2
and 4. It has no voice-chat icon (stock paks ship no `.voice` file), so the
balloon never yields to one.

### 7.4 `objective_position` detaches

VERIFIED: `objective_position` (0x5e128) opens with the same detach
`objective_onentity` makes: when the slot's `entNum` (`+0x10`) is not
0x3ff it clears `eFlags` bit 0x10 on that entity (0x5e190) and writes
0x3ff (0x5e197), then stores the origin. `re.gsc` relies on it:
`drop_objective` never touches the objective, and the `objective_position`
at the top of the `retrieval_think` it restarts pins the marker where the
book fell.

### 7.5 What ours got wrong, and the fixes

Each was a difference the gate showed. All are fixed; `gametypes_ab`'s
`GAPS` is empty.

- No head icon on the wire. `.headicon` and `.headiconteam` are indices in
  the client store, the getters answer as 7.3 says, and
  `ClientSim::to_entity` writes `iHeadIcon` / `iHeadIconTeam`, mirrored at
  the end frame. Unset, they read `""` and `"none"` (not `undefined`), and
  `.statusicon` `""`.
- `objective_position` must detach (7.4), or the dropped book's marker
  stays on the carrier's entity number.
- A client that never had a model sends `modelindex` 0, not the first
  empty model slot (`client_model_index` must not match `""` against the
  table).

Left alone. VERIFIED, the log fixture and ours: the landing after
`setOrigin` 1.66 units over the 1.8-degree grade at the drop point settles
at y -987.99 on retail and -987.96 on ours; the gate compares the log's
coordinates to the unit. VERIFIED, two retail runs that set the player
down 32 units up (not committed): one settled 22 units off in y, the next
0.03. The fall does not repeat on retail itself, so the probe sets the
player down on the ground.

## 8. Behind Enemy Lines: the team swap

### 8.1 How the run was taken

Same map, `client-probes/probe_bel.gsc` around `bel::main()`, two
`--save-scripted` probes that both answer the team menu with `axis`
(`--probe-team axis --save-scripted first` / `second`, 130 s). The probe
moves every player that spawns into play to a fixed spot by team, 88 units
apart (`(1432, -988, -37)` allied, `(1432, -900, -39)` axis), so bodies
and corpses do not ride on `getSpawnpoint_MiddleThird`'s pick. It kills
with `victim finishPlayerDamage(attacker, attacker, 1000, 0,
"MOD_RIFLE_BULLET", attacker getCurrentWeapon(), ...)`, which reaches
`Callback_PlayerKilled` with the attacker as a rifle round would. Kill 1:
the axis player kills the allied one. Kill 2: the same the other way
round. Kill 3: the allied player kills the axis one. Fixtures:
`mp_brecourt-bel-first.txt`, `-second.txt`, `-bel-log.txt`.

The probe's `log_spawns` writes one
`PROBE spawned <slot> <team> <time> <origin>` line per spawn into play,
from a thread that wakes on `"spawned"` and never waits on anything else,
so it reads the origin `spawnPlayer` left before the move hides it. Trap:
logging from the moving thread misses spawns (four of seven): a spectator
spawn's notify puts that thread in its `wait 0.05`, and the player spawn
after it notifies nobody. The fixtures were retaken with `log_spawns`;
every view the gate compares matched both captures.

### 8.2 What retail does

- Join. VERIFIED: the first probe, alone, answers `team_germanonly` with
  axis and `weapon_german`, and 2 s later is moved to allies: the
  blackscreen hudelems (`BEL_BLACKSCREEN_WILLSPAWN`, a timer, `black`
  640x480), `MPSCRIPT_JOINED_ALLIES`, the `weapon_american` menu, a spawn
  with the carbine and the allied HUD a frame and a half later: `black`
  130x35 at `505,382` colour `0x33ffffff`, a timer-up element (type 5)
  labelled `BEL_TIME_ALIVE` and a value element (type 2)
  `BEL_POINTS_EARNED` reading 1. The compass marker is objective slot
  `entnum + 1`, `teamNum 1`, icon `gfx/hud/hud@objective_bel.tga`.
- Kill 1. VERIFIED: `BEL_KILLED_ALLIED_SOLDIER` with the killer's name;
  both roster entries flip team on the kill's frame while each body model
  waits for its owner's next spawn. VERIFIED, the log: the killer respawns
  allied 2 s after the kill (`PROBE placed 1 allies`), with the carbine,
  the allied HUD and its own marker slot. The victim's view shows its
  killcam from 2 s after the death (`MPSCRIPT_KILLCAM`,
  `MPSCRIPT_PRESS_ACTIVATE_TO_RESPAWN`, a timer at `320,428`), and it
  spawns axis through `weapon_german` with the kar98k 7 s after the kill.
  The killer's own view shows the blackscreen with
  `BEL_BLACKSCREEN_KILLEDALLIED` until that respawn. Its score goes up by
  one at the marker's spawn (`make_obj_marker`) and by one every 10 s
  after (`bel_alive_tick`), the points element counting with it.
- Kill 2 is the same with the roles swapped. Kill 3 (axis victim) gives
  the allied killer a point and the victim a killcam and an axis respawn,
  with no swap.
- VERIFIED, both probes: a respawn empties the objective block. A slot
  whose record went to state 0 before the spawn reads all zero after it;
  before the spawn it kept its icon and origin under state 0.

### 8.3 `classname` before the first spawn

VERIFIED: `ClientConnect` calls `G_InitGentity` (0x4254b), which sets
`classname` to `scr_const+0x56` (0x6783c), allocated from `noclass`
(0x760ac) by `GScr_LoadConsts` (0x58963); `ClientSpawn` sets it to
`scr_const+0x62` (0x4271d), allocated from `player` (0x760e3, 0x589f3).
VERIFIED, the bel log: no state line ever lists a client still on
`Callback_PlayerConnect` (status icon `hud@status_connecting`); a server
whose connecting client reads "player" lists it with `pers["team"]`
undefined. `cod11-items.md`'s `PROBE other 0 noclass` reading is the same
fact.

### 8.4 What ours got wrong, and the fixes

- A client is `classname` "noclass" from its connect until
  `self spawn(...)` (`client_spawn`), so `getentarray("player",
  "classname")` agrees with retail through the connect callback.
- A spawn clears the client's objective copy. `ClientSpawn` zeroes the
  whole `gclient_t` (0x42804, 0x22c4 bytes), the copy at `client+0x3e8`
  with it; `client_spawn` resets the copy
  (`GameHost::reset_client_objectives`).

VERIFIED, the gate without these two fixes: the swap itself, the menus,
the HUD, the roster, the scores and the log already matched retail.

The marker's origin is its own view (`marker`). INFERRED, off `bel.gsc`'s
`make_obj_marker`: it reads the allied player's origin inside
`spawnPlayer`, before the probe moves it, and follows a running mean of
where the player stood, so it carries the random spawn pick. The gate
replays every pick off the log's `PROBE spawned` lines (8.7).

### 8.5 The rest of a client before its first spawn

`client-probes/probe_noclass` logs a client's fields from its connect
callback, 3 s later with no spawn, and straight after a `spawn`, on 1.1d,
1.5b and UO 1.51b (`crates/server/tests/fixtures/noclass/`, gated by
`crates/server/tests/noclass_ab.rs`).

- VERIFIED, all three logs, connect and 3 s on: `classname` "noclass",
  `targetname` undefined, `model` "", `origin` and `angles` (0, 0, 0),
  `health` and `maxhealth` 0, `sessionstate` and `sessionteam`
  "spectator", `score` and `deaths` 0, `statusicon` and `headicon` "",
  `headiconteam` "none", `spectatorclient` -1, `archivetime` 0, `isAlive`
  0, `isPlayer` 1, entity 0, `getentarray("player", "classname")` empty
  and `getentarray("noclass", "classname")` 1. After the spawn
  `classname` reads "player" and the two arrays swap; `health` stays 0
  (the probe sets none).
- VERIFIED, 1.1d `game.mp.i386.so`: `ClientConnect` writes
  `sess.sessionTeam` (`client+0x217c`) 3 at 0x42516 and
  `sess.sessionState` (`client+0x20d0`) 2 at 0x42520. The `sessionteam`
  getter (0x418fc) maps 0, 1, 2, 3 to `none`, `axis`, `allies`,
  `spectator` and pushes nothing for any other value.
- VERIFIED, the probe client's snapshots on all three: `ps.origin` reads
  (0, 0, 0) on every snapshot until the spawn. INFERRED, off `ClientBegin`
  (0x42f88), which writes only the connected state, calls `CalculateRanks`
  and notifies `begin`: nothing between the zeroing in `ClientConnect` and
  a script's `spawn` places the client.
- VERIFIED, the logs after the spawn: `origin` (992, -744, -3.09) on 1.1d
  and (992, -744, -4.01) on 1.5b and UO; `angles` (0, 0, 0) on 1.1d and
  (0, -180, 0) on 1.5b and UO, where the spot's are (0, 180, 0). 8.6 has
  why.

vcod: `sessionteam` reads "spectator" from the connect. Under a game
module a client enters at the origin with `delta_angles` 0
(`enter_world`); parking it at the map's first spawn with that spot's view
is wrong, since a script reads that as `origin` and the client as
`ps.origin`. A server with no scripts still parks it at the map's spawn,
since nothing would ever spawn it.

### 8.6 The origin and angles a script reads straight after `spawn`

`client-probes/probe_spawnview` spawns a playing client on mp_carentan's
first deathmatch spot (992, -744, 0), facing 180, with health 0 and 100,
100 higher, with angles (10, 90, 5) and (-30, -135, 0), at heights -4 to
16 around the spot, then in one frame as a player facing 90 and as a
spectator, and as an intermission camera, at (20, 45, 0) 64 up. It logs
`origin` and `angles` times 1000. Fixtures:
`crates/server/tests/fixtures/spawnview/mp_carentan{,-1.5,-uo}.txt`, gated
by `crates/server/tests/spawnview_ab.rs`.

VERIFIED, `GScr_AddFieldsForEntity`'s table (1.1d 0x78e68,
`tools/re/dump_script_fields.py`): `origin` reads `ent+0x134`
(`r.currentOrigin`) and `angles` `ent+0x140` (`r.currentAngles`).

Who writes them inside `ClientSpawn` (1.1d 0x4268c):

- VERIFIED: `G_SetOrigin` with the spot (0x429b7), then
  `SetClientViewAngle` (0x429e2), which stores the spot's angles into
  `ent+0x140..0x148` (0x4209f..0x420b1) and copies them into
  `ps.viewangles` (`ps+0xc0`), then `ClientEndFrame` (0x42a75) and
  `ClientThink_real` (0x42a82) on the null cmd.
- VERIFIED, `ClientThink_real`'s tail, after the link (0x40595) and the
  touches: `ent+0x134..0x13c` from `ps.origin` (0x405c7..0x405dc), then
  `ent+0x140` and `ent+0x148` zeroed and `ent+0x144` set to
  `ps.viewangles[1]` (0x405e2..0x40606).
- VERIFIED: `SpectatorThink` (0x3fab8) writes `ent+0x134..0x13c` after its
  `Pmove` (0x3fc16..0x3fc28) and nothing at `ent+0x140`, and
  `ClientThink_real`'s intermission arm returns before either (0x3ffcd).
- INFERRED: a playing client's spawn reads (0, view yaw, 0), its pitch and
  roll dropped; a spectator's and an intermission camera's keep the spot's
  angles.

The measurements:

- VERIFIED, all three logs: the spectator and the intermission camera read
  the spot's (20, 45, 0), and every playing spawn reads pitch and roll 0.
  1.1d reads yaw 0 at every spot: its think's cmd angles are the negated
  `delta_angles` (`ClientSpawn` 0x42a4e..0x42a69), so the view yaw is
  `SHORT2ANGLE(0)`. 1.5b and UO read the spot's yaw through `SHORT2ANGLE`,
  which takes 180 to -180 (90 stays 90, -135 stays -135): their think runs
  the client's last cmd at the spot's view (cod15-movement.md 4).
- VERIFIED, the 1.1d drops: from 8, 16 and 100 over the spot and 4 under
  it the body falls 4.0068 in the think's 66 + 34 ms; from 4, 2, 1 and 0
  over it and 1 and 2 under it, 3.0888. Spots 64 east and 64 north (a run
  not in the fixture) did not move at all.
- INFERRED, off the tail of `PmoveSingle`'s default arm
  (0x34398-0x3443d, cod11-mantle.md, "The tail of the default arm"):
  3.0888 is the 66 ms step's 1.7424 plus a 34 ms step from -26, so the
  first step left with the displacement over its time (26.4, snapped to
  26) instead of its -52.8. A fall from rest moves exactly half its end
  velocity, the tail's boundary, and on 1.1d's x87 stack the comparison
  falls either way by the last bits of the start height. vcod's tail
  evaluates both sides unrounded (`pmove::clamp_velocity_to_move`) and
  reproduces all ten 1.1d heights.
- VERIFIED, the 1.5b and UO drops: 4.0068 from every height but 2 under
  the spot, which falls 3.0888; a fall from 4 over the spot ends at
  -0.00680161 on both. Both rest on those builds' stored floats in the
  slide's gravity, the tail and the trace's end point (cod15-movement.md
  7), which vcod follows there and matches at every height.

vcod: the `spawn` builtin runs the think on a fresh sim
(`game/builtins/entity.rs`, `spawn_think`) and writes the origin it leaves
and, for a playing client, (0, the think's view yaw, 0); a spectator or an
intermission camera keeps the angles the script passed.

### 8.7 The spawn picks and the menu answer's clock

The gate (`crates/server/tests/gametypes_ab.rs`, `retail_spawns`) pins
each of retail's spawns into play on ours (`Server::pin_spawns`), at the
map's spawn point as `placeSpawnpoint` dropped it
(`game::builtins::entity::placed_origin`, cod11-gsc-object-model.md 16).
A capture whose probe hides the pick logs it (`PROBE spawned`); otherwise
the pick is the point under each life's first playing snapshot. Trap: a
capture without `PROBE spawned` lines shows none of bel's seven picks. The
allied spot `(1432, -988)` is itself an `mp_teamdeathmatch_spawn`, so two
lives pin to the probe's spot, two read `None`, and three respawns that
follow a killcam or a swap with no non-playing snapshot go uncounted.

- VERIFIED, the retaken capture: the first probe's `t 2`
  (`weapon_american`) rides snapshot 8000, its first playing snapshot is
  8050, and the log reads `PROBE spawned 0 allies 8050` and
  `PROBE placed 0 allies 8050`. Every spawn off a menu answer logs its
  `spawned` and `placed` on one `getTime()`; the three off a script thread
  (`respawn`'s `wait`) log `placed` 50 ms after `spawned`. INFERRED:
  `Cmd_MenuResponse_f`'s `Scr_Notify` (gsc language doc 6) only wakes the
  gametype's menu thread, which runs in `G_RunFrame`'s first
  `Scr_RunCurrentThreads` on the new `level.time`, ahead of the probe's
  `wait 0.05` due that frame (gsc language doc, "Engine notifies and the
  trigger queue").
- VERIFIED, vcod measurement (gsc language doc, "The packet pass"):
  `ScriptRuntime::menu_response` only notifies, from the move pass, and
  the menu thread runs in the next frame's first round on its new
  `level.time`. Ours logs the first pick on the first playing snapshot's
  time and places it on the same time, as retail does. `gametypes_ab`'s
  `spawn clock` rows, per role: the first pick's `getTime()` against the
  role's first playing snapshot (+0 on both), and each pick against its
  `PROBE placed` (+0 off a menu answer, +50 off `respawn`'s thread, the
  same seven on both). Trap: stepping the woken threads inside
  `menu_response` on the last frame's clock reads the first pick -50 and
  fails both rows.
- VERIFIED, retail 1.1d, 1.5b and UO 1.51b on mp_pavlov
  (`client-probes/probe_menuclock`, no stock gametype): a spectator whose
  `mr` wakes the connect thread, which spawns him, logs `PROBE spawned` on
  his first playing snapshot's `serverTime` on all three (1.1d and 1.5b
  17000, UO 17200), and so do the two respawns `Callback_PlayerKilled`
  runs after its `wait 0.5`. Each `PROBE placed` reads 50 past its
  `spawned`: that probe's placing thread starts its `wait 0.05` at the
  "spawned" notify, where `probe_bel`'s was already due from the
  spectator spawn before it. Ours reads the same six rows at each protocol
  (`crates/server/tests/menuclock_ab.rs`).
- VERIFIED, vcod measurement, the client order trap: the in-process join
  harness answers both clients' menus in one frame, and the first round
  runs the last woken thread first. Queuing the menu notify in
  `GameHost::engine_notifies` made client B spawn ahead of A and take A's
  spawn point, moving eleven tests. `common::join_pair_stamped` sends B's
  packets first once both clients are in (`step_pair_joining`, the
  `step_pair_b_first` order), so A's thread runs first and A spawns first,
  as on retail. Sending A's first instead fails the same eleven and
  nothing else: `combat.rs` 4, `killcam.rs` 1,
  `player_clip.rs` 3 and `turret_ab.rs`'s spawn, hitgrid and victim
  captures. The three `turret_ab` rows are retail captures, so their
  spawns are proven against retail's.
- VERIFIED, the gate: ours' seven `PROBE spawned` origins equal retail's
  to the hundredth, and so does every marker origin. VERIFIED, the
  retaken capture: retail's allied body leaves the probe's spot at
  `(1432, -988)` on the frame after the move (-987, then -986). VERIFIED,
  vcod measurement: with the gate's clients sending one cmd a frame, ours
  held the body one frame more and the marker's fifth mean read y -900
  where retail reads -901 (a truncation edge; `objective_position`'s
  vector is stored truncated, object-model doc 23.3). With the net probe's
  cadence, three cmds a frame 16, 16 and 18 ms apart at the snapshot's
  view (`common::ProbeHold`, `framed` and `aim`), every marker row matches
  on both roles and no other row of the gate moves, at 1.1d and UO: the
  slide off a `setOrigin` onto the grade moves only on a cmd's own time
  (AGENTS.md, "A fall onto a slope").
