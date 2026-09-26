# Probes that need a connected client

Everything in the directory above measures from a gametype's `main`, which
`tools/run_probe.sh` can drive on its own: it boots the retail server, waits,
and reads `games_mp.log`. `tools/capture_probes.sh` globs
`../probe_*.gsc`, and `semantics_ab.rs` pairs every `.gsc` beside it against a
`retail-captures.txt` section, so a probe that logs nothing without a client
would sit in that set as an empty section indistinguishable from a broken one.

These probes need a client for their own reason -- one measures from
`Callback_PlayerConnect`, one from a notify only a walking player raises, and
one has a half that is only visible on the wire -- so they are here, outside
both. Their output is quoted in whatever
research doc the measurement belongs to, or committed as a fixture a test in
`crates/server` reads; neither is paired against `retail-captures.txt`.

## Running one

Two shells. The first boots the retail server with the probe installed as a
loose gametype (the `.txt` is the description file the engine refuses to load
a map without), on a port clear of `run_server.sh`'s 28960 and
`run_probe.sh`'s 28970:

```
cp client-probes/probe_pers.gsc "$COD_LNXDED_HOME/main/maps/mp/gametypes/"
printf '"PROBE_PERS"\r\n' > "$COD_LNXDED_HOME/main/maps/mp/gametypes/probe_pers.txt"
rm -f "$COD_LNXDED_HOME/main/games_mp.log"
private/reference/cod-lnxded-1.1d/cod_lnxded \
    +set dedicated 1 +set developer 1 +set logfile 2 \
    +set g_log games_mp.log +set g_logSync 1 \
    +set fs_basepath "$COD_DIR" +set fs_homepath "$COD_LNXDED_HOME" \
    +set net_port 28971 +set sv_maxclients 8 +set sv_pure 0 \
    +set g_gametype probe_pers +map mp_pavlov
```

The second connects a client, which is what makes the callback run:

```
cargo run -p vcod -- --net-probe 127.0.0.1:28971 --probe-secs 12
```

Then read `$COD_LNXDED_HOME/main/games_mp.log` for the `PROBE ` lines and the
server's console for a `script runtime error` block, exactly as
`run_probe.sh` does. Delete the loose `probe_*` files from the homepath
afterwards: the engine keeps only the first 31 loose gametype scripts and
silently falls back to `dm` past that.

`COD_LNXDED_HOME` must be an absolute path with no `+` in it — the engine
splits its own command line on `+`.

## probe_pers

Run 2026-08-31 against the retail 1.1d dedicated server, `dm`-shaped probe
gametype on mp_pavlov, one client. `games_mp.log`:

```
  0:00 PROBE at main
  0:00 PROBE at startgametype
  0:05 PROBE at connect_before_begin
  0:05 PROBE pers_before_begin defined
  0:05 PROBE at connect_after_begin
  0:05 PROBE pers_after_begin defined
  0:05 PROBE at read_pers_key
  0:05 PROBE pers_team undefined
  0:05 PROBE at write_pers_key
  0:05 PROBE pers_team_written spectator
  0:05 PROBE at read_name
  0:05 PROBE name vcod
  0:05 PROBE at read_undef_field_index
```

and the console, on the line after the last one logged:

```
******* script runtime error *******
undefined is not an array, string, or vector: (file 'maps\mp\gametypes\probe_pers.gsc', line 48)
 if(isdefined(self.nosuchfield["team"]))
```

The line number is from the copy that ran, which had no header comment; the
committed file carries one, so a rerun names a later line.

Three measurements out of it, all used in
`docs/research/cod11-gsc-object-model.md`: `.pers` is an indexable object
before `begin` and holds nothing, `.name` already carries the client's
userinfo name, and reading an index off a genuinely undefined field is fatal.

## probe_trigger

Every `"trigger"` notify the engine's touch pass raises. It threads a
`waittill("trigger", other)` onto every trigger entity of the six trigger
classnames and logs one line per notify with the trigger's entity number, its
classname, the toucher's entity number and the toucher's origin; a `PROBE
watch` line per entity ahead of them is the census the map ended up with after
the gametype's `_gameobjects` pass deleted what it did not claim.

