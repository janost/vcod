# CoD4 1.7 map x gametype sweep

Every claim here is VERIFIED by the sweep itself (vcod-server and the
in-process client on this repo's code, lavapipe, 2026-10-26 and
2026-10-27) unless it
carries its own label.

## The tool

`COD4_DIR=... cargo test -p vcod --bins -- --ignored cod4_map_sweep
--nocapture` (`crates/client/src/play/cod4_sweep.rs`, its header lists the
knobs). Per map, vcod-server loads it with six shooting bots and
`sv_mapRotation` naming the map once for each of war, dm, sd, sab, dom and
koth (time limits 0.5 min, round limits 1); one client joins through the
stock menus (auto-assign, `assault_mp,0`) and plays through
`Cod4Client::pump` into a headless 320 x 240 renderer, firing and walking.
Each level runs to `postgame` and the server's own `map_rotate`; one that
does not end in 120 s, or does not rotate 60 s past its end, is forced on
and marked. A row: the level loaded, its match ended, it rotated on its
own, the client spawned, the script errors (`ScriptRuntime::aborts`), the
bots' stalled share of playing bot-time (bot_match's rule: 12 s inside
150 units, not engaged, in cover, retreating or on an objective), the
client's and the server's own warnings (repeats counted), and a panic.

Run one map a process (`VCOD_SWEEP_MAPS=<map>`, `VCOD_SWEEP_JOBS=1`)
under a local wrapper script: a map peaks near 3 GB and takes 5 to 8 minutes.

## Run of 2026-10-27

All 21 maps, 126 pairs, one map a process under `heavy.sh`, with
`VCOD_SWEEP_LEVEL_SECS=180`. It followed the CoD4 call-scoped `endon`,
the frame-rounded waits, ragdolls, the portal-only cell walk and static
model LODs, client-side rounds, the damage overlay and kicks, and the
HUD menus. The other eleven maps ran first, on the tip with the HUD
menus and voice chat merged; the first ten were rerun on the later tip
with the script model damage (`entity_damage.rs`) merged. mp_creek and
mp_killhouse ran after fix 8 below.

| map | gametype | load | end | rotate | client spawn | script errors | stall % | client warn | server warn | panic |
|---|---|---|---|---|---|---|---|---|---|---|
| mp_backlot | war | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_backlot | dm | yes | yes | yes | yes | 0 (0 distinct) | 1.2 | 0 | 0 |  |
| mp_backlot | sd | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_backlot | sab | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_backlot | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_backlot | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_bloc | war | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_bloc | dm | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_bloc | sd | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_bloc | sab | yes | yes | yes | yes | 0 (0 distinct) | 0.4 | 0 | 0 |  |
| mp_bloc | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_bloc | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_bog | war | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_bog | dm | yes | yes | yes | yes | 0 (0 distinct) | 0.6 | 0 | 0 |  |
| mp_bog | sd | yes | yes | yes | yes | 0 (0 distinct) | 1.1 | 0 | 0 |  |
| mp_bog | sab | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_bog | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.6 | 0 | 0 |  |
| mp_bog | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_broadcast | war | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_broadcast | dm | yes | yes | yes | yes | 0 (0 distinct) | 0.6 | 0 | 0 |  |
| mp_broadcast | sd | yes | yes | yes | yes | 0 (0 distinct) | 1.1 | 0 | 0 |  |
| mp_broadcast | sab | yes | yes | yes | yes | 0 (0 distinct) | 0.3 | 0 | 0 |  |
| mp_broadcast | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.6 | 0 | 0 |  |
| mp_broadcast | koth | yes | yes | yes | yes | 0 (0 distinct) | 1.2 | 0 | 0 |  |
| mp_carentan | war | yes | yes | yes | yes | 0 (0 distinct) | 0.6 | 0 | 0 |  |
| mp_carentan | dm | yes | yes | yes | yes | 0 (0 distinct) | 1.2 | 0 | 0 |  |
| mp_carentan | sd | yes | yes | yes | yes | 0 (0 distinct) | 0.9 | 0 | 0 |  |
| mp_carentan | sab | yes | yes | yes | yes | 0 (0 distinct) | 1.4 | 0 | 0 |  |
| mp_carentan | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_carentan | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_cargoship | war | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_cargoship | dm | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_cargoship | sd | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_cargoship | sab | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_cargoship | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_cargoship | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_citystreets | war | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_citystreets | dm | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_citystreets | sd | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_citystreets | sab | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_citystreets | dom | yes | yes | yes | yes | 0 (0 distinct) | 1.8 | 0 | 0 |  |
| mp_citystreets | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_convoy | war | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_convoy | dm | yes | yes | yes | yes | 0 (0 distinct) | 0.6 | 0 | 0 |  |
| mp_convoy | sd | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_convoy | sab | yes | yes | yes | yes | 0 (0 distinct) | 1.0 | 0 | 0 |  |
| mp_convoy | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_convoy | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.6 | 0 | 0 |  |
| mp_countdown | war | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_countdown | dm | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_countdown | sd | yes | yes | yes | yes | 0 (0 distinct) | 0.8 | 0 | 0 |  |
| mp_countdown | sab | yes | yes | yes | yes | 0 (0 distinct) | 0.6 | 0 | 0 |  |
| mp_countdown | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.6 | 0 | 0 |  |
| mp_countdown | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_crash | war | yes | yes | yes | yes | 0 (0 distinct) | 0.6 | 0 | 0 |  |
| mp_crash | dm | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_crash | sd | yes | yes | yes | yes | 0 (0 distinct) | 7.1 | 0 | 0 |  |
| mp_crash | sab | yes | yes | yes | yes | 0 (0 distinct) | 0.4 | 0 | 0 |  |
| mp_crash | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_crash | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |

