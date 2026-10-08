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

`probe_pose 1` is the pose measurement of `docs/research/cod11-combat.md`
16.2: every hit logs `PROBE hit <time> <clientnum> <sHitLoc> <iDamage>
<point>` and none lands. The target flips its own view pitch and logs the
snapshot each flip reached; the gunner holds the trigger at it for 8 s. The
connect order picks the slots: gunner first puts the target in slot 1,
target first puts it in slot 0. A tagged turret fixture lands in
`crates/server/tests/fixtures/turret/`; move it out after the run.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=120 \
    tools/run_probe.sh client-probes/probe_turret mp_carentan +set probe_teleport 1 +set probe_pose 1
# 15 s later, the two in the order that picks the slots, 3 s apart:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --save-turret --probe-turret-target-ms 8000 --capture-tag pose --probe-secs 90
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team axis --probe-pitch-flip 85 --probe-secs 95
```

Pair each `PITCH serverTime=<t>` line of the target with the `PROBE hit`
lines at `t - 50`, `t` and `t + 50`. Retail, 2026-10-08, one change each way
(hit x of the frames k-1 to k+3, `k0` the frame at `t`):

```
target slot 1:  0 -> 85  1518.34 1518.34 1515.40 1512.97 1512.08
target slot 0:  0 -> 85  1518.11 1515.98 1513.45 1512.48 1512.03
```

## probe_prone

The prone slope capture's server half. Under `probe_teleport 1` it puts each
spawning allied player on a mp_carentan grade once per spawn, and logs
`PROBE place <time> <clientnum> <origin> <yaw>`: `probe_spot street` is the
4-degree street at (900 1930), `probe_spot mound` the 19-degree terrain mound
at (-224 60). The spots were picked with a throwaway ground-normal scan over
our collision world; on any other map the thread logs `PROBE teleport
unsupported <map>`. The client holds its own heading, so the yaw the gsc sets
does not matter to the capture.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=118 \
    tools/run_probe.sh client-probes/probe_prone mp_carentan +set probe_teleport 1 +set probe_spot street
# second shell, about 10 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --save-slope --probe-prone 90 \
    --probe-cmd-ms 8 --capture-tag prone-street --probe-secs 105
```

The mound run is the same with `probe_spot mound`, `--probe-prone 270` and
`--capture-tag prone-mound`. It writes
`crates/server/tests/fixtures/playerstate/mp_carentan-dm-slope-8ms-prone-<spot>.txt`,
named `dm` because retail runs the probe as gametype `probe_prone`.

## probe_fall

The landing stun and fall damage measurement's server half. Under
`probe_teleport 1` it drops each spawned allied player onto the mp_carentan
street at (900 1930) from 100, 300, 340, 420, 340 again at `maxhealth` 200,
and 520 units above it, 8 s apart, with `self.health` reset to the max
before each, and logs `PROBE drop <time> <height> <origin> maxhealth <n>`
and, 4 s on, `PROBE after <time> <height> health <health> <origin>`. It
wraps `level.callbackPlayerDamage` and `level.callbackPlayerKilled` to log
`PROBE damage`, `PROBE damaged` and `PROBE killed` lines with every argument
the engine handed them. The client half prints a `FALL` line per snapshot
whose ground entity, `pm_flags`, `pm_time`, event ring or health moved, and
every airborne snapshot, and a `CMDS` line per 60 cmds with the `serverTime`
of each. Neither half writes a file; each run's lines were pasted by hand
into a fixture in `crates/server/tests/fixtures/playerstate/`, the server's
as comments: `mp_carentan-dm-fall.txt` (the first run, five drops, no
callback or `CMDS` lines), `mp_carentan-dm-fall-damage.txt` and
`mp_carentan-dm-fall-damage-cvars.txt` (2026-10-06, the second with `+set
bg_fallDamageMinHeight 200 +set bg_fallDamageMaxHeight 1000` after the
`probe_teleport` set). `fall_ab` replays the `CMDS` timeline, so a new
capture of either needs those lines. `mp_carentan-dm-fall-walk.txt`
(2026-10-06) is the stock-bounds run with `--probe-fall-walk 315` on the
client, which holds forward at that world yaw, so each stun walks the player
into the street's south wall; its `CMDS` lines carry each cmd's yaw word. `docs/research/cod11-player-clip.md` 8.9 and 8.10 read
them, and `crates/server/tests/fall_ab.rs` gates the last two.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=90 \
    tools/run_probe.sh client-probes/probe_fall mp_carentan +set probe_teleport 1