It calls `maps\mp\gametypes\dm::main()` itself, so a client can answer the
stock team menu and spawn. The scan waits a second first: the entity lump is
loaded before the gametype's `main()` runs but the map's own `main()` is not,
so a scan without the wait would watch a trigger the map script goes on to
delete or to move.

`tools/run_probe.sh` drives it under its subdirectory name, so both shells are

```
COD_LNXDED_HOME=<absolute, no '+'> SECS=400 \
    tools/run_probe.sh client-probes/probe_trigger mp_pavlov
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-triggers \
    --probe-team allies --probe-secs 380
```

The output is committed as `crates/server/tests/fixtures/triggers/`'s capture
and diffed by `crates/server/tests/triggers_ab.rs`, which runs this same file
as our gametype so both sides log through the same `logPrint`.

## probe_mover

The ten scriptent mover verbs. Two halves of one run, which is why it is here
rather than beside the other probes: the script logs `getorigin()` and
`.angles` once per server frame through each verb, which settles the units and
the motion law, and a `--net-probe` attached to the same server reads the
`pos`/`apos` trajectory groups, which is the only way to see whether retail
ships a mover as per-frame origins or as a trajectory. It needs no client for
its own sake; it spawns one bobbing `script_model` over each player that
connects, because mp_pavlov's two surviving map `script_model`s sit in one
corner and a lone client spawns anywhere.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=75 \
    tools/run_probe.sh client-probes/probe_mover mp_pavlov
# 18 s later, in the second shell:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-secs 45
```

Both halves are committed under `crates/server/tests/fixtures/movers/` and
written up in `docs/research/cod11-movers.md`.

One thing it learned the expensive way: a mover verb on anything but a
`script_brushmodel`, `script_model` or `script_origin` is a fatal script
runtime error, so a first version that moved the map's placed weapons for
PVS coverage died on its first frame.

## probe_lookat

The `trigger_lookat` half of S&D: every `"trigger"` notify the aim trace
raises, and every player `isLookingAt` answers true for, polled once a server
frame. It threads both onto each `trigger_lookat` the map spawned, logs a
`PROBE watch` census line per entity and a `PROBE lookats` count, and then one
`PROBE fire` per notify and one `PROBE looking` per frame a player is aimed at
one. Each carries `getTime()`, which is what pairs a line with the `serverTime`
on a client probe's own `!trace`.

It calls `maps\mp\gametypes\sd::main()` itself, so the bombzones, the bomb and
the lookat triggers are the stock gametype's. The scan waits a second first,
for the same reason `probe_trigger` does.

Three shells, the gsc probe first, then the defender, then the attacker:

```
COD_LNXDED_HOME=<absolute, no '+'> SECS=420 \
    tools/run_probe.sh client-probes/probe_lookat mp_carentan +set probe_teleport 1