The other eleven maps:

| map | gametype | load | end | rotate | client spawn | script errors | stall % | client warn | server warn | panic |
|---|---|---|---|---|---|---|---|---|---|---|
| mp_crash_snow | war | yes | yes | yes | yes | 0 (0 distinct) | 1.2 | 1 | 0 |  |
| mp_crash_snow | dm | yes | yes | yes | yes | 0 (0 distinct) | 1.2 | 0 | 0 |  |
| mp_crash_snow | sd | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_crash_snow | sab | yes | yes | yes | yes | 0 (0 distinct) | 1.0 | 0 | 0 |  |
| mp_crash_snow | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_crash_snow | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_creek | war | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_creek | dm | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_creek | sd | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_creek | sab | yes | yes | yes | yes | 0 (0 distinct) | 1.1 | 0 | 0 |  |
| mp_creek | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_creek | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_crossfire | war | yes | yes | yes | yes | 0 (0 distinct) | 1.2 | 0 | 0 |  |
| mp_crossfire | dm | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_crossfire | sd | yes | yes | yes | yes | 0 (0 distinct) | 1.4 | 0 | 0 |  |
| mp_crossfire | sab | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_crossfire | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_crossfire | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_farm | war | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_farm | dm | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_farm | sd | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_farm | sab | yes | yes | yes | yes | 0 (0 distinct) | 0.3 | 0 | 0 |  |
| mp_farm | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_farm | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_killhouse | war | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_killhouse | dm | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_killhouse | sd | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_killhouse | sab | yes | yes | yes | yes | 0 (0 distinct) | 0.5 | 0 | 0 |  |
| mp_killhouse | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_killhouse | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_overgrown | war | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_overgrown | dm | yes | yes | yes | yes | 0 (0 distinct) | 0.6 | 0 | 0 |  |
| mp_overgrown | sd | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_overgrown | sab | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_overgrown | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_overgrown | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_pipeline | war | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_pipeline | dm | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_pipeline | sd | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_pipeline | sab | yes | yes | yes | yes | 0 (0 distinct) | 0.3 | 0 | 0 |  |
| mp_pipeline | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_pipeline | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_shipment | war | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_shipment | dm | yes | yes | yes | yes | 0 (0 distinct) | 2.0 | 0 | 0 |  |
| mp_shipment | sd | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_shipment | sab | yes | yes | yes | yes | 0 (0 distinct) | 12.1 | 0 | 0 |  |
| mp_shipment | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.7 | 0 | 0 |  |
| mp_shipment | koth | yes | yes | yes | yes | 0 (0 distinct) | 3.4 | 0 | 0 |  |
| mp_showdown | war | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_showdown | dm | yes | yes | yes | yes | 0 (0 distinct) | 0.6 | 0 | 0 |  |
| mp_showdown | sd | yes | yes | yes | yes | 0 (0 distinct) | 0.4 | 0 | 0 |  |
| mp_showdown | sab | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_showdown | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.6 | 0 | 0 |  |
| mp_showdown | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_strike | war | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_strike | dm | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_strike | sd | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 1 |  |
| mp_strike | sab | yes | yes | yes | yes | 0 (0 distinct) | 0.6 | 0 | 0 |  |
| mp_strike | dom | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_strike | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.6 | 0 | 0 |  |
| mp_vacant | war | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_vacant | dm | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_vacant | sd | yes | yes | yes | yes | 0 (0 distinct) | 2.1 | 0 | 0 |  |
| mp_vacant | sab | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |
| mp_vacant | dom | yes | yes | yes | yes | 0 (0 distinct) | 1.8 | 0 | 0 |  |
| mp_vacant | koth | yes | yes | yes | yes | 0 (0 distinct) | 0.0 | 0 | 0 |  |