# second shell, about 7 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team allies --probe-fall --probe-secs 78
```

## probe_ride

The mover push and ride capture's server half. It renames mp_carentan's two
bombzone brush models' `script_gameobjectname` to `dm` so `_gameobjects`
keeps them, and under `probe_teleport 1` stands each spawned allied player on
model `*5`'s plank slab and moves it one verb per phase (up, down, across, a
2-degree yaw, a push, the slab lowered onto the player's head), then links
the player to a `script_origin` and moves and turns that. Every server frame
of a phase logs `PROBE f <phase> <time> <player origin> <mover origin> <mover
angles>`. The client half prints a `RIDE` line per snapshot. Neither half
writes a file; the 2026-10-05 run is in
`crates/server/tests/fixtures/movers/mp_carentan-dm-ride.txt` (the server's
`PROBE` lines) and `-ride-wire.txt` (the client's `RIDE` and trajectory lines
in the phase window), which `crates/server/tests/ride_ab.rs` replays and
`docs/research/cod11-movers.md` 11 to 13 reads.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=115 \
    tools/run_probe.sh client-probes/probe_ride mp_carentan +set probe_teleport 1
# second shell, about 6 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team allies --probe-ride --probe-secs 104
```

## probe_cull

When a moving brush model leaves and re-enters the snapshot. It keeps
mp_carentan's bombzone brush models as probe_ride does, stands each spawned
allied player on the attackers' spawn beside model `*5`, lifts the slab 20000
units over 8 s and lowers it back over 8 s, logging `PROBE f <phase> <time>
<mover origin>` every frame. The client half's `RIDE_GONE` and `RIDE_ENT`
lines say which snapshot dropped it and which brought it back. The
2026-10-06 run is `crates/server/tests/fixtures/movers/mp_carentan-dm-cull.txt`;
`docs/research/cod11-movers.md` 14 reads it.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=75 \
    tools/run_probe.sh client-probes/probe_cull mp_carentan
# second shell, about 12 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team allies --probe-ride --probe-secs 55
```

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

`probe_blastbody` takes the same recipe with 120 s on the server and
`--probe-secs 110` on the clients. Against ours
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
blasts at a flat 20: once, once with the front one moved off the line, once
per yaw 0 to 315 in steps of 45 with the front one back on its station and
turned by `setPlayerAngles`, and once over its corpse. Each yaw row logs the front client's `angles` as they read back: the call did not
land on every row, and the rows are read by the logged yaw. Retail,
2026-09-27, the first run from the yaw rows on (the rows above them read as
the second run's):

```
0:23 PROBE blast yaw 0 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-271.00, 2379.00, -31.99)
0:23 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:23 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:25 PROBE blast yaw 45 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.99) (-271.77, 2378.23, -31.99)
0:25 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:25 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:28 PROBE blast yaw 90 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-272.53, 2377.47, -31.99)
0:28 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:28 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:30 PROBE blast yaw 135 (0.00, 135.00, 0.00) (-226.00, 2424.00, -31.00) (-273.30, 2376.70, -31.99)
0:30 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:33 PROBE blast yaw 180 (0.00, 180.00, 0.00) (-226.00, 2424.00, -31.00) (-273.30, 2376.70, -31.99)
0:33 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:33 D;1;axis;vcod;-1;world;;none;6;MOD_EXPLOSIVE;none
0:35 PROBE blast yaw 225 (0.00, 225.00, 0.00) (-226.00, 2424.00, -31.00) (-273.30, 2376.70, -31.99)
0:35 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:38 PROBE blast yaw 270 (0.00, 270.00, 0.00) (-226.00, 2424.00, -31.00) (-273.30, 2376.70, -31.99)
0:38 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:40 PROBE blast yaw 315 (0.00, 315.00, 0.00) (-226.00, 2424.00, -31.00) (-273.30, 2376.70, -31.99)
0:40 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:42 K;0;allies;vcod;0;allies;vcod;none;100000;MOD_SUICIDE;none
0:44 PROBE blast corpse dead (-273.30, 2376.70, -31.99)
0:44 D;1;axis;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:45 PROBE done
```

The second run, whole:

```
0:18 PROBE place 0 (-226.00, 2424.00, -32.00) (0.00, 0.00, 0.00)
0:18 PROBE place 1 (-269.00, 2381.00, -32.00) (0.00, 0.00, 0.00)
0:18 PROBE place 2 (400.00, 3272.00, -23.88) (0.00, 0.00, 0.00)
0:18 PROBE place 3 (224.00, -1280.00, 1.86) (0.00, 0.00, 0.00)
0:19 PROBE blast shielded
0:19 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:19 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:21 PROBE blast unshielded (-300.00, 2473.00, -31.87) (-269.77, 2380.23, -31.87)
0:21 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:21 D;1;axis;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:24 PROBE blast yaw 0 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-270.94, 2379.06, -31.87)
0:24 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:24 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:26 PROBE blast yaw 45 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-271.71, 2378.29, -31.87)
0:26 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:26 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:29 PROBE blast yaw 90 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-272.71, 2377.30, -31.87)
0:29 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:29 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:31 PROBE blast yaw 135 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-272.71, 2377.30, -31.87)
0:31 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:31 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:34 PROBE blast yaw 180 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-273.80, 2376.20, -31.87)
0:34 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:34 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:36 PROBE blast yaw 225 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.78) (-274.66, 2375.34, -31.87)
0:36 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:36 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:39 PROBE blast yaw 270 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-275.36, 2374.64, -31.87)
0:39 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:39 D;1;axis;vcod;-1;world;;none;6;MOD_EXPLOSIVE;none
0:41 PROBE blast yaw 315 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-275.80, 2374.21, -31.87)
0:41 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:41 D;1;axis;vcod;-1;world;;none;6;MOD_EXPLOSIVE;none
0:43 K;0;allies;vcod;0;allies;vcod;none;100000;MOD_SUICIDE;none
0:45 PROBE blast corpse dead (-276.51, 2373.49, -31.87)
0:45 D;1;axis;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:46 PROBE done
```

What the rows measure is `docs/research/cod11-combat.md` 14.4.

## probe_passthru

Whether a rifle round goes on through the player it hits, for
`docs/research/cod11-combat.md` 2.4. Under `probe_teleport 1` on mp_carentan
it stands the lower-numbered axis player on `probe_bump`'s spot, the other
axis player 100 units behind it along +x and the allied player 200 units in
front, all facing +x, once per spawn. It wraps `dm.gsc`'s damage callback to
log `PROBE damage <time> <victim> <iDamage> <iDFlags> <mod> <hitloc> <vPoint>
<origin>` first, so two hits in one frame share a time. Four shells:

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=200 \
    tools/run_probe.sh client-probes/probe_passthru mp_carentan +set probe_teleport 1
# second and third shell, about 8 s and 14 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-target --probe-team axis --probe-secs 180
# fourth shell, about 18 s after that, once both targets have killed
# themselves once and respawned:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --save-hit --probe-sweep --probe-team allies --probe-secs 150
```