# second and third shells, the defender first:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-defuse --probe-secs 400
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-plant --probe-secs 380
```

`probe_teleport 1` is why the walk arrives at all. mp_carentan's allied S&D
spawns sit at y around -1064 and bombzone_A at (-146, 2490), some 3500 units
through the town, and a first retail run spent its whole 380 s oscillating
around (300, 1200) without ever reaching the zone. Under the cvar the probe
puts each player once per level on a teamdeathmatch spawn in the courtyard,
416 units from the zone for the attacker and 541 for the defender, so the walk
is a courtyard crossing. The same cvar also moves every
defender to 20 units from the charge, toward where the planter stood, once the plant has spawned
`level.bombmodel`, because the flak88 and the cart ring the zone and a walk
that has to round them reached the charge 27 s after its 60 s fuse had blown
it, and sends the planter back to the attackers' spawn at the same moment,
since a planter left standing on the defender-to-bomb line is what the
lookat's body trace stops at and no station fires. A run without the cvar walks from the stock spawns and, on mp_carentan,
does not get there. Anything after the map name is passed
to the engine verbatim, which takes its `+set` arguments in any order. Only
mp_carentan has the two origins; on any other map the spawn thread logs `PROBE
teleport unsupported <map>` once and does nothing, and the plant-time thread
returns without a line. `crates/server/tests`'s A/B
never sets the cvar, since it stands its clients where it wants them itself.

What the three halves measure between them: how often a lookat fires while a
player is aimed at it (the server's log says every frame, with no wait gate in
`G_Trigger`), what `isLookingAt` answers on the frames around a fire, the
`pm_type` a planting player sits at while retail has it linked to the bombzone
and what its velocity does under a forward cmd, the objective slots the plant
and the defuse write and delete, and the HUD element the progress bar rides
with its `scaleStartTime` / `scaleTime` / `fromWidth` / `fromHeight` tween
fields.

The server's log goes to
`crates/server/tests/fixtures/triggers/<map>-sd-lookat.txt`; the two client
halves write `crates/server/tests/fixtures/playerstate/<map>-sd-plant-attacker.txt`
and `<map>-sd-defuse-defender.txt`. All three are retail evidence, and a run
against `vcod-server` overwrites the two client ones: move them to `tmp/` and
`git checkout` the fixture directory after.

## probe_pickup

Every `"touch"` and `"trigger"` notify an item entity or a player takes, for
the A/B in `crates/server/tests/pickup_ab.rs`. It logs one `PROBE item` line
per watched entity at first sight (its number, classname, `getTime()` and
origin), one `PROBE other` line for any other non-player entity a scan first
sees after the startup census (so a dropped weapon whose classname is not in
the watched list still shows up), one `PROBE touch` per item's own `"touch"`
notify and one `PROBE ptouch` per player's, one `PROBE trigger` per item's
`"trigger"` notify with the toucher and, if the pickup swapped out a held
weapon, that weapon's number and classname (`undefined` otherwise), and one
`PROBE teleport` per `probe_teleport` move.

The watched classnames are mp_carentan's two placed weapon kinds
(`mpweapon_fg42`, `mpweapon_panzerfaust`, `docs/research/cod11-items.md`
§4.1's stock numbers), the allied loadout a weapon swap drops
(`mpweapon_m1carbine`, `mpweapon_colt`, `mpweapon_fraggrenade`) and the
health `dm` drops on a death (`item_health`); an unlisted classname is a
`PROBE other` line instead. The census's `PROBE item` lines are what
measured the two fg42s at entities 252 and 258
(`docs/research/cod11-items.md` §12.1).

It calls `maps\mp\gametypes\dm::main()` itself, so a client can answer the
stock team menu and spawn. mp_carentan's two fg42s sit a town apart, so
under `probe_teleport 1` each live player is put on the first one on its
first spawn and, once that fg42's own `"trigger"` notify has fired (i.e. it
has been taken), on the second one two seconds later; both moves are onto
the item's own `origin`, not a nearby spot, so Task 3's `--save-pickup`
finds "on the first fg42" by the stepped snapshot's position (within 48
units xy of the fg42's origin, or a jump, ledger ruling R1) without having
to see the teleport as an origin jump. A run without the cvar never
teleports and never logs a `PROBE teleport` line, since an unset cvar reads
`""`. `crates/server/tests/pickup_ab.rs` sets it, so ours teleports the way
retail did.

```
COD_LNXDED_HOME=<absolute, no '+'> SECS=150 \
    tools/run_probe.sh client-probes/probe_pickup mp_carentan \
        +set probe_teleport 1 +set scr_allow_fg42 1
