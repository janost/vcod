# CoD2 1.3 gsc against CoD 1.1: builtins, fields, language, callbacks

What vcod's compiler and VM (`crates/gsc`, `cod11-gsc-language.md`,
`cod11-gsc-object-model.md`) and the server's builtin families
(`crates/server/src/game/builtins/*`) would need to run CoD2 1.3's stock MP
scripts.

Sources. `cod2_lnxded` is the retail Linux 1.3 dedicated server (md5
fefed4c4909fb7c5c13b13a003f5ccf7, version string `pc_1.3_1_1`), stripped, the
game module linked in, ET_EXEC with real pointers in `.data`. Addresses are
its own unless a claim says `mac`: the fully symbolized Mac 1.3 MP build's
i386 slice, used for names and for the compiler, whose Linux functions carry
no symbol. CoD 1.1 is `game.mp.i386.so` 1.1d (md5
de8947beb6f86fbfb46f5adfaab3d3ed) and the 1.1d `cod_lnxded`. Tools:
`tools/re/dump_cod2_gsc.py` (every table in sections 1 and 2, CoD2 only),
`tools/re/dump_builtins.py` and `dump_script_fields.py` (the 1.1 side),
`crates/gsc/examples/gsc_census.rs` (section 3.6).

The live probes in section 3 ran on retail `cod2_lnxded` on 127.0.0.1:28971
inside a network namespace with only loopback up (`unshare -rn`), because a
1.3 server at `dedicated 1` still resolves and heartbeats
`cod2master.activision.com` (VERIFIED, the console's `Sending heartbeat to
cod2master.activision.com`; the namespace refused the send). Each probe is a
gametype in the homepath's `maps/mp/gametypes/`, run with `+set developer 1
+set developer_script 1 +set g_gametype <probe> +map mp_toujane`, logging
through `logPrint`; the server warns `Could not load GameType description
file maps/mp/gametypes/<probe>.txt` and runs it anyway (VERIFIED). A compile
error ends the run with `Sys_Error: Error during initialization: script
compile error` after printing the file, line and message, which is how each
compile-time row below was read.

## 1. Builtin tables

### 1.1 Layout and lookup

- VERIFIED: `Scr_GetFunction` (0x8117cb2) walks `functions` at **0x8188040**
  (not 0x8187040, which holds unrelated 12-byte records), 145 records
  (loop bound 0x90), 12 bytes each `{name, fn, developer}`, as 1.1's.
- VERIFIED: every method record is now 12 bytes too (1.1's were 8). The four
  method tables: player 0x8158a40 (59, walked by 0x80fddac), scriptent
  0x815e3a0 (12, 0x811a1fa), generic entity 0x8188720 (59, 0x8117d50),
  hudelem 0x815a180 (18, 0x81047fe). The mac build names the first, second
  and fourth walker `Player_GetMethod`, `ScriptEnt_GetMethod` and
  `HudElem_GetMethod`; the Linux ones are matched by their table's first
  name (`giveweapon`, `moveto`, `settext`).
- VERIFIED: `Scr_GetMethod` (0x8117dea) calls the player, scriptent, generic
  and hudelem lookups in that order. INFERRED from its control flow: it calls
  all four and keeps the first non-null, and only the generic lookup writes
  the developer out-parameter. 1.1 searched player, scriptent, hudelem, then
  generic (`cod11-gsc-language.md` section 7); 1.5's order is CoD2's. No
  method name sits in two CoD2 tables (VERIFIED, the dump), so the order
  decides nothing for stock names.
- VERIFIED: a `developer` 1 record in CoD2: `print`, `println`, `print3d`,
  `line`, `assert`, `assertex`, `assertmsg`, `getentbynum`. 1.1 flags only
  `print`, `println`, `assert`. Every method record's third word is 0.

### 1.2 The diff against 1.1

1.1 has 106 functions and 110 methods (`dump_builtins.py`); CoD2 145 and 148.
Names compared case-folded, methods across all four tables. VERIFIED, both
dumps:

- Functions removed (1): `getweaponclassname`.
- Functions added (40): `spawnturret`, `precacheturret`, `assertex`,
  `assertmsg`, `isstring`, `bullettracepassed`, `sighttracepassed`,
  `physicstrace`, `getnorthyaw`, `int`, `issubstr`, `getsubstr`, `tolower`,
  `strtok`, `precacherumble`, `playrumbleonpos`, `playlooprumbleonpos`,
  `stopallrumbles`, `issplitscreen`, `soundexists`, `map`, `kick`, `ban`,
  `mapexists`, `isvalidgametype`, `matchend`, `allclientsprint`,
  `clientprint`, `setplayerteamrank`, `sendranks`, `endparty`,
  `setvotestring`, `setvotetime`, `setvoteyescount`, `setvotenocount`,
  `openfile`, `closefile`, `fprintln`, `freadln`, `fgetarg`.
- Methods removed (4): `allowcomplaint` (player); `lockdoor`, `unlockdoor`,
  `isdoorlocked` (generic).
- Player methods added (14): `getcurrentoffhand`, `switchtooffhand`,
  `getplayerangles`, `playerads`, `closeingamemenu`, `disableweapon`,
  `enableweapon`, `deactivatereverb`, `setchannelvolumes`,
  `deactivatechannelvolumes`, `setweaponclipammo`, `istalking`,
  `allowspectateteam`, `getguid`.
- Generic entity methods added (24): `getattachignorecollision`,
  `getammocount`, `getclanid`, `getclanname`, `getclandescription`,
  `getclanmotto`, `getclanurl`, `setstablemissile`, `playsoundasmaster`,
  `playrumble`, `playlooprumble`, `stoprumble`, `showtoplayer`,
  `setrightarc`, `setleftarc`, `settoparc`, `setbottomarc`,
  `enableaimassist`, `disableaimassist`, `updatescores`,
  `setteamfortrigger`, `clientclaimtrigger`, `clientreleasetrigger`,
  `releaseclaimedtrigger`.
- Hudelem methods added (4): `setplayernamestring`, `setmapnamestring`,
  `setgametypestring`, `setwaypoint`.
- Scriptent methods: unchanged.
- No method moved table: every CoD2 method 1.1 also has sits in the table
  1.1 had it in. No rename: the cvar builtins keep 1.1's script names
  (`getcvar`, `setcvar`, `makecvarserverinfo`) though the mac build's
  handlers are `GScr_GetDvar` and kin, and the fog pair keeps `setcullfog`
  and `setexpfog`.

### 1.3 The full CoD2 lists

Table order. `+` marks a name 1.1 lacks, `*` a `developer` 1 record.

**functions** (145): print\*, println\*, iprintln, iprintlnbold, print3d\*,
line\*, getent, getentarray, spawn, +spawnturret, +precacheturret,
spawnstruct, assert\*, +assertex\*, +assertmsg\*, isdefined, +isstring,
isalive, getcvar, getcvarint, getcvarfloat, setcvar, gettime, getentbynum\*,
getweaponmodel, getanimlength, animhasnotetrack, getbrushmodelcenter,
objective_add, objective_delete, objective_state, objective_icon,
objective_position, objective_onentity, objective_current, bullettrace,
+bullettracepassed, +sighttracepassed, +physicstrace, getmovedelta,
getangledelta, +getnorthyaw, randomint, randomfloat, randomintrange,
randomfloatrange, sin, cos, tan, asin, acos, atan, +int, distance,
distancesquared, length, lengthsquared, closer, vectordot, vectornormalize,
vectortoangles, anglestoup, anglestoright, anglestoforward, +issubstr,
+getsubstr, +tolower, +strtok, musicplay, musicstop, soundfade, ambientplay,
ambientstop, precachemodel, precacheshellshock, precacheitem, precacheshader,
precachestring, +precacherumble, loadfx, playfx, playfxontag, playloopedfx,
setcullfog, setexpfog, grenadeexplosioneffect, radiusdamage,
setplayerignoreradiusdamage, getnumparts, getpartname, earthquake,
newhudelem, newclienthudelem, newteamhudelem, resettimeout, isplayer,
isplayernumber, setwinningplayer, setwinningteam, announcement,
clientannouncement, getteamscore, setteamscore, setclientnamemode,
updateclientnames, getteamplayersalive, objective_team, logprint,
worldentnumber, obituary, positionwouldtelefrag, getstarttime, precachemenu,
precachestatusicon, precacheheadicon, map_restart, exitlevel, addtestclient,
makecvarserverinfo, setarchive, +allclientsprint, +clientprint, +mapexists,
+isvalidgametype, +matchend, +setplayerteamrank, +sendranks, +endparty,
+setvotestring, +setvotetime, +setvoteyescount, +setvotenocount, +openfile,
+closefile, +fprintln, +freadln, +fgetarg, +kick, +ban, +map,
+playrumbleonpos, +playlooprumbleonpos, +stopallrumbles, +soundexists,
+issplitscreen

**player** (59): giveweapon, takeweapon, takeallweapons, getcurrentweapon,
+getcurrentoffhand, hasweapon, switchtoweapon, +switchtooffhand,
givestartammo, givemaxammo, getfractionstartammo, getfractionmaxammo,
setorigin, setplayerangles, +getplayerangles, usebuttonpressed,
attackbuttonpressed, meleebuttonpressed, +playerads, isonground, pingplayer,
setviewmodel, getviewmodel, sayall, sayteam, showscoreboard, setspawnweapon,
dropitem, finishplayerdamage, suicide, openmenu, openmenunomouse, closemenu,
+closeingamemenu, freezecontrols, +disableweapon, +enableweapon, setreverb,
+deactivatereverb, +setchannelvolumes, +deactivatechannelvolumes,
getweaponslotweapon, setweaponslotweapon, getweaponslotammo,
setweaponslotammo, getweaponslotclipammo, setweaponslotclipammo,
+setweaponclipammo, iprintln, iprintlnbold, spawn, setentertime,
cloneplayer, setclientcvar, islookingat, playlocalsound, +istalking,
+allowspectateteam, +getguid

**scriptent** (12): moveto, movex, movey, movez, movegravity, rotateto,
rotatepitch, rotateyaw, rotateroll, rotatevelocity, solid, notsolid

**generic entity** (59): attach, detach, detachall, getattachsize,
getattachmodelname, getattachtagname, +getattachignorecollision,
+getammocount, +getclanid, +getclanname, +getclandescription, +getclanmotto,
+getclanurl, linkto, unlink, enablelinkto, getorigin, geteye, useby,
+setstablemissile, istouching, playsound, +playsoundasmaster, playloopsound,
stoploopsound, +playrumble, +playlooprumble, +stoprumble, delete, setmodel,
getnormalhealth, setnormalhealth, show, hide, +showtoplayer, setcontents,
setcursorhint, sethintstring, shellshock, stopshellshock, viewkick,
localtoworldcoords, +setrightarc, +setleftarc, +settoparc, +setbottomarc,
getentitynumber, enablegrenadetouchdamage, disablegrenadetouchdamage,
enablegrenadebounce, disablegrenadebounce, +enableaimassist,
+disableaimassist, placespawnpoint, +updatescores, +setteamfortrigger,
+clientclaimtrigger, +clientreleasetrigger, +releaseclaimedtrigger

**hudelem** (18): settext, +setplayernamestring, +setmapnamestring,
+setgametypestring, setshader, settimer, settimerup, settenthstimer,
settenthstimerup, setclock, setclockup, setvalue, +setwaypoint,
fadeovertime, scaleovertime, moveovertime, reset, destroy

### 1.4 Against vcod's builtin families

vcod answers 217 names (every family's `NAMES` plus `host::BUILTINS`, read
off the source, 2026-10-21). 118 of CoD2's 290 builtin names are not among
them; 26 of those are called by the stock MP scripts (section 3.6's census,
the MP subset, calls that resolve to a table name): `assert`, `assertex`,
`print`, `line`, `int`, `vectordot`, `bullettracepassed`, `precacherumble`,
`playrumble`, `stoprumble`, `issplitscreen`, `endparty`, `sendranks`,
`setplayerteamrank`, `updatescores`, `closeingamemenu`, `getcurrentoffhand`,
`switchtooffhand`, `getplayerangles`, `setweaponclipammo`, `getammocount`,
`setnormalhealth`, `setteamfortrigger`, `clientclaimtrigger`,
`clientreleasetrigger`, `setwaypoint`. Of these, `print`, `line` and the
asserts sit in developer blocks or are compiled out (section 3.3); the rumble,
splitscreen, rank and party builtins are console features a PC server
answers with nothing (INFERRED from their names; their handlers are unread).

## 2. Fields and spawn classnames

### 2.1 Entity fields

VERIFIED, `GScr_AddFieldsForEntity` (0x811ac48) over 0x815e820, 16-byte
records `{name, offset, type, setter}`, `type` the `fieldtype_t` 1.1 uses
(the mac stabs spell it `F_INT 0, F_FLOAT 1, F_LSTRING 2, F_STRING 3,
F_VECTOR 4, F_ENTITY 5, F_VECTORHACK 6, F_OBJECT 7, F_MODEL 8`): 10 fields
where 1.1 has 34.

| field | type | setter |
|---|---|---|
| classname | string | read-only |
| origin | vector | 0x8117e62 (`G_SetOrigin`, links) |
| model | model | read-only |
| spawnflags | int | read-only |
| target, targetname | string | |
| count, dmg | int | |
| health | int | `Scr_SetHealth` |
| angles | vector | 0x8117ea6 (`G_SetAngle`) |

Gone from the engine table: `speed`, `closespeed`, `message`, `teamname`,
`wait`, `random`, `light`, `duration`, `rotate`, `degrees`, `time`,
`_color`, `color`, `key`, `harc`, `varc`, `delay`, `radius`,
`missionlevel`, `start_size`, `end_size`, `shard`, `spawnitem`, `track`.
Several return as Radiant keys: CoD2's `radiant/keys.txt` (in `iw_07.iwd`,
256 lines) declares `float delay` and `int radius` among its typed keys
(VERIFIED). `GScr_AddFieldsForRadiant` (0x811ac96) passes `"radiant"` and
`"txt"` to 0x807facc, which builds `radiant/%s` for each name a listing
returns and parses it (VERIFIED, the strings at 0x815e682 and the
`sprintf`); that the listing is every `radiant/*.txt`, where 1.1 read the
one `radiant/keys.txt`, is INFERRED from that call shape (0x80a19bc, the
listing, is unnamed).

### 2.2 Client fields

VERIFIED, 0x80f84be over 0x8157ce0, 20-byte records `{name, offset, type,
setter, getter}`, 13 fields: `name` (lstring, read-only), `sessionteam`,
`sessionstate`, `maxhealth`, `score`, `deaths`, `statusicon`, `headicon`,
`headiconteam`, `spectatorclient`, `archivetime` (float), `psoffsettime`
(int, new), `pers` (object). Against 1.1: `handicap` is gone,
`psoffsettime` is new; the ids stay tagged `| 0xc000` (the walker ORs
0xffffc000 into the index, VERIFIED).

### 2.3 Hudelem fields

VERIFIED, 0x8103dca over 0x8159ee0, 28-byte records `{name, offset, type,
mask, shift, setter, getter}`, 15 fields, registered as class 1:

| field | type | offset | mask/shift |
|---|---|---|---|
| x, y, z | float | 4, 8, 12 | |
| fontscale | float | 16 | |
| font | int | 20 | |
| alignx / aligny | int | 24 | 3 << 2 / 3 << 0 |
| horzalign / vertalign | int | 28 | 7 << 3 / 7 << 0 |
| color, alpha | int | 32 | |
| label | int | 48 | |
| sort | float | 120 | |
| foreground | int | 124 | |
| archived | int | 136 | |

Against 1.1 (11 fields, 20-byte records): `x` and `y` turn from int to float,
`z`, `horzalign`, `vertalign` and `foreground` are new, and the align pairs
pack into bitfields of one word. The `font`, align and `color`/`alpha`
fields have getters and setters (string names in, INFERRED from 1.1's
equivalents; the CoD2 handlers are unread).

### 2.4 Spawn classnames

VERIFIED, `G_CallSpawn`'s loop over 0x8188a00 (8-byte `{name, fn}`, null
ended), 21 classnames: `info_null`, `info_notnull`, `func_group`,
`trigger_multiple`, `trigger_radius`, `trigger_disk`, `trigger_hurt`,
`trigger_once`, `light`, `misc_model`, `misc_mg42`, `misc_turret`, `corona`,
`trigger_use`, `trigger_use_touch`, `trigger_damage`, `trigger_lookat`,
`script_brushmodel`, `script_model`, `script_origin`, `script_struct`.
Against 1.1's 27: `func_bobbing`, `func_door`, `func_door_rotating`,
`func_pendulum`, `func_rotating`, `func_static`, `misc_spawner`,
`misc_teleporter_dest`, `mp_target_location` and `target_location` are gone;
`script_struct`, `trigger_disk`, `trigger_radius` and `trigger_use_touch`
are new. INFERRED, `G_CallSpawn`'s control flow: an item classname still
goes to `G_GetItemForClassname` before this table.

`script_struct` (0x811ee50) builds no entity: the load runs
`codescripts/struct::initstructs` (`level.struct = []`) and each
`script_struct` calls `codescripts/struct::createstruct`, which appends a
`spawnstruct()` to `level.struct` and returns it; 0x811a828 then writes the
map entity's keys onto that struct (VERIFIED, the handles at 0x884e7b0 and
0x884e7b4 bound in 0x811041e, the `createstruct` call at 0x811a828, and
`codescripts/struct.gsc` in the stock iwds; the key copy is INFERRED from
0x811a828's loop over the parsed spawn pairs).

## 3. Language and compiler

The compiler is a yacc parser over a flex lexer; the keywords live in the
flex DFA tables, so there is no keyword string table to dump (VERIFIED: no
`waittillframeend`, `waittill` or `endon` string in the binary). The
statement and expression node kinds are read off the mac `EmitStatement`
(mac 0x971e2) and its callees instead.

### 3.1 `#include`

- VERIFIED, 1.1d's `cod_lnxded` has none of the strings `Duplicate
  #include` or `function '%s' already defined in '%s'`; CoD2 has both
  (0x8144445, 0x8143be0). `%s recursively included`, in both, belongs to
  the menu preprocessor (INFERRED: its CoD2 user, 0x804aa0e, sits among the
  `PC_*` source functions beside `SourceError`).
- VERIFIED, probe `probe_c2b`: `#include maps\mp\gametypes\_c2one;` makes
  `_c2one`'s functions callable unqualified (`one_fn()` returned
  `one+two`, its own call into a file it includes working inside it).
- VERIFIED, `probe_c2c`: inclusion is not transitive. The probe includes
  `_c2one`, which includes `_c2two`; calling `two_fn()` from the probe is
  `unknown function`, a compile error.
- VERIFIED, `probe_c2b`: a builtin beats an included function of the same
  name. `_c2one` defines `getcvar(a)`; the probe's unqualified
  `getcvar("sv_hostname")` returned the cvar (`CoD2Host`), and
  `maps\mp\gametypes\_c2one::getcvar("x")` the script's string.
- VERIFIED, `probe_c2e`: a function defined in the same file beats a
  builtin: a local `getcvar(a)` answered the unqualified call. The mac
  `EmitCall` (mac 0x9575c) tries the builtin table only when the name is
  absent from the file's own function object (INFERRED, control flow).
- VERIFIED, `probe_c2d`: a local function with the name of an included one
  is a compile error, `function 'one_fn' already defined in
  'maps/mp/gametypes/probe_c2d.gsc'`, reported at the `#include` line.
- INFERRED, the include pass in `ScriptCompile` (Linux 0x8075a26, the one
  function holding `Duplicate #include`; mac 0x9a0ca): includes are 12-byte
  records `{u16 canonical name, u8 first, int source position}`; after the
  file's own functions are emitted each included file is loaded through
  `Scr_LoadScript`, a second include of the same file in one script is
  `Duplicate #include`, and each function the included file defines is
  bound into the including file through `SpecifyThreadPosition` (0x80711ce)
  and `LinkThread`, which is where the duplicate-name error comes from.
- Stock use: 243 `#include` lines in 159 of the 622 scripts; in MP 4 files
  (`maps/mp/_createfx.gsc`, `_fx.gsc`, `_shutter.gsc`,
  `gametypes/_weapons.gsc`), all `#include maps\mp\_utility;` (VERIFIED,
  grep). The MP functions reached only through it: `abs`,
  `array_levelthread`, `exploder`, `vectorscale`.
- VERIFIED, `compile::tests` and `load::tests` (`a_cod2_include_*`,
  `an_include_imports_*`, `a_builtin_beats_*`): under `Dialect::Cod2` the
  lexer makes `#include` a directive and
  `compile::compile_file_with` resolves an unqualified call against the
  file's own functions, then the host's builtin table
  (`CompileOptions::is_builtin`, `Loader::set_builtins`), then the
  included files' functions, reading each included file's names through
  `CompileOptions::include_funcs`. A bare `::name` pointer resolves
  local first, then included, never to a builtin (INFERRED: unprobed on
  retail). Two includes that define one name are refused with the
  local-duplicate message (INFERRED from `LinkThread` being the one
  check; no stock script does it). `Loader` loads every included file
  whether or not anything calls into it, as the include pass does.

### 3.2 `waittillframeend`

- VERIFIED: statement node 0x39 compiles to opcode 0x4d through Linux
  0x8072858 (mac `EmitStatement` case 0x39), which writes no operand.
- INFERRED, the mac `VM_Execute` (mac 0x8393a), op 0x4d: the thread's stack
  is archived and appended with `GetNewObjectVariableReverse` to the list of
  threads due at the current time, where `wait` (the op with `negative wait
  is not allowed`) uses `GetNewObjectVariable` at now plus the wait. A
  frame-end waiter therefore runs after every `wait 0` due that frame and in
  the order the waits were reached.
- VERIFIED, probe `probe_c2a` at `getTime()` 1200: `main` starts, in order,
  `fe1` (`waittillframeend`), `w0a` (`wait 0`), `fe2`, a `w05` (`wait
  0.05`), a `waiter` on `level waittill("go")`, `w0b`; notifies `go`; then
  itself reaches `waittillframeend`. Logged order: `waiter_woke`, `w0b_end`,
  `w0a_end`, `fe1_end`, `fe2_end`, `main_after_fe`, `waiter_fe`, then the
  two threads `fe1` and `fe2` started after waking (each one more
  `waittillframeend`), all at 1200; `w05` at 1250, after `main`'s own
  `wait 0.05`. So `wait 0` resumes newest first in the frame it was reached
  (as 1.1, `cod11-gsc-language.md` section 9), frame-end waiters resume
  oldest first after them, and a frame-end wait reached while the frame-end
  list runs still runs that frame.
- Stock MP use: 12 sites, in all five gametypes (`ctf`, `dm`, `hq`, `sd`,
  `tdm`) (VERIFIED, grep).
- VERIFIED, probe `probe_c2n` (`fe1` and `fe2` each reach
  `waittillframeend`, then `fe1` takes a `wait 0`): logged `fe1_end`,
  `fe1_w0_end`, `fe2_end`. A `wait 0` reached off the frame-end list
  resumes ahead of the frame-end waiters still queued, as the head-insert
  model predicts.
- VERIFIED, a 1.1d probe on `cod_lnxded` (the c2a shape cut to `fe`, `w0`
  and `main` each reaching `waittillframeend;`): `******* script compile
  error *******` / `bad syntax`, the marker under the `;`. `x = 1; x;` on
  its own line gives the same `bad syntax`, so 1.1d has no
  `waittillframeend` keyword and a bare local is no statement there.
  INFERRED, 1.1d `VM_Execute` (0x80a45f0 and its twin at 0x80c8634):
  each has two calls to the thread-archive function 0x80a9224, under the
  `wait` case (0x24) and the `waittill` cases (0x4f, 0x50), where CoD2's
  has three (op 0x4d's the third); the 1.1d switch runs ops 0..0x59, CoD2's
  0..0x86.
- VERIFIED, the tests named here: under `Dialect::Cod2`
  `waittillframeend` is a keyword compiling to
  `Op::WaitTillFrameEnd`; the scheduler parks the thread as
  `ThreadState::FrameEnd(now)`, picked after every thread due at that
  clock reading and oldest first (`sched::Turn`), so the c2a and c2n
  orders replay (`sched::tests::waittillframeend_runs_behind_the_frame_s_wait_0s`,
  `a_wait_0_inside_the_frame_end_list_runs_before_the_rest_of_it`). No
  host hook is needed. Under `Dialect::Cod1` vcod still parses the line as
  an expression statement reading a local of that name (`LoadLocal, Pop`),
  where 1.1d refuses it and any bare local as `bad syntax`; no stock 1.1
  script has one, so the divergence is kept (`cod11-gsc-language.md`
  section 10).

### 3.2.1 Notify wake order

- VERIFIED live (cod2-protocol.md 15.5): a notify resumes its waiters
  newest `waittill` first, where 1.1 resumes them in start order. The
  stock `connecting` waiters show it on retail's first two connects.
  INFERRED: the waiters sit on a list the newest `waittill` heads. vcod:
  `Vm::set_newest_waiter_first`, on in `VmProfile::COD2`.

### 3.2.2 Writing undefined into an array

- VERIFIED live (`probe_array_undef`, `probe_undef_into_unset`,
  `probe_undef_call_into_unset` on `cod2_lnxded` 1.3, 2026-10-22): the
  same lines as 1.1d. The literal `x[k] = undefined` removes the key and
  is fatal (`undefined is not an array`) on a never-set local; an
  undefined value from a call is stored and counted by `.size`.
  `cod11-gsc-language.md` section 9 has the measurement and vcod's ops.

### 3.3 Developer builtins and `/# #/`

- VERIFIED, `probe_c2f`: `x = getEntByNum(0);` outside a `/# #/` block is a
  compile error, `return value of developer command can not be accessed if
  not in a /# ... #/ comment` (raised in Linux 0x8070b42 and 0x8070d66). 1.1d's `cod_lnxded` carries the same rule worded `developer
  command can only be used as a statement if not in a /# ... #/ comment`
  (VERIFIED, the string).
- VERIFIED, `probe_c2g` with `developer_script 1`: a `/# #/` block runs and
  a bare `assertEx(false, "boom")` is a runtime error (`assert fail:
  boom`); with `developer_script 0` the block does not run and the same
  `assertEx` is skipped, the next statement logging. INFERRED from mac
  `EmitCall`: a developer builtin used as a statement is compiled as if
  wrapped in `/# #/`.
- INFERRED, mac `ScriptCompile`: `/#` and `#/` may also bracket whole
  function definitions at file scope (nodes 0x45, 0x46 around 0x44); such
  functions are compiled only under `developer_script`, and a normal
  function that calls one is `normal script cannot reference a function in
  a /# ... #/ comment`. `#using_animtree` inside such a block is `cannot put
  #using_animtree inside /# ... #/ comment` (Linux 0x80757fc, VERIFIED the
  string). No stock MP script brackets a function that way (VERIFIED,
  census passes without it).

### 3.4 Compile-time typing of `self`, `level`, `game`

1.1 refuses `level[k]` and `self[k]`, read or written, at compile time with
`not an array, string, or vector`, and `game.x` with `not an object`
(`cod11-gsc-language.md` section 9; vcod's `Compiler::check_array`,
`check_object`). CoD2:

- VERIFIED: the string `not an array, string, or vector` is absent from
  `cod2_lnxded`.
- VERIFIED, `probe_c2a`: `arr thread selfidx()` with `selfidx()` reading
  `self.size` and `self[1]` logs `2 one`: a bracket read on `self`
  compiles and works when `self` is an array.
- VERIFIED, `probe_c2m`: `level["k"]` read compiles and fails at run time,
  `struct is not an array`.
- VERIFIED, `probe_c2k` and `probe_c2m`: `level["k"] = "v"` and
  `self[0] = "written"` are compile errors, `not an lvalue` (Linux
  0x8072352, 0x80721e8, 0x80725ce).
- VERIFIED, `probe_c2i`: `game.x = 1` is still `not an object` (Linux
  0x8071772, 0x80723da, 0x80724b2). The mac object emitter
  (`EmitPrimitiveExpressionFieldObject`, mac 0x96ad2) accepts three keyword
  nodes (0x20, 0x22, 0x24; opcodes 0x26, 0xd, 0xe), a variable, a call and
  a parenthesised one of those, and nothing else (VERIFIED, the switch).

Stock SP scripts read `self[i]` (`maps/_tankai.gsc:49`, `maps/toujane.gsc`,
`maps/toujane_ride.gsc`). vcod under `Dialect::Cod2` compiles a bracket
read on `self`, `level` and `anim` and refuses the write as `not an
lvalue` (`Compiler::check_index_write`); `Dialect::Cod1` keeps 1.1's
refusal of both (VERIFIED,
`compile::tests::cod2_reads_self_and_level_by_index_but_refuses_the_write`).

### 3.5 Other compiler changes

- VERIFIED, `probe_c2h` and `probe_c2l`: reading a local that is not
  assigned on every path to the read is a compile error, `uninitialised
  variable 'y'` (the local assigned under one `if` arm) and the same for a
  local never assigned (`isDefined(x)`), from `Scr_FindLocalVarIndex`
  (0x806e9b2). Assigned in both arms of an `if`/`else` it compiles
  (`probe_c2m`). 1.1d has no such string (VERIFIED); there an unassigned
  local reads undefined. VERIFIED, `compile::tests::cod2_refuses_*` and
  `definite_assignment_*`: vcod under `Dialect::Cod2` runs a forward
  definite-assignment pass (`crates/gsc/src/definite.rs`): sets meet at
  joins, a loop body is checked against its entry set, a constant-true
  loop leaves only by `break`, code past `return`, `break` or `continue`
  reads nothing, `waittill`'s binds assign, and `x[i] = v` assigns the
  `x` it creates (`probe_autoviv`). How retail's pass treats loops and
  constant conditions is not established; the stock corpus passes vcod's
  (3.6).
- VERIFIED, `probe_c2a`: `x = thread retfn();` compiles, and `x` is
  undefined though `retfn` returns 5 after a wait. VERIFIED live
  (retail 1.3, 2026-10-22, a `dm.gsc` that stored each result in a dvar
  read back over rcon): a thread that returns before it suspends hands its
  value over, `r = thread f()` with `f() { return 7; }` read 7 and a
  `spawnstruct()` return its field; with a `wait 0.05` first it read
  undefined. Stock `_fx::soundfx` writes `ent.delay` on such a return
  (`createfx_showOrigin`'s struct), and a script error there on vcod
  ended mp_burgundy's level script. vcod: `Vm::set_thread_returns`, on
  for CoD2. VERIFIED,
  `compile::tests::cod2_assigns_a_thread_call_s_undefined`: vcod under
  `Dialect::Cod2` parses a receiverless `thread` call as an assignment's
  right side; a threaded call pushes undefined. Under `Dialect::Cod1` it
  stays a parse error (`expected an expression, found Thread`).
- VERIFIED, compiler and VM error strings only one side carries (each
  binary's `strings`, filtered to those messages): CoD2 adds `uninitialised variable '%s'`, `unreachable code`,
  `conditional expression cannot be always false`, `not an lvalue`, `not an
  entity`, `self field in assignment expression not currently supported`,
  `self field can only be used in the script debugger`, `$ can only be used
  in the script debugger`, `bad expression`, `type %s is not a float`,
  `missing case statement`, `/# has no matching #/`, `#/ has no matching
  /#`, `VALUE_STACK_SIZE exceeded`, and the `#include` pair; 1.1d's
  `waittill not allowed in /# ... #/ comment (call as a thread to fix)`,
  `waittill not allowed in developer script` and `waittillmatch not allowed
  in developer script` are gone, as are its `cannot cast %s to ...` family.
  Which of these a stock script can reach is not established.
- VERIFIED, the trace probe on the live server (`cod2-d3dbsp-format.md`,
  "The trace A/B"): `==` between floats has a tolerance, `0.00000001 == 0`
  true and `0.000001 == 0` false, and a float becomes a string through
  `%g` (0x08079e74), so a probe that needs a float's last bits prints them
  as an integer built by exact halvings and doublings.
- VERIFIED, `SL_GetStringForVector` (lnxded 0x8079f00, Mac's same
  string): a vector becomes a string as `(%g, %g, %g)`, where 1.1 prints
  two decimals; the combat captures' `PROBE` lines read `(1936, 1783,
  65)` and `(1928.99, 1766.65, 120.048)` (cod2-combat.md 9). vcod:
  `Interner::vectors_g`, set for a CoD2 level.
- VERIFIED, `Scr_EvalDivide` (Mac 0x9012c): two ints divide as floats
  (`(float)a / (float)b`, the result a float), where 1.1 truncates to an
  int; a zero divisor is the divide-by-zero error either way. The killed
  gunner capture's `wait 1500 / 1000` waited 1.5 s on retail
  (cod2-turrets.md 14). vcod: `Op::DivFloat`, which the compiler emits
  for `/` and `/=` under `Dialect::Cod2`.
- VERIFIED, `Scr_EvalMultiply` (lnxded 0x807e290): `*` takes two numbers
  only, an int promoted to a float beside a float (0x807d7aa); a vector
  operand is `pair '(1, 0, -0)' and '200' has unmatching types 'vector'
  and 'int'`, which stopped retail's server live (cod2-protocol.md 16.8).
  vcod: `Op::MulNumber` under `Dialect::Cod2`; CoD1's VM scales.
- VERIFIED live: a string literal running over a line break is a compile
  error, `bad token '"'` at the opening quote (cod2-protocol.md 16.8).
  vcod's lexer refuses it under `Dialect::Cod2` only.
- VERIFIED live (cod2-protocol.md 16.8): a thread an `endon` ends while
  it sits under a `thread` call whose new thread notifies runs no further
  instruction; stock tdm's killed callback never logs its `K;` line at
  the score limit. vcod: `Vm::set_kill_mid_step`, on for a CoD2 level;
  1.1's VM runs such a thread on (cod11-gsc-language.md section 10).
- VERIFIED: `#using_animtree("...")` and `#animtree` keep 1.1's spelling (200
  and 62 uses in the stock iwds); `animtrees/%s.atr`, `#using_animtree was
  not specified` and `type %s is not an animtree` are in the binary.

### 3.6 The stock corpus through vcod's compiler

`gsc_census` over every `.gsc` the stock iwds hold (622 after later iwds
override earlier ones: `iw_07`, `iw_08`, `iw_13`, `iw_15`), 2026-10-21. MP
subset: `maps/mp/**`, `codescripts/`, `mptype/`, `character/mp_*`,
`xmodelalias/mp_*`, 131 files. VERIFIED, the run:

| failure | all | MP | first site |
|---|---:|---:|---|
| `unknown directive '#include'` (parse) | 159 | 4 | `maps/mp/gametypes/_weapons.gsc:1` |
| `self[i]` read, `not an array, string, or vector` | 3 | 0 | `maps/_tankai.gsc:49` |
| `x = thread f()`, `found Thread` | 1 | 0 | `maps/silotown_assault.gsc:3233` |
| retail rejects it too (`bad syntax`) | 2 | 0 | `animscripts/traverse/stairs_down.gsc:20`, `stairs_up.gsc:29` |

With every `#include` line blanked the other 616 compile (6648 functions)
and pass `stack_depth`. The two `stairs_*` scripts are broken in the
shipped data: retail's compiler stops on them with `bad syntax` when a
gametype references them (VERIFIED, probes `probe_c2_stairs_down` and
`_up`). Silent miscompiles the census cannot see, under 1.1's grammar:
`waittillframeend` (3.2), and any read of an unassigned local (3.5);
`Dialect::Cod2` handles both (below).

The same 622 files through `DIALECT=cod2 gsc_census` with `BUILTINS` set to
CoD2's 290 builtin names (section 1.3), 2026-10-21 (VERIFIED, the run): 620
compile (7067 functions) and pass `stack_depth`, all 131 MP files among
them; the two failures are the `stairs_*` pair above. No MP call is left
unresolved (`UNKNOWN` lines: none in MP), so `abs`, `array_levelthread`,
`exploder` and `vectorscale` reach `maps/mp/_utility`. The SP files'
`UNKNOWN` lines are SP builtins (`getnode`, `getaiarray`, `setgoalnode`,
...) the MP table lacks. No stock script trips vcod's definite-assignment
pass or the `not an lvalue` rule.

## 4. Callbacks, engine notifies, script loading

### 4.1 CodeCallbacks

VERIFIED, 0x8110286 binds five handles in
`maps/mp/gametypes/_callbacksetup`, the same five names 1.1 uses:
`CodeCallback_StartGameType`, `CodeCallback_PlayerConnect`,
`CodeCallback_PlayerDisconnect`, `CodeCallback_PlayerDamage`,
`CodeCallback_PlayerKilled`, plus `main` in `maps/mp/gametypes/<g_gametype>`;
0x811037a binds `maps/mp/<mapname>::main`, optional. Two signatures grew:

- `Scr_PlayerDamage` (0x81183b8) runs the damage callback with 10
  arguments (1.1: 9): `eInflictor, eAttacker, iDamage, iDFlags,
  sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc, timeOffset`, the last an
  int (VERIFIED, the `Scr_Add*` calls and the count 10).
- `Scr_PlayerKilled` (0x8118492) runs the killed callback with 9 (1.1: 7):
  `... sHitLoc, timeOffset, deathAnimDuration`, both ints (VERIFIED, count
  9). The stock `_callbacksetup.gsc` names the extra parameters so.
- Means of death passed as strings off 0x8187c40, 15 names (VERIFIED):
  `MOD_UNKNOWN`, `MOD_PISTOL_BULLET`, `MOD_RIFLE_BULLET`, `MOD_GRENADE`,
  `MOD_GRENADE_SPLASH`, `MOD_PROJECTILE`, `MOD_PROJECTILE_SPLASH`,
  `MOD_MELEE`, `MOD_HEAD_SHOT`, `MOD_CRUSH`, `MOD_TELEFRAG`, `MOD_FALLING`,
  `MOD_SUICIDE`, `MOD_TRIGGER_HURT`, `MOD_EXPLOSIVE`; out of range is
  `badMOD`.

`codescripts/delete::main` (bound in 0x811041e, run on an entity by
0x811ef90, which a trigger with spawnflag 0x10 and `Activate_trigger_damage`
call) and the struct pair (2.4) are the other engine-bound scripts
(VERIFIED, the bindings and the call sites).

### 4.2 Engine notifies

VERIFIED: every `Scr_Notify` in `cod2_lnxded` with a constant event name,
read off `GScr_LoadConsts` (0x81224f8, which assigns each `scr_const` slot
from `GScr_AllocString`):

| event | args | raised by |
|---|---:|---|
| `touch` | 1 | 0x80f4ab6 (the player's touch pass, INFERRED from its per-class touch table at 0x8187d0c), `Player_UseEntity` 0x8121054 |
| `trigger` | 1 or 2 | `Pickup_Ammo`, `Pickup_Weapon`, `Pickup_Health`, `G_RunFrame` (the trigger queue), `ScrCmd_UseBy`, 0x811c45c (the queue append, notifying on the spot only when 0x100 entries are queued, as 1.1's `G_Trigger`), `Player_UseEntity` |
| `damage` | 2 | `PlayerCmd_finishPlayerDamage`, `G_CheckHitTriggerDamage`, 0x811d096 |
| `death` | 0 or 1 | `player_die` 0x810175a (the attacker), `ScrCmd_Delete` |
| `movedone`, `rotatedone` | 0 | `Reached_ScriptMover` |
| `begin` | 0 | `ClientBegin` 0x80f90ae |
| `menuresponse` | 2 | 0x8100d16 (the `mr` command); `ClientDisconnect` 0x80f94ce as (`"-1"`, `"disconnect"`) |
| `binocular_enter`, `_exit`, `_fire`, `_release`, `_drop` | 0 | the client event switch in 0x80f52da, events 0xa8..0xac |
| `call_vote` | 3 | 0x8118562 |
| `vote` | 1 | `Scr_PlayerVote` 0x81185aa |

New against 1.1: the binocular five, `call_vote` and `vote` (votes went to
script, with the `setvote*` builtins, where 1.1 ran them in the engine,
`cod11-votes.md`), and `menuresponse` from `ClientDisconnect` (INFERRED new:
the 1.1 side of that call is not read here). `GScr_LoadConsts`'s table
itself changed (1.1d's `GScr_LoadConsts` allocates 128 names, CoD2's 89): 1.1's
RTCW leftovers (`team_CTF_*`, `zombiespit`, `mp_team_*`, `func_door` and
more) are gone; new are the bone and tag names (`tag_flash`, `tag_weapon`,
`j_head`, `back_low`, ...), the `snd_*prio_*` names, `freelook`,
`call_vote` and `vote` (VERIFIED, both lists).

## 5. What vcod needs, by size

Items 1 and 2, 4 and 5 are done in `crates/gsc` (`Dialect::Cod2`, sections
3.1 to 3.5); `ScriptRuntime::load_cod2` passes `Dialect::Cod2` and the
CoD2 builtin table to the `Loader` (5.1).

1. `#include` (parser directive, per-file import set, non-transitive,
   builtins first then includes for an unqualified name, local-vs-include
   duplicate a compile error): 4 MP files, every gametype through
   `_weapons.gsc`. The compiler today decides builtin-or-script without the
   builtin table (`Op::CallBuiltin` for any non-local name); with includes
   it needs the table, or a link step that resolves an unknown name against
   the includes before the host.
2. `waittillframeend` as a keyword and an op that queues the thread behind
   the frame's `wait 0`s (3.2). Today a silent no-op.
3. The 26 MP-called builtins vcod lacks (1.4), the 10- and 9-argument
   damage and killed callbacks, the new notifies (4.2), `script_struct` and
   the `codescripts/struct` calls, the shrunken entity field table with the
   rest coming from `radiant/*.txt`, and the float, bit-packed hudelem
   fields.
4. A dialect switch for the compile-time typing (3.4): CoD2 reads
   `self[k]`/`level[k]` at run time and refuses their writes as `not an
   lvalue`; optional for MP (no stock MP script depends on it).
5. Optional fidelity: definite assignment (3.5), `x = thread f()`.

### 5.1 The bootstrap in vcod-server

VERIFIED, `game::cod2_boot` (`COD2_DIR`): `ScriptRuntime::load_cod2`
takes mp_toujane's entities off `vcod_cod2::world`, compiles under
`Dialect::Cod2` with `BuiltinSet::cod2()` (section 1.3's tables) handed to
the `Loader`, and runs `dm`'s and `tdm`'s `main`, the map's `main` and
`CodeCallback_StartGameType` to their first wait with no unknown call and
no script error, then 200 empty frames with none. No name the closure
calls is left unanswered (`ScriptRuntime::unimplemented_builtins` is
empty for both).

CoD2-only handlers (`builtins::cod2`): `isSplitScreen` pushes 0
(`Scr_IsSplitscreen` 0x8117918, VERIFIED); the rumble, rank, party and
`matchEnd` builtins listed there have empty handlers (VERIFIED, each
address); `assert`, `assertEx`, `assertMsg`, `print` and `line` do
nothing, as a developer statement below `developer_script 1` (1.1, 3.3;
`GScr_line` 0x8110c02 is empty besides, VERIFIED). `precacheItem` registers
a weapon by name in first-use order, CoD2 having no fixed weapon list
(`G_FindItem` 0x80df226, `G_GetWeaponIndexForName` 0x8120dd0, VERIFIED);
its static items, item bits and model indexes are not done.

### 5.2 A CoD2 level in vcod-server

`game::cod2::Cod2Level` is the game half of a CoD2 level: it owns the
`ScriptRuntime` (`load_cod2_in`, which also takes the level's collision,
`game::cod2::level_world` over `vcod_cod2::collision::collision_world`),
one `vcod_cod2::pmove::PlayerState` per client, and takes the engine's
calls in order: `connect`, `begin`, `menu_response`, one `client_think`
per cmd (pmove over the level's collision, then the origin, aim, ground,
`pm_type`, offhand and view angles written back for the script, the
binocular notifies off the new events and the touch pass), one
`run_frame` per frame (threads, entity pass, the frame's spawns put on
the playerstates, `setOrigin` and `setPlayerAngles`, then
`ClientEndFrame`'s playerstate stores). CoD2's engine,
`cod2::Cod2Server`, drives it (cod2-protocol.md 14.1); CoD1's
`server.rs` never does. VERIFIED (`COD2_DIR`,
`cod2_dm_plays_two_clients_on_mp_toujane` and its `tdm` twin): two
clients connect, answer the team and weapon menus, spawn, and run 30 s on
mp_toujane with no script error at `developer 1`; the menus land at 1246,
the weapons the level named in configstring 7, each loadout's frag
grenade in the offhand.

The level runs on CoD2's tables where vcod has them:

- The configstring blocks are `vcod_cod2::net::CS_LAYOUT_118`'s (the
  host's `Cod1Wire` is `PROTOCOL_118.as_cod1_shim()`) and `Defs` is `defs::COD2`: 15
  means of death (4.1, VERIFIED), no fixed weapon list. A weapon's index
  is the order the level first named it (`GameHost::weapon_index`, which
  on CoD2 loads the weapon file and appends it; VERIFIED, 1.3).
  Configstring 7 carries the list as it grows; the live gamestate holds
  the list there (cod2-protocol.md 9), and when retail writes it is
  INFERRED. The weapon table (`WeaponTable::cod2`) parses each file as
  1.1's does.
- Fields: the ten entity fields of 2.1 keep 1.1's storage; a name CoD2
  dropped is a script field (`fields::route_entity_cod2`). The client table
  is 1.1's without `handicap`, with `psoffsettime` in `handicap`'s cell.
  The HUD table is 2.3's: float `x`/`y`, `z`, `horzalign` and `vertalign`
  (names VERIFIED off the pointer tables at 0x8187cc0 and 0x8187ce0: `subleft`,
  `left`, `center`, `right`, `fullscreen`, `noscale`, `alignto640`,
  `center_safearea`; `subtop`, `top`, `middle`, `bottom`, `fullscreen`,
  `noscale`, `alignto480`, `center_safearea`) and `foreground`. Radiant
  keys come from the paks' `radiant/keys.txt` (`fields::parse_radiant_keys`).
- Spawns (2.4): `trigger_radius` is a box `(-r, -r, 0)..(r, r, height)`
  with `svFlags` 0x21, `trigger_disk` `(-(r + 64), .., -100000)..(r + 64,
  .., 100000)` with 0x41, both `Touch_Multi` triggers with 1.1's contents
  rule (`SP_trigger_radius` 0x811c6e0, `SP_trigger_disk` 0x811c880,
  VERIFIED); a missing `radius` or `height` is retail's `Com_Error`, a
  failed load (VERIFIED, the strings).
  That the flags select a horizontal-distance test is INFERRED; vcod
  touches when the player's box comes within the radius of the axis.
  `trigger_use_touch` runs `SP_trigger_use`'s function (0x8110052 and
  0x811003e both call 0x810fe50, VERIFIED), so it registers as a `trigger_use`; its
  use rule is 5.3's. `script_struct`'s spawn function
  is `G_FreeEntity` (0x811ee50, VERIFIED); vcod keeps the block and, after the spawn
  pass, runs `InitStructs` and one `CreateStruct` per block, writing the
  keys onto the struct (typed by the Radiant list, INFERRED). mp_toujane
  spawns its ten `trigger_radius` and three `trigger_use_touch`.
  `func_bobbing`, `func_pendulum` and `misc_spawner` have no arm on CoD2.
- 1.1's rules everywhere else: `HostProfile::speaks_1_1` is true in
  `profile::COD2`, so every 1.5-or-UO arm stays off.

The fourteen builtins later frames call, and what else a joining client
reached, each row read off its handler (VERIFIED):

| builtin | handler | behaviour |
|---|---|---|
| `bulletTracePassed(a, b, chars, ignore)` | `Scr_BulletTracePassed` 0x8113a4a | `G_LocationalTracePassed`, mask 0x2802831 or 0x802831 without characters; vcod runs `bulletTrace`'s trace |
| `closeInGameMenu()` | `PlayerCmd_CloseInGameMenu` 0x80fbed2 | reliable `K` |
| `getAmmoCount(w)` | `GScr_GetAmmoCount` 0x8111104 | `BG_WeaponAmmo`: reserve plus clip, 0 for no weapon |
| `getCurrentOffhand()` | `PlayerCmd_getCurrentOffhand` 0x80fa030 | ps+0xd0's name; `"none"` unless playing with one |
| `getPlayerAngles()` | `PlayerCmd_getAngles` 0x80fa9ec | `ps.viewangles` |
| `int(x)` | `GScr_CastInt` 0x811400a | float truncated (`fistp` under `\| 0xc00`), string `atoi`, int as is, else `cannot cast %s to int` |
| `setNormalHealth(f)` | `ScrCmd_SetNormalHealth` 0x8111fd2 | `f` capped at 1; a player `floor(0.5 + maxhealth * f)` and `I "0"`, another entity `maxhealth * f` truncated; below 1 an error print and no change |
| `setWaypoint(b)` | `HECmd_SetWaypoint` 0x81044c0 | hudelem type 13, the flag as a float at `+0x70` |
| `setWeaponClipAmmo(w, n)` | `PlayerCmd_SetWeaponClipAmmo` 0x80fc91e | clip clamped to `0..=clipSize` |
| `switchToOffhand(w)` | `PlayerCmd_switchToOffhand` 0x80fa2ac | unknown name a parameter error; held: `C <index>` (0x8120e44) and true; else false |
| `updateScores()` | `GScr_UpdateScores` 0x81169a2 | `H <allies>` then `G <axis>` to the client |
| `vectorDot(a, b)` | `Scr_VectorDot` 0x8114240 | the dot, stored as a float |
| `setTeamScore(t, n)` | `GScr_SetTeamScore` 0x8115dda | no configstring: `H <n>` (allies, `level+0x204`) or `G <n>` (axis, `level+0x200`) to every client, ranks flag set (`allies` is 0x8853222 in `GScr_LoadConsts`) |
| `giveWeapon(w)` | `G_GivePlayerWeapon` 0x8120b26 | an `offhandClass` weapon becomes ps+0xd0 when it is 0, sent as `C <index>`; vcod adds that to 1.1's give, the two primary slots at ps+0x555/0x556 and the `Add_Ammo` of `startAmmo` (cod2-combat.md 6.3). Not done: the next offhand when one runs dry |

`print` and `line` are developer statements above. `getCurrentWeapon`
reads CoD2's list.

The callbacks' extra arguments (4.1), VERIFIED off the call sites:
`timeOffset` is `G_Damage`'s tenth argument. The bullet path passes
`level.time` minus the shot's time (0x811fe90's `DAT_0864c56c -
param_9`), and `FireWeaponAntiLag` (0x8120870) is handed the client's lag
time under `g_antilag` 1, its default, else `level.time`, so 0. The lag
time is `gclient+0x289c`, which `SV_ClientThink` (0x8090dac) moves to each
cmd's `serverTime` through 0x80f5614: at most 1000 ms behind
`level.time`, and only forward unless the cmd is at or past `level.time`
(`game::cod2::lag_time`, `time_offset`). Every other `G_Damage` caller
(melee, blasts, falls, `trigger_hurt`, `finishPlayerDamage`) passes 0.
`deathAnimDuration` is `player_die`'s `BG_AnimScriptEvent(ps,
ANIM_ET_DEATH, 0, 1)`, the death anim's length. `Cod2HostState::callback`
holds both; vcod's CoD2 shot and death path sets them
(`cod2-combat.md`).

Notifies (4.2): `menuresponse` ("-1", "disconnect") from
`ClientDisconnect` is 1.1's too (1.1d 0x42ad9), and vcod already raises
it. The binocular five come off playerstate events 168..172
(`game::cod2::binocular_notify`, raised by `Cod2Level::client_think`).
Votes: `g_oldVoting` (default 1, `Dvar_RegisterBool("g_oldVoting", 1,
..)`) keeps the engine's vote; at 0, `Cmd_CallVote_f` (0x80ffbb2) refuses
under `g_allowVote` 0 or on a `;` in its three arguments and otherwise
notifies `call_vote` with them, and `Cmd_Vote_f` (0x8100960) notifies
`vote` with `"yes"` when the first letter is `y` or the second `Y` or `1`
(Q3's test), else `"no"` (`ScriptRuntime::vote_command`).

### 5.3 The stock gametypes

The iwds carry five gametypes, `dm`, `tdm`, `sd`, `ctf` and `hq`
(`maps/mp/gametypes/*.gsc` in `iw_07` and `iw_15`). VERIFIED. Every one
loads on each of the 15 stock maps, two clients join allies and axis
through the stock menus, spawn on one of the gametype's spawn classes and
play 10 s with no script error and no builtin
unanswered (`game::cod2_gametypes`, the ignored `cod2_gametype_census`,
and a 20 s run of all five on mp_toujane and mp_breakout in the default
suite; `COD2_DIR`). VERIFIED. Before vcod read a thread's return (3.5),
a map with a `soundfx` line lost the rest of its `main` at `developer 1`,
game[] team names included; the census runs at `developer 0`, the
server's default.

`sd`'s bomb zones call four builtins on a `trigger_use` or
`trigger_use_touch` (any other entity is the handlers' `trigger entity
must be of type ...` error). VERIFIED, each handler:

| builtin | handler | behaviour |
|---|---|---|
| `setTeamForTrigger(team)` | `GScr_SetTeamForTrigger` 0x8117944 | `allies` 2, `axis` 1, `none` 0 into `ent+0x167`; another string an error |
| `player clientClaimTrigger(t)` | `GScr_ClientClaimTrigger` 0x8117a72 | `ent+0x1b4` (0x3ff unclaimed) takes the player's number when unclaimed or his |
| `player clientReleaseTrigger(t)` | `GScr_ClientReleaseTrigger` 0x8117b5a | back to 0x3ff when the claim is his |
| `releaseClaimedTrigger()` | `GScr_ReleaseClaimedTrigger` 0x8117c28 | back to 0x3ff |

The use key reads both bytes through the one use list (5.4). The round
flows these drive (`sd`'s plant and defuse, `ctf`'s capture, `hq`'s radio)
are gated in `cod2/cycle_tests.rs` (cod2-map-cycle.md 3).

### 5.4 The use key: one list for items, guns and use triggers

Retail has one use path. `Player_UpdateCursorHints` (Mac 0x1ccebe, lnxded
0x8121bc6, from `ClientEndFrame`) picks the cursor hint and its entity off
`Player_GetUseList` (Mac 0x1cc8fc, lnxded 0x8121494), and
`Player_UpdateActivate` (Mac 0x1cc628, lnxded 0x8121314, last in
`ClientThink_real`) uses that entity on the press. vcod-server runs the
three as `game::cod2_use` (`Cod2Level::use_list`, `update_cursor_hints`,
`update_activate`) for the weapon items, the guns and the use triggers
alike. Addresses below are Mac's; the constants are lnxded rodata.

`Player_GetUseList`. VERIFIED, the decompile and the rodata:

- `CM_AreaEntities` over the view origin +- (192, 192, 96)
  (0x815ee4c..0x815ee54) under contents 0x200000; the player is skipped,
  and an entity is kept when its `eType` is 3 (item) or its contents hold
  0x200000.
- A `trigger_use_touch` scores -256 (`0xc3800000`) when the player's
  absolute box overlaps its bounds and `SV_EntityContact` meets it, and is
  dropped otherwise, however near.
- Anything else: `to = (absmin + absmax) * 0.5 - eye`, `dist` its length
  (`Vec3Normalize`); past 128 (0x815ee60) it is dropped. The score is
  `((dot(to, view forward) + 1) * -0.5 + 1) * 256` (0x815ee58,
  0x815ee5c), less 256 for a `trigger_use`, plus 10000 (0x815ee64) for an
  item `BG_CanItemBeGrabbed` refuses, plus `dist`.
- `qsort` by `compare_use` (0x1cc8e2: the score difference truncated to
  an int); the refused items, sorted last by their 10000, are cut.
- Each entry left but a `trigger_use_touch` is traced to from the eye
  (`G_TraceCapsuleComplete`, zero box, the player passed, mask 0x11), at
  its bounds centre, a gun (`eType` 9) at its `tag_aim`; one the trace
  does not reach takes 10000. Sorted again, the blocked ones are cut.

No kind outranks another: an item and a gun compete by score alone, and
the use triggers win by their 256. VERIFIED (capture,
`crates/cod2/tests/fixtures/turret/probe_usehint.gsc`, the `turret`
script of `move_probe` on mp_carentan's gun 295): with an mp40 hung 30
ahead of the player at eye height the first snapshot carries `cursorHint`
9 (the mp40, weapon 5, `+ 4`) and `cursorHintString` -1, the first use
press takes the mp40, and the hint turns to 6 with string 0 (the gun);
with it 30 behind the hint is 6 and the press mounts the gun, and turning
toward the mp40 after the release hints 9 again.

`Player_UpdateCursorHints`. VERIFIED, the decompile: the hint, its string
and its entity reset to 0, -1 and 0x3ff (ps+0x598, 0x59c, 0x5a0); with
health above 0 and off weapon states 17..22:

- Off a gun (busy byte 0) and not mantling (`pm_flags` 0x4), the list is
  walked in order and the first entry that hints wins:
  - an item whose row is a weapon (`giType` 1), not a grenade
    (`weaponType` 1, def+0x78) and not held (`ps+0x544`): hint `weapon +
    4`, string -1. Any other item is passed over;
  - a gun `G_IsTurretUsable` accepts: hint `weapon + 4`, its
    `useHintString`'s index (def+0x574) when the string is not empty, else
    -1. A refused gun is passed over;
  - an `eType` 0 entity: a `trigger_use` or `trigger_use_touch` of another
    team (`ent+0x167`) or claimed by another client (`ent+0x1b4`) is passed
    over; otherwise its cursor hint (`ent+0xdc`) and string (`ent+0xd8`,
    0xff read -1) win. Any other `eType` 0 entity, or a trigger with hint
    0, ends the walk with no hint.
- A gunner (busy byte set, `eFlags & 0x300`) takes the drop hint
  (`Player_SetTurretDropHint`, cod2-turrets.md 6).

`Player_UpdateActivate`. VERIFIED, the decompile: `pm_flags` 0x8 (the
reload request `PM_Weapon` reads) is cleared; then, off weapon states
17..22 (`gclient+0x27bc` read as this cmd's buttons, `+0x27c0` the last
cmd's and `+0x27c4` the new presses: INFERRED, the Q3 layout):

1. With a use target pending (`gclient+0x2830` not 0x3ff), `+usereload`
   (0x20) down on the last cmd and up on this one sets `pm_flags` 0x8 and
   returns: a usereload let go before the hold time is a reload.
2. A press of 0x28 clears the target. A player on a gun (busy byte set)
   takes busy byte 2 when `eFlags & 0x300` (the release), else 0. Anyone
   else not mantling takes the hint's entity as the target and the level
   time as the press time (`gclient+0x2834`); with no hint entity nothing
   is activated.
3. With a target or an activation, 0x28 held, the target set,
   `g_useholdspawndelay` ms since the spawn (`gclient+0x28a0`) and
   `g_useholdtime` ms since the press: an item is notified `touch` with
   the player, its busy byte set to 1 and its touch handler run with
   `touched` 0 (the swap); a gun `G_IsTurretUsable` refuses is left; else
   `trigger` is notified with the player and the use handler runs
   (`turret_use` for a gun). The target is cleared.
4. With neither, a press of `+usereload` sets `pm_flags` 0x8.

INFERRED, off `Dvar_RegisterInt` at 0x8111853 (`g_useholdtime` 0,
`g_useholdspawndelay` 1): on a stock server the press uses its target on
its own cmd.

vcod: the item, the gun and the trigger arms above, the 0x8 reload
request, `g_useholdtime`. Not done: `g_useholdspawndelay` (no spawn time
is kept), the item's busy byte, the hint walk's stop on an `eType` 0
entity that is not a use trigger (vcod leaves such entities out of the
list), and the sight trace meets the world and brush models only.
INFERRED, that no stock map has an entity the last two would change.
vcod's `spawn("weapon_<name>")` makes no item on CoD2, so the in-process
replica of the check above (`an_item_and_a_gun_share_one_use_list`)
launches the mp40 with `item::launch`. Tests in `game::cod2_use`.

## 6. Open

- How the stock gametypes use `timeOffset` and `deathAnimDuration`
  (5.2 has what retail passes).
- The handlers of the new builtins past 5.2's table are unread.
- The contact test behind `svFlags` 0x20 and 0x40.