Against ours the server half is `vcod-server mp_carentan --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_passthru.gsc --set
probe_teleport=1`. The two targets write
`crates/server/tests/fixtures/playerstate/mp_carentan-probe_passthru-hit-target.txt`,
which is nobody's evidence: delete it after. Retail, 2026-09-27, the first
round:

```
0:24 PROBE place 24600 0 axis (1132.00, -376.00, -151.88)
0:28 PROBE place 28100 1 axis (1232.00, -376.00, -151.88)
0:30 PROBE place 30650 2 allies (932.00, -376.00, -151.88)
0:33 PROBE damage 33600 0 40 32 MOD_RIFLE_BULLET torso_upper (1122.40, -376.23, -90.13) (1132.00, -375.95, -151.87)
0:33 PROBE damage 33600 1 33 32 MOD_RIFLE_BULLET head (1229.84, -377.40, -85.77) (1232.00, -376.00, -151.88)
0:33 PROBE damage 33950 0 67 32 MOD_RIFLE_BULLET head (1134.93, -375.65, -82.77) (1135.53, -375.95, -151.88)
0:34 PROBE damage 34350 0 67 32 MOD_RIFLE_BULLET head (1143.72, -374.13, -86.55) (1143.27, -375.55, -151.88)
0:34 PROBE damage 34700 1 67 32 MOD_RIFLE_BULLET head (1231.67, -376.03, -84.59) (1234.30, -376.00, -151.88)
```

The sweep spends its taps within about 20 s, so the back player is in line
behind a live front one for only a few of them: this run caught one pass,
at 33600.

## probe_glass

Whether a round goes on through a window pane, for
`docs/research/cod11-combat.md` 2.4, step 3. Under `probe_teleport 1` on
mp_depot it stands every axis player at (-848 -3656), 76 units behind the
glass brush at x -772..-764, and every allied player at (-748 -3656), 16
units in front of it, facing each other, once per spawn; the sweep's
120-unit range is met from the spot, so the shooter never has to walk round
the pane. The damage log is `probe_passthru`'s. Three shells:

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=200 \
    tools/run_probe.sh client-probes/probe_glass mp_depot +set probe_teleport 1