# about 15 s later, in the second shell:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --save-pickup --probe-secs 120
```

`scr_allow_fg42 1` is needed because stock `default_mp.cfg` sets it 0, and
`_teams::restrictPlacedWeapons` then deletes both fg42s at map load.

The `PROBE` lines plus the server's own `Weapon:` and `Item:` lines (read
straight out of `games_mp.log`, which `run_probe.sh` does not print) go to
`crates/server/tests/fixtures/items/mp_carentan-dm-pickup-script.txt`; the
client half writes `mp_carentan-dm-pickup.txt`. Both are retail evidence,
and a run against `vcod-server` overwrites the client one: move it to
`tmp/` and `git checkout` the fixture directory after.

## probe_turret

Mounted MG's server half. It logs every `misc_mg42` once (`PROBE turret
<num> <origin> <angles>`), and under `probe_teleport 1` puts each spawning
player at a fixed spot around the gun nearest (1712 1830 8), mp_carentan's
one MG: an allied player 40 units behind it facing along its yaw, an axis
player 300 units in front facing back down it, both inside the gun's
128-unit activate range and its ±45 degree arc. It logs each move as `PROBE
place <clientnum> <team> <origin> <yaw>`. The hits themselves never need a
script line: the engine's own `D;`/`K;` records in `games_mp.log` carry the
weapon and MOD of every one, the same evidence the hit-capture probes read.

`watch_delete`, gated on `probe_delete_after N` (retail runs leave it
unset), deletes the same gun N seconds in; only the A/B rig sets the cvar,
for the mount's release-on-delete case.

It calls `maps\mp\gametypes\dm::main()` itself, so a client can answer the
stock team menu and spawn.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=200 \
    tools/run_probe.sh client-probes/probe_turret mp_carentan +set probe_teleport 1
# second shell, about 15 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team axis --probe-secs 180
# third shell:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --save-turret --probe-secs 170
```

The server's `PROBE` lines and `games_mp.log`'s `D;`/`K;` records are the
retail evidence a later capture reads; this probe itself writes no fixture.

## probe_bump

Player-vs-player clipping's server half. Under `probe_teleport 1` it puts
each spawning axis player on a flat brush floor on mp_carentan at
(1132 -376 -151.875) and each allied player 200 units behind it along -x,
both facing +x, once per spawn, and logs each move as `PROBE place <time>
<clientnum> <team> <origin> <yaw>`. The floor is flat from 280 units behind the
spot to 360 in front and 70 either side, which a throwaway `test_ground_under` /
`test_clear_line` search in `crates/server` found; on any other map the thread
logs `PROBE teleport unsupported <map>` and does nothing.

Under `probe_overlap 1` it also waits for both players to be placed (`PROBE
both_placed <time>`), then `setorigin`s the axis player onto the allied one's
origin 12 s and 30 s later (`PROBE overlap <time> <target> <walker> <origin>`)
and logs `PROBE state <time> <clientnum> <team> <origin> <isOnGround>` for every
playing player every frame from 1 s before to 3 s after each. 1.1 gsc has no
`getvelocity` (`tools/re/dump_builtins.py` lists none), so the per-frame
origins stand in for it.

It calls `maps\mp\gametypes\dm::main()` itself, so a client can answer the
stock team menu and spawn. Three shells, the gsc probe first, then the target,
then the walker:

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=200 \
    tools/run_probe.sh client-probes/probe_bump mp_carentan +set probe_teleport 1