Every pair loaded, ended, rotated on its own and had the client spawn,
with no script error and no panic. Against the 2026-10-26 run:

- No new script error. The call-scoped `endon` (cod4-gsc.md 4.2) let
  no thread run on into an error, and ended none that a level's end or
  rotation waits on.
- The 12 script errors of that run's first table (mp_bloc `sab`,
  mp_bog `sd`, mp_carentan `sab`, fixes 4 and 5's) are gone.
- The client warnings it counted on many pairs (the root-grafted
  `prop_suitcase_bomb` and stowed RPGs, fixes 6 and 7) are gone. The one
  left is mp_crash_snow `war`'s missing alias (Left 3).
- With 180 s a level, the three `sab` pairs that were forced on
  (mp_creek, mp_overgrown, mp_shipment) end and rotate on their own.
- On the first ten maps the 05c95b34 tip (before the HUD menus) gave
  the same row on every pair but the stall share, which moves from run
  to run (mp_broadcast `koth` 4.8 there, 0.0 here).

## Findings and fixes, by count

Fixes 1 to 7 came out of the 2026-10-26 run, 8 to 10 out of the 2026-10-27 one.

1. 1974 + 38 wgpu validation errors on mp_cargoship `war`: the map has
   more reflection probes than wgpu's default 256 array layers hold,
   so the probe cube array, and every entity bind group with it, was
   invalid. Fixed: the devices ask for the adapter's layer limit, and a
   probe past it is left out (cod4-world.md 9.7). The live client used
   the same default limit.
2. A client panic (`attempt to add with overflow`, `trajectory.rs`) on
   mp_backlot `sab`, which ended that map's sweep: a destroyed car's
   `physicsLaunch` sends `trDuration` 0x7fffffff, and the trajectory's end
   time overflowed. Fixed: the clocks wrap as retail's `int`s do.
3. 20 script errors on mp_backlot `sab`, `_utility::play_sound_on_tag:699`
   "failed to link entity since tag ... does not exist": a CoD4 `linkTo`
   looked the tag up on the `.iwd` search path, which holds no xmodel.
   Fixed: it reads the zones' bind tags (cod4-gsc.md 3.4).
4. 12 script errors, `_globallogic::Callback_PlayerKilled:4787`, on `sd`
   and `sab` (mp_bloc, mp_bog, mp_carentan): the bomb's
   `radiusDamage(..., player)` dropped its attacker, so the kill read the
   world. Fixed: 1.7's builtin reads the attacker and the method's
   inflictor, and `weaponClass` of no weapon is `"none"`
   (cod4-combat.md 10.10).

5. The world's classname: 1.7's
   `SP_worldspawn` writes `worldspawn` (cod4-gsc.md 3.5), and vcod's
   world now carries it, so a blast with no attacker no longer fails
   `Callback_PlayerKilled:4787`.
6. `prop_suitcase_bomb` and `weapon_rpg*_stow` grafted by their root
   (15 pairs of the first run): the roster indexed a new tag after the
   frame's configstring flush, so the client saw `attachTagIndex` a
   snapshot ahead of its string. The roster's writes now flush before
   the messages (cod4-player-anims.md 5). The second run's mp_crash_snow
   (on the old binary) still showed both; the later maps did not.
7. One stowed RPG still grafted by its root on mp_shipment `dom` after
   fix 6: vcod indexed tags only for a playing or dead client (CoD1's
   rule), where 1.7's `ClientEndFrame` indexes every client's
   (cod4-player-anims.md 5). Fixed; not re-swept.