# second shell, about 8 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-target --probe-team axis --probe-secs 180
# third shell, about 14 s after that:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --save-hit --probe-sweep --probe-team allies --probe-secs 150
```

Against ours the server half is `vcod-server mp_depot --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_glass.gsc --set
probe_teleport=1`. The target writes
`crates/server/tests/fixtures/playerstate/mp_depot-probe_glass-hit-target.txt`,
which is nobody's evidence: delete it after. The shooter's `bullethit` lines
carry the pane's impact and the flesh one behind it. Retail, 2026-09-27, the
first hits:

```
1:32 PROBE damage 92600 0 180 0 MOD_PISTOL_BULLET head (-840.35, -3655.53, 27.39) (-848.00, -3656.00, -39.88)
1:48 PROBE damage 108850 0 180 0 MOD_PISTOL_BULLET head (-840.68, -3655.10, 24.69) (-848.00, -3656.00, -39.88)
2:18 PROBE damage 138350 0 107 0 MOD_PISTOL_BULLET torso_upper (-839.14, -3656.68, 18.99) (-848.00, -3656.00, -38.99)
```

## probe_blastloop

What one `radiusDamage` does between its victims, for
`docs/research/cod11-combat.md` 14.5. It wraps `level.callbackPlayerDamage`
so every victim logs `PROBE cb <slot> <iDamage> <sessionstate> <health>` as
the engine calls it and `PROBE cbafter <slot> <sessionstate> <health>` once
sd's own callback returns. `probe_blastbody`'s recipe, 75 s on the server
and `--probe-secs 65` on the clients. Against ours the server half is
`vcod-server mp_carentan --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_blastloop.gsc`.
Nothing here writes a fixture.

Rows: three flat-20 blasts across `setPlayerIgnoreRadiusDamage(true)`, a
second blast with the flag still set, and `(false)`; then a lethal flat 200
down `probe_blastbody`'s line with 100-health slot 0 in front and
1000-health slot 1 behind, a flat 20 in the same frame and one a frame
later; then the same three on a second line with slot 3 in front and slot 2
behind. Retail, 2026-10-05, from the first blast on (sd's `D;` and `K;`
records dropped):

```
PROBE blast ignore_on
PROBE blast ignore_still
PROBE blast ignore_off
PROBE cb 0 20 playing 100
PROBE cbafter 0 playing 80
PROBE cb 1 13 playing 100
PROBE cbafter 1 playing 87
PROBE before_low 0 playing 100 (-226.00, 2424.00, -31.87)
PROBE before_low 1 playing 1000 (-269.00, 2381.00, -31.87)
PROBE blast lethal_low
PROBE cb 0 200 playing 100
PROBE cbafter 0 dead 0
PROBE cb 1 200 playing 1000
PROBE cbafter 1 playing 800
PROBE blast same_frame_low
PROBE cb 0 20 dead 0
PROBE cbafter 0 dead -20
PROBE cb 1 20 playing 800
PROBE cbafter 1 playing 780
PROBE blast next_frame_low
PROBE cb 1 20 playing 780
PROBE cbafter 1 playing 760
PROBE before_high 0 spectator -20 (-245.37, 2404.63, 28.01)
PROBE before_high 1 playing 760 (224.43, -1280.81, 1.97)
PROBE before_high 2 playing 1000 (-306.80, 2473.10, -31.87)
PROBE before_high 3 playing 100 (-246.80, 2473.10, -31.00)
PROBE blast lethal_high
PROBE cb 3 200 playing 100
PROBE cbafter 3 dead 0
PROBE blast same_frame_high
PROBE cb 3 20 dead 0
PROBE cbafter 3 dead -20
PROBE cb 2 20 playing 1000
PROBE cbafter 2 playing 980
PROBE blast next_frame_high
PROBE cb 2 20 playing 980
PROBE cbafter 2 playing 960
```

Ours, the same day, after the walk was made retail's: every row reads the
same down to `next_frame_low`. On the second line slot 3 stands at z -21.88
where retail's stands at -31.00, its body leaves two of slot 2's five
probes clear where retail's left none, and slot 2 takes 133 on
`lethal_high`; the
`same_frame_high` callbacks run 2 then 3 where retail's ran 3 then 2.

Ours on 2026-10-06, after the walk took the area tree's order (combat doc
14.7): `same_frame_high` runs 3 then 2 as retail's did. Then, with
`PM_CorrectAllSolid` and the step gate for a start in solid ported (mantle
doc, "A start inside a solid"), slot 3 stands at -31.00 inside the flak88
clip as retail's does, and every row from the first blast on reads as
retail's.

The `after_low` and `before_high` rows this section's block drops, retail
then ours on 2026-10-06 after the `PM_DeadMove` and spawn-health fixes
(combat doc 5.6 and 5.7): dead slot 0 came to rest at (-245.37, 2404.63)
and (-244.78, 2405.35), and read `spectator -20` after sd's
`spawnSpectator` on both. Before the fixes ours slid it to (-273.20,
2376.80) and its spectator read 100.

## probe_blastorder

The order one `radiusDamage` walks its victims in, for
`docs/research/cod11-combat.md` 14.7. Four players at 1000 health are set
down round mp_carentan's second area-tree split (y 2464), then moved one at
a time, a flat-20 blast from `(-176.8, 2473.1, 7)` after each; the wrapped
damage callback logs `PROBE cb <entity> <iDamage>` and puts the health back.
`probe_blastbody`'s recipe, 100 s on the server and `--probe-secs 85` on the
clients. Against ours the server half is `vcod-server mp_carentan
--gametype-script crates/gsc/tests/fixtures/semantics/client-probes/probe_blastorder.gsc`.
Nothing here writes a fixture.

Retail, 2026-10-06 (the per-blast `state` lines cut to the moved player):

```
PROBE placed 0 playing (-290.00, 2430.00, -31.94)
PROBE placed 1 playing (-260.00, 2480.00, -31.83)
PROBE placed 2 playing (-230.00, 2380.00, -31.87)
PROBE placed 3 playing (-230.00, 2540.00, -31.97)
PROBE blast placed
PROBE cb 1 20
PROBE cb 0 20
PROBE cb 3 20
PROBE cb 2 20
PROBE moved_within 0 playing (-290.00, 2440.00, -31.87)
PROBE blast moved_within
PROBE cb 0 20
PROBE cb 1 20
PROBE cb 3 20
PROBE cb 2 20
PROBE moved_into 2 playing (-200.00, 2430.00, -31.77)
PROBE blast moved_into
PROBE cb 2 20
PROBE cb 0 20
PROBE cb 1 20
PROBE cb 3 20
PROBE moved_out 1 playing (-290.00, 2380.00, -31.87)
PROBE blast moved_out
PROBE cb 2 20
PROBE cb 0 20
PROBE cb 3 20
PROBE cb 1 20
```

Ours, the same day, after the walk took the area tree's order: every `cb`
line the same and in the same order. The rest heights differ by up to 0.12
(-31.87 on all four, -31.75 for slot 1's last move).

## probe_itemdrop

The item flight, landing and respawn capture's server half, on mp_carentan
with one allied player. With the full loadout it first spawns an
`item_health` at the feet of a player at 40 health with `random` 0.5, then a
colt with spawnflags 8 after emptying the pistol's reserve, each taken by
walking onto it, and moves the player off each so the return stays on the
ground. Then four `dropItem` drops (the held carbine onto flat ground, the
colt into a wall 60 units ahead, the frag down a slope, and `item_health`)
and three script spawns (a carbine 72 units in the air, a `dropHealth`-style
pack at the feet, a colt with spawnflags 1). It logs `PROBE spawned`, `PROBE
drop` (with the player's and the item's origin and angles), `PROBE trigger`,
one `PROBE at` per frame a tracked item's `origin` or `angles` changed and a
`PROBE rest` once they have held for 3 s. `wait` is a gsc keyword, so the
pack's respawn goes through `random`. The client half, `--probe-items`,
prints an `ITEM` line per snapshot an item entity appeared or changed in and
an `ITEM_GONE` when it left. Neither half writes a file; the two runs' lines
were concatenated into
`crates/server/tests/fixtures/items/mp_carentan-dm-itemdrop.txt`, read in
`docs/research/cod11-items.md` 14 and gated by
`crates/server/tests/itemdrop_ab.rs`.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=115 \
    tools/run_probe.sh client-probes/probe_itemdrop mp_carentan
# second shell, about 15 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team allies --probe-items --probe-secs 90
```