# second shell, about 12 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-bump-target --probe-team axis --probe-secs 185
# third shell, about 10 s after that:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --save-bump --probe-team allies --probe-secs 170
```

`--probe-bump-target` starts its schedule once it sees the walker on its mark:
standing 35 s, crouched 25 s, standing 1.5 s (a prone straight out of a
crouch is sometimes refused), prone 25 s, standing. `--save-bump` runs its
script once per stance, each started once the target's `solid` top byte reads
that stance: a head-on walk, a walk back, a glance 20 units right of the line,
a walk back, a jump from rest 10 units short of contact, a walk back. It writes
`crates/server/tests/fixtures/playerstate/<map>-dm-bump-walker.txt`.

The overlap capture is the same three shells with `+set probe_overlap 1` on
the server and `--capture-tag overlap` on the walker, which picks its overlap
script: still through the first setorigin, then walking +x through the second,
clocked off the placement's server time. The spectator-slot capture adds a
plain `--net-probe 127.0.0.1:28970 --probe-secs 185` started before the target,
so it holds slot 0 and never joins, and tags the walker `overlap-spectator`.
The gsc's `PROBE` lines of each run go to
`<map>-dm-bump-script.txt`, `-bump-overlap-script.txt` and
`-bump-overlap-spectator-script.txt` beside the walker fixtures, with the
target's own push lines (`BUMPT`, its stdout) appended as comments.

All six files are retail evidence. A walker run against `vcod-server`
overwrites the untagged one and refuses the tagged ones without
`--overwrite-fixture`: move them to `tmp/` and `git checkout` the fixture
directory after.

## probe_bomb and probe_blastbody

Who a scripted blast reaches, for `docs/research/cod11-combat.md` 14.4. Both
call `maps\mp\gametypes\sd::main()` and wait for four playing clients after
the match-start restart, so the recipe is four `--probe-team` clients, two per
team, started a couple of seconds apart:

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=110 \
    tools/run_probe.sh client-probes/probe_bomb mp_carentan
# second shell, about 6 s later:
for t in allies axis allies axis; do
    cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team $t --probe-secs 100 &
    sleep 2
done
```

`probe_blastbody` takes the same recipe with 60 s on the server. Against ours
the server half is
`vcod-server mp_carentan --gametype-script crates/gsc/tests/fixtures/semantics/client-probes/probe_bomb.gsc`,
whose log carries the same lines as `script: …`. Nothing here writes a
fixture.

`probe_bomb` stands the four on fixed stations round the committed plant
capture's charge, `radiusDamage`s at and near it four times at a flat 20, then
replays the tail of `sd.gsc`'s plant and lets the stock `bomb_countdown` run
out. Retail, 2026-09-26, the first round of the run:

```
0:16 PROBE place 0 (-196.00, 2455.00, -22.00)
0:16 PROBE place 1 (-84.00, 2511.00, -22.00)
0:16 PROBE place 2 (-112.00, 2446.00, -22.00)
0:16 PROBE place 3 (-77.00, 2473.00, -32.00)
0:16 PROBE bomb (-176.80, 2473.10, -22.96)
0:16 PROBE trigger getorigin (-176.80, 2473.10, -22.96) origin (-176.80, 2473.10, -22.96)
0:17 PROBE blast at_bomb
0:18 PROBE blast above_bomb
0:18 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:19 PROBE blast trigger_getorigin
0:20 PROBE blast above_player0
0:20 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:21 PROBE before 0 100 playing (-197.05, 2454.00, -21.88)
0:21 PROBE before 1 100 playing (-84.00, 2511.00, -21.88)
0:21 PROBE before 2 100 playing (-112.00, 2446.00, -21.88)
0:21 PROBE before 3 100 playing (-77.00, 2473.00, -31.87)
1:21 W;allies;vcod;vcod
1:21 L;axis;vcod;vcod
1:22 PROBE after 0 100 playing (-197.05, 2454.00, -21.88)
1:22 PROBE after 1 100 playing (-84.00, 2511.00, -21.88)
1:22 PROBE after 2 100 playing (-112.00, 2446.00, -21.88)
1:22 PROBE after 3 100 playing (-77.00, 2473.00, -31.87)
```

`probe_blastbody` puts two clients on one line from a blast in the open and
blasts twice at a flat 20, the second time with the front one moved off the
line. Retail, 2026-09-26:

```
0:17 PROBE place 0 (-226.00, 2424.00, -32.00)
0:17 PROBE place 1 (-269.00, 2381.00, -32.00)
0:17 PROBE place 2 (400.00, 3272.00, -23.88)
0:17 PROBE place 3 (224.00, -1280.00, 1.86)
0:18 PROBE blast shielded
0:18 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:18 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:20 PROBE blast unshielded (-300.00, 2473.00, -31.87) (-269.73, 2380.27, -31.87)
0:20 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:20 D;1;axis;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:21 PROBE done
```