8. The sweep's client panicked at load on mp_creek and mp_killhouse
   (`create_buffer_init`, "Buffer with 'map vertices' label is
   invalid"), on every gametype after the first. With every static model
   LOD baked (the static model LOD work) their world vertex buffers are
   393 MB and 303 MB, past wgpu's default 256 MB `max_buffer_size`. The
   window's device had been given the adapter's limit; the headless one
   the sweep and `pipeline_check` build had not. Both now take one
   `renderer::device_limits`. Gate:
   `renderer::pipeline_check::headless_device_has_the_window_limits`.
   The live client was never affected.

9. The bot stalls of the 2026-10-27 run (mp_shipment `sab` 12.1 %,
   `koth` 3.4 %; mp_crash `sd` 7.1 %) had two causes
   (bot-objectives.md 12.8). On mp_shipment every zone's stand lay
   outside the yard: the graph's largest component is the
   out-of-bounds grounds a ladder's ends flood, and the stand came from
   it, so bots stood pressed against the yard's south wall at y -617.
   Stands now come from the floor the spawns flood. On mp_crash the
   bots ran at a mantle edge's end node, which a body run at is never
   offered the climb; they now run at the landing the graph's climb
   reached. INFERRED (`bot_match` traces). A sweep of mp_crash,
   mp_shipment and mp_strike on the fix (2026-10-27, one process, three
   jobs) read 0.0 % on every pair but mp_crash `sab` 0.7, mp_shipment
   `dom` 0.7 and mp_strike `sab` 0.4. VERIFIED (the sweep). One sweep's
   share moves from run to run: a sweep of mp_shipment alone on the
   tree before the fix read `sab` 0.0 and `dom` 3.3 (VERIFIED, the
   sweep); `bot_match`'s fixed seeds measure the change.
10. The sweep filed a level's load-time lines under the level before:
   the server logs while the console's `map_rotate` loads the next
   level, before the sweep sees the new level and marks its pair. The
   mp_strike `sd` spawn in solid was `sab`'s. The marker now goes in
   ahead of the frame's lines. VERIFIED (the re-sweep files it under
   `sab`).

## Left, ranked

1. Bot stalls: after fix 9 no pair of the three maps re-swept is over
   1 %; mp_vacant `sd` (2.1 %) was not re-swept.
2. mp_strike `sab`: one `placeSpawnPoint` "Spawn point entity 315 is in
   solid", which retail 1.7 prints for the same spawn
   (cod4-collision.md 5): not a divergence.
3. mp_crash_snow `war`: `loopSound alias "emt_christmas_heli_radio" is
   not in the alias table`. The map's createfx script names it, and no
   stock zone carries an alias of that name: the string's only copy in
   `zone/english/*.ff` is the script line. VERIFIED (the inflated zones).
   Retail has nothing to play there either. INFERRED.

## The soak

The sweep checks that every level loads, ends and rotates; the soak
checks what a player would hit on the way. `tools/cod4_soak.sh <outdir>
[map...]` runs `play::cod4_sweep::cod4_soak` one map a process (the run
here went through a local wrapper script as `WRAP`). Per map, vcod-server
loads it with six shooting bots and rotates through war, dm, sd, koth and
sab at 3 min time limits (`VCOD_SOAK_MINUTES`), one round each. The client
joins through the stock menus and plays through `main`'s whole CoD4
frame: `Session::pump`, `scene`, `draw_view`, `ViewTail` and the HUD
quads, rendered at 10 Hz (`VCOD_SOAK_RENDER_HZ`) into a 320 x 240
lavapipe target. Its player (`Player`) walks random paths on the bots'
nav graph, jumps when held up, fires in bursts, throws a frag every 10 to
20 s, opens the scoreboard for 1.5 s every 15 to 30 s and the Esc menu
for 2 s every 25 to 45 s, and answers the team and class menus after
each death and level change.