## probe_nodrop

`CONTENTS_NODROP` (0x80000000) on four spawned carbines. No stock material
carries the bit, so it runs on `mp_itemtest`: mp_carentan's BSP with 0x80000000
ORed into material 0's contents (`textures/common/clipmonster`, the
`u32` at lump 0's offset + 68), saved as `maps/mp/mp_itemtest.bsp` beside a
copy of `maps/MP/mp_carentan.gsc` renamed to `maps/mp/mp_itemtest.gsc`, in a
`zzz_` pak in the homepath's `main/`. The pak is built from game data and is
never committed. It needs no client, so `tools/run_probe.sh` alone prints
everything; run it on stock mp_carentan too for the control.
`docs/research/cod11-items.md` 14.5 reads both.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=30 \
    tools/run_probe.sh client-probes/probe_nodrop mp_itemtest
```

Ours takes the same pak through a scratch `--game-dir` whose `main/` links
the stock paks beside it: `vcod-server mp_itemtest --game-dir <dir>
--gametype-script .../probe_nodrop.gsc`.

## probe_victims

Four leftovers of the damage path, for `docs/research/cod11-combat.md` 4.4,
5.6, 5.7 and 14.6. `probe_blastbody`'s recipe, 75 s on the server and
`--probe-secs 65` on the clients; against ours the server half is
`vcod-server mp_carentan --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_victims.gsc`.
Nothing here writes a fixture.

Rows: three lethal `finishPlayerDamage` calls whose killed callback logs
the inflictor and attacker it was handed; a `spawnSpectator` and a bare
`self spawn` on a client whose health the probe set first; a flat 200 down
`probe_blastbody`'s line, killing slot 0 in front and not 1000-health slot 1
behind, with both origins logged every frame for 1.5 s; three flat-60 blasts
beside carentan's `misc_mg42` at (1712, 1830, 8) with slot 1 in range too,
the turret's `"damage"` and `"death"` waiters logging. Retail, 2026-10-06
(sd's `D;` and `K;` records and slide frames 10 to 29, all equal to 9,
dropped):

```
PROBE org 179
PROBE fpd ent_player
PROBE killed 0 inflictor 1 attacker 1 MOD_GRENADE_SPLASH
PROBE fpd player_ent
PROBE killed 3 inflictor 179 attacker 179 MOD_RIFLE_BULLET
PROBE fpd undefined_player
PROBE killed 0 inflictor 1022 attacker 1 MOD_RIFLE_BULLET
PROBE spec health 37 spectator
PROBE spec_later health 37
PROBE play health 41 maxhealth 100
PROBE play_later health 41
PROBE slide_before 0 (-226.00, 2424.00, -31.79) 1 (-269.00, 2381.00, -31.99)
PROBE cb 0 200 playing 100
PROBE killed 0 inflictor 1022 attacker 1022 MOD_EXPLOSIVE
PROBE cb 1 200 playing 1000
PROBE slide 0 0 dead (-226.00, 2424.00, -31.79) 1 (-269.00, 2381.00, -31.99)
PROBE slide 1 0 dead (-231.11, 2418.92, -31.87) 1 (-274.65, 2375.35, -31.99)
PROBE slide 2 0 dead (-238.91, 2411.17, -31.87) 1 (-286.18, 2363.82, -31.99)
PROBE slide 3 0 dead (-242.41, 2407.71, -31.99) 1 (-293.87, 2356.14, -31.99)
PROBE slide 4 0 dead (-243.68, 2406.47, -31.99) 1 (-299.84, 2350.16, -31.99)
PROBE slide 5 0 dead (-243.68, 2406.47, -31.99) 1 (-304.31, 2345.70, -31.99)
PROBE slide 6 0 dead (-243.68, 2406.47, -31.99) 1 (-307.60, 2342.40, -31.99)
PROBE slide 7 0 dead (-243.68, 2406.47, -31.99) 1 (-309.86, 2340.14, -31.99)
PROBE slide 8 0 dead (-243.68, 2406.47, -31.99) 1 (-310.80, 2339.21, -31.99)
PROBE slide 9 0 dead (-243.68, 2406.47, -31.99) 1 (-311.49, 2338.51, -31.99)
PROBE turret 297 (-500.00, 1896.00, 175.00) health 100
PROBE turret 298 (1712.00, 1830.00, 8.00) health 100
PROBE tplace 1 (1712.00, 1990.00, -23.87)
PROBE tblast 0
PROBE cb 1 60 playing 1000
PROBE tblast_after 0 health 40
PROBE tdamage 60 1022 health 40
PROBE tblast 1
PROBE cb 1 60 playing 940
PROBE tblast_after 1 health -20
PROBE tdeath 1022 health -20
PROBE tdamage 60 1022 health -20
PROBE tblast 2
PROBE cb 1 60 playing 880
PROBE tblast_after 2 health -80
PROBE tdeath 1022 health -80
PROBE tdamage 60 1022 health -80
PROBE done
```

Ours, the same day, after the fixes: every row the same but the slide. Ours
stands both clients at z -31.87, and the frames move in bursts, since a
debug build's tick overruns and catches up; slot 0 came to rest at
(-241.43, 2408.58), 21.8 units out where retail's slid 24.9, and slot 1 at
(-307.67, 2342.33). Before the `PM_DeadMove` fix slot 0 slid 56.4.

## probe_hud_disconnect

Whether a client's `newClientHudElem` record outlives it, for
`docs/research/cod11-hud-protocol.md`, "A client's elements die with it".
One client is enough: it connects, `begin` makes one owned and one shared
element, and leaving runs the disconnect callback. Two shells:

```
COD_LNXDED_HOME=<absolute, no '+'> tools/run_probe.sh client-probes/probe_hud_disconnect mp_pavlov
# second shell, once the map is up:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-secs 8
```

Against ours: `vcod-server mp_pavlov --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_hud_disconnect.gsc`.
Plain `--net-probe` writes no fixture. Retail and ours, 2026-10-06, print
the same three lines; the doc section quotes them.

## probe_say

What `sayAll`, `sayTeam` and `pingPlayer` send and who `G_Say` lets hear
it, for `docs/research/cod11-chat.md` 2.1. Two plain probes: the first to
begin is slot 0, the allied speaker, the second the axis listener. Each
`--probe-say` line prints the command it sent as `SAY`; every `h`/`i` line
that arrives prints as `CHAT`, control bytes escaped.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=60 tools/run_probe.sh client-probes/probe_say mp_carentan
# second shell, once the map is up:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-secs 45 --probe-say "0.5:say first hello"
# third shell, two seconds later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-secs 43 --probe-say "0.5:say second hello"
```

Against ours: `vcod-server mp_carentan --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_say.gsc`; a debug
build's `wait 2` runs long, so give its probes 90 s. Retail and ours,
2026-10-06, print the same `CHAT` lines in both probes; the doc's table
quotes them. The client-command half of that table needs no probe script:
`tools/run_server.sh mp_carentan +set g_gametype tdm` and three probes,
`--probe-team axis`, `--probe-team spectator` and `--probe-team allies`
with the `--probe-say` list (`say`, `say_team`, `kill`, `say`, `tell 0 ...`).


## probe_touchorder

The order one touch pass meets what it touches, for
`docs/research/cod11-combat.md` 14.7, "The touch pass". One client under dm
on mp_pavlov, set down where the minefield triggers 101 and 102 overlap,
with three `item_health` spawned on the spot; damage is swallowed, so the
mines kill nobody and no health is taken. Each phase logs every `"trigger"`
(`f`), item `"touch"` (`t`), mine `"touch"` (`mt`) and player `"touch"`
(`pt`) for a fifth of a second. `+set probe_mode ammo` runs the other half:
three mosin items on the spot and a reserve one round short, so only the
first item the walk meets is taken. Two shells:

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=60 \
    tools/run_probe.sh client-probes/probe_touchorder mp_pavlov [+set probe_mode ammo]