On top of the sweep's table, `soak.md` counts per pair: the player's
spawns, deaths, frags, board and Esc opens, paths and waypoints reached;
every wgpu validation error; the debug lines that name a problem (an
unknown alias, a missing tag, a menu item or dvar not found); audio cues
whose alias is not in the table; client frames whose CPU side passed
100 ms, by the part that took longest (`pump`, `hud`, `view weapon`, `fx
and sound`, `hud upload`) and what the player did that frame; server
ticks past 100 ms, and those past it on a CPU (cod4-server-frame.md
9); the process's resident set at the level's first and
last sample. `VCOD_FRAME_PROFILE=<path>` adds the frame profile by stage
(`tools/cod4_perf.py`'s format).

The server's first tick on a new map blocks on the bots' nav graph
because the harness asks it to (`wait_for_graph`). A live server builds
the graph in the background, so the soak counts no server stall before
the graph lands.

### Run of 2026-10-27

All 21 maps x 5 gametypes, 105 pairs, about 25 minutes a map, on
613b00de (fix 1 below in). Totals: 522 spawns, 445 deaths, 951 frags,
559 board opens, 268 Esc menus, 1014 paths with 19371 waypoints reached,
880890 frames. The sweep's columns came out clean on every pair: loaded,
ended, rotated on its own, client spawned, no script error, no panic.
With sd at one round, an sd level ran 73 to 221 s. A sab level ran 116 to
316 s: past its time limit the match goes to overtime and ends on a
plant or a defuse (INFERRED from `game["state"]` staying `playing` past
the limit; the 180 s budget the first run gave it forced two levels on).

| finding | pairs | count | what a player sees | status |
|---|---|---|---|---|
| the client kept every snapshot | all 105 | +100 to 250 MB a level | memory grows for as long as you play | fixed (1) |
| first-use loads: a mark's material, an effect's sprite images | 21 (every first level), a few later | 137 of 160 client stalls on the first level, 15 to 250 ms each | a hitch at your first shot into each surface, the first explosion, the first tracer | fixed (2) |
| the first frame after the join | 21 | one 600 to 1300 ms frame | a pause as the level first draws | fixed (3) |
| server tick past 100 ms | 3 | 101 to 125 ms | none (one tick each) | harness (4) |
| missing alias `emt_christmas_heli_radio` | mp_crash_snow war | 1 | none | left (Left 3 above) |
| spawn point in solid | mp_strike koth | 1 | none | left (Left 2 above) |

No wgpu validation error, no unknown sound alias in play, no menu or HUD
expression error and no problem debug line in any of the 105 pairs.

1. A live session's `Client::snapshots` held every snapshot parsed, and
   two per-frame readers (`Cod4Session::scene`'s `rposition`, the damage
   feedback's `update`) walked the whole list. The run before the fix
   (the first five maps) showed the resident set growing 100 to 250 MB a
   level after the first (mp_backlot 3332 -> 3562, mp_bog 3147 -> 3890
   over four levels). A live session now sheds down to the newest 1200
   (one minute at 20 snapshots a second) once it holds 2400
   (`Client::shed`, `KEEP_SNAPSHOTS`). A capture or a demo keeps all of
   its snapshots. After the fix, a level after the first moves the
   resident set by -1262 to +183 MB, with no growth from level to level.
   Retail's client holds `cl.snapshots[PACKET_BACKUP]`, a ring of 32
   (INFERRED, the Q3 lineage's ; not read out of iw3mp.exe).
   Gate: `play::cod4::tests::cod4_live_session_sheds_old_snapshots_and_plays_on`
   (a live join on vcod-server sheds at 80, fires, and the clock, scene
   and own fire events carry on).
2. The fx pass loaded an effect material's images the first frame it
   drew it, and the mark pass built a mark material (images, stage bind
   groups, a pipeline per blend) the first frame a mark of it landed. A
   frame profile of mp_cargoship war (`VCOD_FRAME_PROFILE`) put the
   mid-level CPU spikes in `fxdraw`: `images/antiaircraft_tracer.iwi`
   252 ms, `exp_spark` 72 ms, `dirtchunks` 75 ms, `fire_patch_ember_add`
   77 ms, `ak47_flares` 45 ms, and the marks `gfx_impact_metal_painted01`
   168 ms, `gfx_impact_black01` 167 ms, `gfx_crater_blacktop` 123 ms on
   lavapipe. When the client hands the level's effects to the fx system
   it now builds every `Decal` element's mark material unlit
   (`Renderer::prewarm_marks`) and loads every other element's images
   and the tracer's (`Renderer::prewarm_fx`). A surface's first mark then
   builds only its lightmap's bind groups (0.1 ms). After the fix the same
   level loaded no fx image and registered no mark shader in play. Gate:
   `play::cod4_sweep::cod4_live_first_impact_marks_build_nothing_in_play`
   (a live client on mp_crash fires into the ground; it fails without the
   mark prewarm).
3. The first frame after the join: `net` 622 ms (the gamestate and the
   first snapshots), then `bodies` 389 ms and `pose` 237 ms (the first
   pose of every player model) on mp_cargoship. It comes once a map, as
   the level first draws. Retail spends that time behind its loading
   screen (INFERRED). Fixed: the HUD zones, the effects and every entity
   model now build with the load (`Cod4Client::prewarm`); the `net` part
   was the HUD's zones, not the gamestate (cod4-client-perf.md 17).
4. Three single ticks of 101 to 125 ms (mp_citystreets war and sd,
   mp_shipment war), with the client and lavapipe on the same eight
   cores. Not reproduced. The harness: a client joining a level stalls
   every server in its process for 18 to 66 ms on the address space's
   lock while lavapipe compiles, and a dedicated server alone, nav builds
   included, ran no tick past 27 ms (cod4-server-frame.md 9). The table
   now counts the stalls on a CPU apart.

The renders themselves (lavapipe's `submit` and `acquire`, 100 to 500 ms
when the view takes in the whole map) are not counted as stalls; they
say nothing about a GPU.