# second shell, once the map is up:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team allies --probe-secs 45
```

Against ours the server half is `vcod-server mp_pavlov --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_touchorder.gsc
[--set probe_mode=ammo]`; `RUST_LOG=info,touch_pass=trace` prints each
pass's walk. Nothing here writes a fixture.

Retail, 2026-10-06, the first frame of each phase (each later cmd of a frame
adds one more `f 101`, `f 102` pair and nothing else):

```
PROBE phase spawned
PROBE f 101 11250
PROBE f 102 11250
PROBE t c 11250
PROBE t a 11250
PROBE t b 11250
PROBE mt 101 11250
PROBE pt 102 11250
PROBE mt 102 11250
PROBE phase kept
PROBE f 101 12550
PROBE f 102 12550
PROBE t c 12550
PROBE t a 12550
PROBE t b 12550
PROBE mt 101 12550
PROBE pt 102 12550
PROBE mt 102 12550
PROBE phase moved
PROBE f 101 13950
PROBE f 102 13950
PROBE t c 13950
PROBE t b 13950
PROBE t a 13950
PROBE mt 101 13950
PROBE pt 102 13950
PROBE mt 102 13950
```

and with `probe_mode ammo`:

```
PROBE weapon mosin_nagant_mp
PROBE phase ammo
PROBE take e 9650
PROBE done
```

An earlier run without the `mt` and `pt` watchers printed the same `f` and
`t` lines. Ours, the same day: the
`touch_pass` trace walks 102, 101, b, a, c after the spawn and after `kept`,
102, 101, a, b, c after `moved`, and the ammo run takes e. Its log lines
come out in another order (`mt 101`, `mt 102`, `pt 102`, `t a`, `t b`,
`t c`, `f 101`, `f 102`, once a frame), since vcod delivers the notifies at
the next script frame and runs the waiters in thread age.

## probe_squash

Which queued server commands `SV_AddServerCommand` replaces or drops, for
`docs/protocol-1.1.md`, "The server command queue". One plain probe; its
`serverCommand:` lines are the measurement.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=40 tools/run_probe.sh client-probes/probe_squash mp_carentan
# second shell, once the map is up:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-secs 25
```

Retail, 2026-10-07: `v sq_early "1"` arrives and the early print does not;
the one-frame burst arrives as `v sq_a "2"`, `v sq_b "1"`, `v sq_c "1"`,
`f "sq dup"` twice, `u`, `f "sq between"`, `v sq_d "2"`; `v sq_a "3"` and
`v sq_a "4"` both arrive. `crates/server/src/client.rs`'s
`one_frame_squashes_as_the_retail_probe_did` replays the burst.

## probe_compass

What `G_GetNonPVSFriendlyInfo` packs into `iCompassFriendInfo`, for
`docs/research/cod11-hud-protocol.md` section 9, "Compass friendlies". Both
clients join allies; slot 0 stands at a spawn, slot 1 is set down at spawns 2
to 13 of mp_harbor in turn, pings, changes team and back, suicides, takes the
dead session and ends a spectator. Two probes with `--probe-compass`, whose
`COMPASS` lines print the field decoded beside the eye and the players the
snapshot carries:

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=75 tools/run_probe.sh client-probes/probe_compass mp_harbor
# second and third shells, once the map is up, two seconds apart:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-compass --probe-secs 62
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-compass --probe-secs 60
```

The `PROBE step` lines carry slot 1's origin; slot 0's `COMPASS` lines carry
what it was sent. Against ours: `vcod-server mp_harbor --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_compass.gsc`.

## probe_re and probe_bel

Stock `re.gsc` and `bel.gsc` on mp_brecourt with the probe driving: the
Retrieval pickup, hold-use drop, timeout return, drop on death, refused
defender pickup and capture, and the Behind Enemy Lines swap on three
scripted kills. The clients are two `--save-scripted <role>` probes, which
press use when the probe's `setClientCvar("probe_use", ...)` says and record
into `crates/server/tests/fixtures/gametypes/`; the server's `games_mp.log`
goes there too, as `mp_brecourt-<gametype>-log.txt`.
`crates/server/tests/gametypes_ab.rs` runs both probes on ours and compares.
Recipe, timeline and findings: `docs/research/cod11-gametypes-re-bel.md`,
sections 7 (re: allies `attacker` first, axis `defender` 3 s later, 150 s)
and 8 (bel: both `--probe-team axis`, `first` then `second`, 130 s).

## probe_linkto

`linkTo` on script entities. It keeps mp_carentan's bombzone_A brush model
`*5` as probe_ride does and, once an allied player has spawned, links a
script_origin and a script_model to it and moves and turns it, verbs and
unlinks a linked child, runs a link chain in each entity order, deletes a
moving parent, links four script_origins to the player (no tag, a bone, a
tag with zero offsets, the four-argument form with no tag), and ends on a
link cycle whose fatal is the last measurement. Every server frame of a
phase logs `PROBE f`/`g`/`h` lines (the header of the fixture spells them
out). The plain `--net-probe` client's trajectory lines are the wire half.
The 2026-10-08 run is
`crates/server/tests/fixtures/movers/mp_carentan-dm-linkto.txt` and
`-linkto-wire.txt`, which `crates/server/tests/linkto_ab.rs` replays and
`docs/research/cod11-movers.md` 15 reads.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=95 \
    tools/run_probe.sh client-probes/probe_linkto mp_carentan
# second shell, about 7 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team allies --probe-secs 85
```

## probe_blastmove

What a blast victim's damage callback does to the rest of its walk, for
`docs/research/cod11-combat.md` 14.5. dm on mp_carentan with
`+set scr_forcerespawn 1`; three `--probe-team` clients 2 s apart, then a
fourth `--save-grenade --capture-tag <tag>` client 35 s later (move its
tagged fixture out of `crates/server/tests/fixtures/playerstate/` after the
run). 120 s on the server, `--probe-secs 100` on the three and 60 on the
thrower. Against ours the server half is `vcod-server mp_carentan
--gametype-script crates/gsc/tests/fixtures/semantics/client-probes/probe_blastmove.gsc
--set scr_forcerespawn=1`.

Each row logs `PROBE cb <row> <entity> <iDamage> <sessionstate> <origin>`
per victim and `PROBE cbdone <row> <entity>` once the first victim's action
is over. Retail, 2026-10-08, the walk per row (state lines dropped):

```
plain         cb 1, cb 0, cb 2
move_out      cb 1 (parks 0 and 2)
move_in       cb 1 (pulls 2 in from outside the box), cb 0
ignore_mid    cb 1 (setPlayerIgnoreRadiusDamage(true)), cb 0, cb 2
ignore_after  (none)
nested        cb 1, cb nested_inner 0 5, cbdone 1, cb 0, cb 2
kill          cb 1, cb kill_inner 0 50, cb kill_inner 2 50, cbdone 1,
              cb 0 dead, cb 2 dead
```

The grenade half logs `PROBE gcb first <time> <entity> <iDamage> <origin>`
for the first victim of each grenade walk, which parks the other two, and
`PROBE gcb later ...` for any victim after it. Retail logged three `first`
lines and no `later` one.

## probe_linkto2

`linkTo`'s second round on mp_carentan. Once an allied player has spawned it
moves the bombzone_A `trigger_multiple` onto the player by an origin write,
then calls `enableLinkTo` on it and links it to a script_origin that carries it
there and back, logging every `trigger` notify. It links the `misc_mg42` 297
to a script_origin and lifts and turns it, links two script_origins to a
script_model (one on `bip01 head`, one on the model's own frame) and changes
the model four times, links an item the frame it spawns in mid-air and one
that has landed, lifts both and unlinks them, and ends on `enableLinkTo` on a
script_origin, whose fatal is the last measurement. The 2026-10-08 run is
`crates/server/tests/fixtures/movers/mp_carentan-dm-linkto2.txt` and
`-linkto2-wire.txt`; `crates/server/tests/linkto_ab.rs` replays it and
`docs/research/cod11-movers.md` 16 reads it.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=29581 PROBE_SECS=75 \
    tools/run_probe.sh client-probes/probe_linkto2 mp_carentan
# second shell, about 8 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:29581 --probe-team allies --probe-secs 64
```

## probe_trigwait

`Touch_Multi`'s `wait` arm on mp_carentan with four same-length entity lump
edits (`wait` keys on three `trigger_multiple`s, auto2 turned into a
`trigger_once`; the fixture header lists them, and `trigwait_bsp` in
`crates/server/tests/linkto_ab.rs` applies them). `wait` is a gsc keyword, so
the field cannot be written from script; the patched bsp goes into a
`zzz_trigwait.pk3` in the server's homepath, which overrides pak4. The probe
moves one trigger at a time onto the parked player for 20 frames and logs
`isdefined`, the trigger count and every `trigger` notify, then ends on
`enableLinkTo` on a script_origin. The 2026-10-09 run is
`crates/server/tests/fixtures/movers/mp_carentan-dm-trigwait.txt`;
`linkto_ab.rs` replays it and `docs/research/cod11-movers.md` 17 reads it.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=29671 PROBE_SECS=55 \
    tools/run_probe.sh client-probes/probe_trigwait mp_carentan
# second shell, about 10 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:29671 --probe-team allies --probe-secs 40
```
