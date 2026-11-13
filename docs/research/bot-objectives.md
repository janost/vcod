# Bot objectives (S&D, retrieval, Behind Enemy Lines, United Offensive)

How vcod's debug bots (`--bots`) play stock Search & Destroy, Retrieval
(section 4) and Behind Enemy Lines (section 5), what changes on a
United Offensive server (section 6), UO's own `ctf`, `dom`, `hq` and
`bas` (section 7), UO's vehicles (section 10), CoD2's `sd`, `ctf` and
`hq` (section 11), and CoD4's `sd`, `sab`, `dom` and `koth` (section
12). The brain is
`crates/server/src/bots.rs` (`ObjTarget`, `think_objective`); the server fills
`BotView::sd` and `BotView::linked` in `crates/server/src/server/bot_world.rs`
(`bot_sd`, `site_stand`) from `ScriptRuntime::sd_objectives`
(`crates/server/src/game/script.rs`). Navigation is in `bot-navigation.md`.

## 1. What stock `sd.gsc` asks of a player

Line numbers are `maps/MP/gametypes/sd.gsc` in `pak5.pk3`.

- VERIFIED (`sd.gsc` `bombzones`, 1797-1814): the sites are the two
  `trigger_multiple` entities with targetname `bombzone_A` and `bombzone_B`;
  `level.planttime` is 5 and `level.defusetime` 10 (seconds). The defuse
  trigger is the entity with targetname `bombtrigger`, sent 10000 units down
  by `_utility::triggerOff` (`maps/MP/_utility.gsc` 134-141).
- VERIFIED (map scripts, `maps/MP/mp_*.gsc`): each map sets
  `game["attackers"]` and `game["defenders"]`. `mp_pavlov` has axis attack,
  `mp_carentan` allies. VERIFIED (grep of `sd.gsc`): the gametype only
  mentions the two assignments in its header comment (51-52) and never
  writes either key, so the sides hold for the whole map.
- INFERRED (`bombzone_think`, 1816-1935, branch conditions): a plant starts
  when a player whose `pers["team"]` is the attackers' touches a zone, is on
  the ground and holds use, while the other zone has no `.planting`. The
  script then `linkTo`s the planter to the zone and counts 0.05 s per frame
  while use stays down; a release or a death before 5 s aborts and unlinks.
- INFERRED (same function, the success arm): a finished plant deletes both
  zones, spawns `level.bombmodel` at `getPlant()` (the planter's feet), moves
  `bombtrigger` to the model's origin and sets `level.bombplanted = true`.
  `bomb_countdown` (1956) runs the 60 s fuse; the explosion deletes
  `bombtrigger` (1956-1990).
- INFERRED (`bomb_think`, 1993-2085, branch conditions): a defuse starts when
  a defender on the ground has `isLookingAt(bombtrigger)`, stands under 64
  units from its origin and holds use. It links the defuser the same way and
  needs use held 10 s; success deletes `bombtrigger`. The progress loop does
  not test `isLookingAt` again, so only the start needs the aim.
- VERIFIED (measured on ours, `mp_rocket`): the moved defuse trigger's bounds
  are centred 8 units above its origin, which is the point the bot aims at.

## 2. What the server tells the brain

- `linked`: `sim.link_to` is set, which is the plant's and defuse's
  `linkTo`. A linked body cannot move, so the brain does not count it stuck.
- `friend_in_line`, every tick in a team game (`Server::bot_friend_in_line`):
  a live teammate's box, grown 8 units (`FRIEND_MARGIN`) for aim error and
  spread, crosses the line from the muzzle point to the enemy's chest short
  of it. The trigger stays up and a running burst ends
  (`bots::tests::a_teammate_across_the_line_holds_fire`). The enemy is the
  10 Hz cached one; the teammates' boxes are this tick's. A design choice
  for vcod's bots, not a retail measurement.
- `sidestep`, with `friend_in_line` (`NavGraph::sidestep`): the nearest
  graph node one or two level edges (no jump, no drop past a step, no
  risky edge) from the bot's nearest, 24 to 128 units off and more across
  the line than along it, from whose eye the line to the enemy passes
  every teammate's grown box and the world (`shot_trace`). While both are
  set the brain walks to it standing, off its sights, the view on the
  enemy, and the next tick checks the line again
  (`bots::tests::a_blocked_bot_steps_aside_to_clear_the_line`,
  `nav::tests::a_sidestep_goes_across_the_line_on_level_edges`). No spot,
  and the bot holds as before. The server keeps a bot's spot
  (`BotDriver::sidesteps`, `bots::held_sidestep`) until the body is
  within 16 units of it, it no longer clears the line, or 3 s have passed;
  only then does it pick again. Picked afresh every tick, the nearest spot
  moved with the body and with a teammate walking across the line, and the
  bot swung between spots (`a_sidestep_is_held_until_reached_or_blocked`).
- The aim. The brain aims from `eye_height`, the body's
  `viewHeightCurrent`, at the enemy's `chest`, two thirds of the enemy's
  own view height up (40 standing, 26.7 crouched, 7.3 prone). Until
  2026-10-15 it aimed from a fixed standing 60 at a fixed 40, while the
  round leaves from `muzzle_point` (combat doc 2.1): a crouched bot put
  every round 20 units low at 900 and hit legs and low walls. The enemy's
  pick tests the line from the eye to the chest and, failing that, to the
  enemy's eye, which then becomes the aim point (`bots::aim_point`): a
  wall that covers the chest but not the head no longer eats every round.
  The trigger waits while `eye_settled` is false, the eye still easing
  between stances, since the aim was taken at the tick's start
  (`bots::tests::a_crouched_bot_aims_from_its_own_eye`,
  `a_chest_behind_cover_is_aimed_over`,
  `the_trigger_waits_for_the_eye_to_settle`). Design choices, not retail
  measurements.
- `gun_offset` (`ClientSim::gun_offset`): the last cmd's shot direction
  less the view it was built from. Down a sight a round leaves along the
  swayed gun, not the view, and from the view the previous cmd left
  (`cod11-combat.md` 15.1); a scope at rest idles up to 0.45 degrees off
  (`adsIdleAmount` 45 times 0.01). The brain turns the view to where the
  gun lands on the aim point, and the trigger tests the gun's line from the
  view it holds now, as a player reads the reticle. Until 2026-10-15 it
  put the view on the target: measured (ours, release, mp_cassino `tdm`,
  6 bots, seeds 1-11 odd, a temporary per-shot dump of the round's line
  against the aim point), scoped shots past 600 units missed it by 10.1
  units on average, 13 of 27 past the 10-unit hit radius the trigger
  allows; with the offset 8.4 on average, none of 38 past it (what is
  left is the skill's own aim error). Kills over seeds 1-23 odd went from
  208 to 254 (`a_scoped_bot_aims_the_swayed_gun`).
- The scope through a rechamber: the brain holds the sight button down
  (`a_scoped_bot_holds_the_sight_through_a_rechamber`). On 1.1 the scope
  stays up; on 1.5 and UO the engine lowers it for the bolt and the held
  button raises it again (`coduo-movement.md` 6), as for a player, so a
  scoped bot shoots about every 1.7 s there. Retail's rule, not the bot's.
- `sd`, only while the level's `g_gametype` is `sd`, read fresh every frame
  because a `map_restart` respawns the zones under new handles:
  - `role`: the bot's `sessionteam` against `game["attackers"]` /
    `game["defenders"]`;
  - `sites`: each zone still standing, its absolute bounds and a standing
    point: the nav graph node inside the bounds (16 units in from the sides)
    nearest the middle, from the largest strongly connected component, so a
    bot anywhere on the main graph can path to it. Cached per bounds for the
    life of the graph;
  - `bomb`: once `level.bombplanted` and while `bombtrigger` exists, its
    origin and the middle of its bounds. `None` again after a defuse or the
    explosion.

## 3. The brain

Goal order: a plant or defuse in progress (use held, or linked), a live
frag, a retreat's cover (bot-combat.md 5), a visible enemy, a health pack for a
hurt bot (bot-combat.md 5), objective travel, then the rest. Use is pressed only at the
objective, since its rising edge also picks up weapons and mounts turrets.

- Sites are dealt out by rank: a bot's place among its team's bots by slot
  (`SdView::rank`), so a team's bots go A, B, A, ... on both sides.
- Attacker before the plant: its site, moved on by one for every failed
  try in a life. Inside the zone (8 units in) it stops, looks 60 degrees up
  and holds use. Held 20 ticks with no link, or 6 s in all, counts as
  failed: it lets go for 10 ticks and goes to the other site.
- The upward look is for the press's rising edge, which also takes the item
  or turret `G_GetActivateEnt` picks (`cod11-items.md` 2.1). INFERRED (from
  `activate_score`'s 128-unit reach and 0.76 cone, about 40 degrees): an
  item lying on the floor within reach sits more than 30 degrees below
  level, so a view 60 up leaves it outside the cone, and a weapon dropped in
  the zone is no longer swapped for the planter's.
- Attacker after the plant: stands 96 to 250 units from the bomb. A body on
  the bomb would block a defender's `isLookingAt`, and the planter starts
  on it.
- Defender before the plant: goes to its site's standing point and holds
  within 250 units of it, looking about.
- An enemy in sight that no round has gone at for 10 s is left out of
  the bot's view for 30 s (`Bot::count_futile`, `Bot::ignores`): two UO
  bots, one holding only an LMG it could not deploy on a stair, the other
  a dry pistol and rockets (which go at no player), stood facing each
  other for twenty minutes of a `bas` round.
- Defender after the plant: only the lead, its team's playing bot nearest
  the bomb (lower slot on a tie, `SdView::lead`), goes for the defuse. The
  rest stand 96 to 250 units off the bomb like the attackers: defenders
  crowding the bomb stood in each other's line from eye to trigger. A dead
  lead hands the defuse to the next nearest on the next frame.
- The lead walks to the bomb, stops within 48 units, turns onto the aim
  point at the normal turn rate, and presses use once the view has sat
  within 1 degree for two ticks (the aim trace is read a frame late,
  `cod11-gsc-object-model.md` 23.1). It holds 10.5 s, or lets go after 20
  ticks with no link and tries again.
- The defuse press's rising edge is `Cmd_Activate_f`'s too, and a weapon
  lying by the bomb (a planter's, dropped where he died) can outscore the
  trigger and swap the held weapon away. INFERRED (from `activate_score`:
  an entity dead on the view scores its distance from the eye): with the
  eye over the bomb, the trigger's middle is nearer than anything lying on
  the floor. So while the end frame's cursor hint names a weapon
  (`BotView::use_takes_weapon`, the hint at or past the protocol's weapon
  base) the lead does not press: it closes in to 16 units of the bomb and
  settles again, and after 5 s of that presses whatever it takes
  (`bots::tests::a_defuser_closes_in_rather_than_take_a_weapon_by_the_bomb`,
  2026-10-18). Not seen in a run yet: in 8 six-bot shoot-on `mp_carentan`
  S&D runs (`ship_stalls`, seeds 1-15 odd, 6000 ticks) no bot killed
  another, so no weapon lay by a bomb.
  The `bas` plant and defuse press the same key on the same edge
  (`bas.gsc` 3184, 3702: `useButtonPressed` while the `trigger_lookat`
  fires) and keep back the same way, closing in to 16 units of the
  charge's model (section 7.2; since 2026-10-20). In six four-bot
  shooting mp_foy rounds no bot settled on a charge with a weapon
  hint, so it is covered by
  `bots::tests::a_bas_planter_closes_in_rather_than_take_a_weapon_by_the_charge`
  only.
- A death sits out the round. VERIFIED (`maps/MP/gametypes/sd.gsc`
  `Callback_PlayerKilled` 756, `spawnSpectator` at 856; `re.gsc` 838, 973,
  `pak5.pk3`): after the 2 s `wait`, the dead player gets the killcam or a
  spectator spawn, never `spawnPlayer`. So on an `sd` or `re` level a bot held in a crease
  (`bot-navigation.md` section 3, "mp_hurtgen's crease") does not send the
  `kill` it sends elsewhere (`BotView::one_life`): held there it is alive,
  can still shoot what comes into view and keeps its team in the round
  (`bots::tests::a_body_held_in_the_air_asks_for_a_kill`). `bel` respawns
  (`bel.gsc` `Callback_PlayerKilled`, `respawn` at 681-757), so it keeps
  the kill.

VERIFIED (measured, `crates/server/tests/bot_objectives.rs`, 2 bots, shoot
off, seed 7, `mp_carentan`): the attacker plants at tick 733 of the run,
counting the match-start restart; the defender defuses 225 ticks later.
Sweeps over seeds 1, 2 and 7 with shoot off, 4 bots on `mp_carentan` and
`mp_rocket` and 6 on `mp_harbor` and `mp_dawnville`, all planted and
defused, the defuse 221 to 234 ticks after the plant. With 6 shooting bots
on `mp_carentan`, seed 1 planted and defused; seeds 2 and 7 planted and
the round ended with every defender dead. `mp_chateau` and `mp_ship` carry
no `bombzone`, `bombtrigger` or S&D spawn entity (VERIFIED, ents lump
strings), so stock S&D does not run there.

## 4. Retrieval (`re`)

### 4.1 What stock `re.gsc` asks of a player

Line numbers are `maps/MP/gametypes/re.gsc` in `pak5.pk3`.

- VERIFIED (`retrieval`, 1878-1886, and `retrieval_spawn_objective`,
  1922-1979): the objectives are the `script_model` entities with
  targetname `retrieval_objective`, gathered into
  `level.retrieval_objective`. Each one's `trigger` field is the
  `trigger_use` it targets, moved onto one of its `mp_retrieval_objective`
  spots, and its `goal` field the `trigger_multiple` it targets.
- VERIFIED (`retrieval_think`, 1981-2042): the pickup is the `trigger_use`'s
  `trigger` notify, which the use key raises through the aim pick
  (`cod11-gametypes-re-bel.md` 4.2). A player of `game["re_attackers"]`
  takes it, and `other.hasobj[self.objnum]` is set to the objective.
- VERIFIED (`hold_objective`, 2044-2073): a taken objective's trigger goes
  10000 units down (`triggerOff`, 2573-2576) and `re_pickup` is logged.
- INFERRED (`objective_carrier_atgoal_wait`, 2075, branch conditions): the
  delivery is the goal's `trigger` notify with the carrier as `other`; any
  other player's touch is let through and ignored.
- INFERRED (`holduse`, 2434-2534, branch conditions): a carrier holding use
  0.3 s is linked in place, and 2 s drops the objective.
- VERIFIED (`objective_think`, 1888-1911, and the default at 184-185): each
  objective and each goal gets a compass record with no team. With
  `scr_re_showcarrier` "0", the default, a pickup sets the objective's
  record to `game["re_attackers"]` and a drop sets it back to none, so the
  defenders' compass loses it while it is carried. With any other value
  the record stays on both compasses.
- VERIFIED (`hold_objective`, 2062; `retrieval_think`, 1983-1984): the
  record follows the carrier (`objective_onEntity`), and the next
  `retrieval_think` after a drop pins it with `objective_position`, which
  detaches it (`cod11-gametypes-re-bel.md` 7.4). While it is carried the
  record's origin still reads where the objective last lay.
- VERIFIED (`Callback_PlayerKilled`, 853-872, and
  `drop_objective_on_disconnect_or_death`, 2617-2760): a carrier's death
  drops the objective without clearing his `hasobj[objnum]`. Only the
  use-key drop (`drop_objective`, 2302-2308) and the capture (2159-2161)
  clear it. The dead carrier's `hasobj` therefore still names the
  objective while it lies there and after the next attacker takes it.
- VERIFIED (`objective_timeout`, 2422-2432): an objective left lying 60 s
  goes back to its start. VERIFIED (`retrieval_think`, 2021-2037): a
  defender's use only prints a message; defenders cannot return it.
- VERIFIED (measured on ours, `mp_dawnville`): the two goal triggers there
  are 500 by 368 units. A defender standing in one fired it every frame it
  was armed, before the carrier's touch (slot order) could, and the carrier
  stood in the goal for 85 s without delivering. INFERRED (Q3 lineage
  `multi_trigger`): a `trigger_multiple` waiting out its `wait` ignores
  every touch, so the same holds on retail.

### 4.2 The brain

`BotView::re`, filled on an `re` level from `ScriptRuntime::re_objectives`:
the bot's role, each objective's pickup trigger middle while it lies there,
its goal's standing point (`site_stand`, as for a bombzone) with the
distance at which a body is clear of the goal trigger, who carries it, the
carrier's position when the bot's team's compass shows the record, and the
record's own origin (where it last lay).

The carrier is the client the compass record follows (`entNum`), not the
one whose `hasobj` names it. Read off `hasobj`, a carrier who died kept
the objective in the bots' view: while it lay, the defenders guarded the
goal instead of it, and once a teammate with a higher slot took it, that
teammate was not told it carried and stood outside the goal.

- Attacker carrying an objective: walks to its goal and never presses use.
- Attacker otherwise: walks to the nearest objective lying there, stops
  within 48 units, turns onto the trigger's middle, and once the view has
  sat within 1 degree for two ticks taps use for two ticks (under
  `holduse`'s 0.3 s). VERIFIED (measured, `mp_dawnville`): from the side
  the graph led to, the eye's trace to the trigger's middle met the world,
  so each tap that took nothing moves the next try 40 units round the
  objective, eight sides in turn.
- Attacker with a teammate carrying: escorts him, holding within 250
  units and closing again when he moves on. Its compass always shows him.
- Attacker with nothing to pick up and no carrier on its compass, and the
  first defender by rank with one carried: a ring round its goal from just
  clear of the trigger to 150 units past.
- Other defenders with one carried: at the carrier while the compass shows
  him (`scr_re_showcarrier` set). Under the default they walk to where the
  objective last lay, once per carry: the carrier set out from there for
  the goal, so the walk runs his likely way backwards. Then the goal ring.
- Defender with nothing carried: guards an objective by rank, within 250
  units of its trigger. A dropped objective counts as lying there.

VERIFIED (measured, `crates/server/tests/bot_objectives.rs`, 2 bots, shoot
off, seed 7, `mp_carentan`): the attacker picks up at tick 483 and delivers
377 ticks later. The same run on each stock map for 150 s: 10 of 12 pick
up and deliver. On `mp_depot` the attacker never reaches the objective,
which lies on an upper floor at z 148; on `mp_hurtgen` every round ends in
an allied win within 18 s and nothing is picked up.

VERIFIED (measured, 2026-10-07, same run): `mp_hurtgen` now picks up at
tick 1108 and delivers at tick 2275. Its rounds ended because the lone axis
bot wandered into the minefields round its spawn (`MOD_EXPLOSIVE`, killer
`world`), and it wandered because the objective's bunker was not on the
graph (bot-navigation.md, "Spacing", "The flood" and section 3).
`mp_depot` still picked nothing up then. Its documents lie on a crate top
at z 148, and the nearest the graph got was a step 32 units below at
(-1852, -608, 116), across a gap the flood never jumped. VERIFIED (measured,
2026-10-07, `tests/bot_objectives.rs`, same run): with leaps
(bot-navigation.md, "Jumps") the attacker comes to rest on the step, leaps
onto the crate, picks up at tick 814 and delivers 224 ticks later.
VERIFIED (measured 2026-10-12, same test): with the leap's lip read off
the tick's move (bot-navigation.md, "Jumps"), at tick 483 and 272 ticks
later; the two leaps before it that walked off the step are gone.

With 6 shooting bots, `mp_carentan` delivered and `mp_harbor` picked up
twice in 150 s and delivered neither.

VERIFIED (measured, ours, 2026-10-08, release, 6 shooting bots, seeds 3
to 6, 200 s per run, the carrier read off the compass record and the
escort and chase above, against the same runs on the code before them):
`mp_carentan` picked up 17 times and delivered 9 (before: 24 and 1);
`mp_dawnville` 19 and 0 (23 and 0), `mp_harbor` 4 and 0 (4 and 0),
`mp_brecourt` 10 and 0 (10 and 0). Kills were 55 to 66 per map in both.
On the three maps that never deliver the carrier dies on the way; the
rounds end on eliminations.

## 5. Behind Enemy Lines (`bel`)

### 5.1 What stock `bel.gsc` asks of a player

Line numbers are `maps/MP/gametypes/bel.gsc` in `pak5.pk3`. The team swap
itself was measured against retail in `cod11-gametypes-re-bel.md` 8.

- VERIFIED (150, 283-310): the team menu is `team_germanonly`, and the
  menu response handler has an `axis` and a `spectator` case and no
  `allies` one. A player only ever joins axis; the script picks who plays
  allied. A bot that answered the menu with `allies` stayed a spectator.
- VERIFIED (`Set_Number_Allowed_Allies`, 1329-1352): `level.alliesallowed`
  is 1 for up to 3 axis players, then one more per 3, up to 11 past 30.
  `CheckAllies_andMoveAxis_to_Allies` (1265-1327) moves random players
  across to keep the allied count there.
- INFERRED (`Callback_PlayerKilled`, 634-760, branch conditions): an axis
  player who kills an allied one becomes allied on his respawn 2 s later
  (`move_to_allies`) while there is room, and the victim respawns axis.
  An allied player who kills an axis one scores a point. An allied death to
  himself or the world moves him to axis and an axis player across.
- INFERRED (`make_obj_marker`, 1563-1606, and `give_allied_points`,
  1608-1628): an allied player scores one point on spawning and one every
  `scr_bel_alivepointtime` seconds (10) while alive, and heals 3 health a
  second under 100. With `scr_bel_showoncompass` 1 (the default, 96-97),
  each one carries objective slot `entnum + 1`, team axis: the axis compass
  shows it. INFERRED (`updateScriptCvars`, 1167-1265, `wait 1` and the
  `update obj` notify; 1588-1603): every `scr_bel_positiontime` seconds (6)
  the marker moves to the mean of where it was and where the player
  stands, so it trails him.

### 5.2 The brain

`BotView::bel`, filled on a `bel` level: whether the bot is allied (the
hunted side), and the live objective records its team's compass shows
(`GameHost::objectives` filtered as `objectives_for` does), which on the
axis side are the allied players' markers.

- Every bot answers `team_germanonly` with `axis`.
- A visible enemy comes first on both sides, as everywhere.
- Hunted (allies): with no enemy in sight, `Goal::Away` from the last
  enemy it saw or the gunfire it heard (`Bot::recall`, else the tick's
  noise), else from its own marker (record `entnum + 1`), else `Roam`. It
  never walks toward a noise. Its compass does not show the marker; the
  bot reads it because a player knows the hunters walk to where he was,
  and moving off it keeps the marker trailing. The points come from
  staying alive.
- Hunters (axis): a remembered or heard spot first, else the nearest
  marker it has not yet stood at, else `Roam`. The marker sits still for
  6 s, so a hunter that reached it looks round from there (the roam)
  instead of standing on it, and heads for it again once it moves.

VERIFIED (measured, ours, 2026-10-08, `mp_brecourt` `bel`, 6 shooting
bots, release, two runs of 120 and 90 s): all six joined; the axis
killed an allied bot 8 times in each run, so 8 swaps each; allied bots
killed axis ones 6 and 7 times and earned 17 and 11 `bel_alive_tick`s.
Whether the hunt and the flight look sensible is pending a hand check.

VERIFIED (measured, ours, 2026-10-08, release, 6 shooting bots, seeds 1
to 4, 150 s, `mp_brecourt` and `mp_carentan`): with the flight from the
bot's own marker the allied side earned 160 `bel_alive_tick`s over the
eight runs, against 163 without it. The flight changes how the hunted
move, not measurably how long they live.

## 6. United Offensive

Line numbers are `maps/mp/gametypes/sd.gsc` in UO's `pakuo00.pk3`.

- VERIFIED (`sd.gsc` 2125-2133, `bombzone_think` 2141-2205, `bomb_think`
  2350-2362): the sites, the bomb trigger, the plant and defuse times and
  the defuse's `isLookingAt` and 64-unit reach are 1.1's. The plant adds
  `!(other isinvehicle())` and `_util_mp_gmi::canPlantGMI()`
  (`maps/mp/_util_mp_gmi.gsc` 373-415: a `bulletTrace` 18 down from 10
  over the feet, 11 ahead along the view, then at the feet, then at four
  corners 16 out, hits something not a `script_vehicle`), at the start
  and every frame of the hold. A bot standing on the zone's floor passes both,
  so the brain is section 3's unchanged.
- VERIFIED (`checkMatchStart` 1503-1531, `endRound` 1558-1560): UO's
  version notifies `kill_endround` and then threads `endRound`, whose
  first line is `level endon("kill_endround")`; 1.1's threads it only.
  vcod's VM queued a notify to the end of the step and killed the new
  `endRound`, so the match-start round reset never ran, the round's
  `level endon("round_ended")` had already ended both bombzone threads,
  and no plant could start for the rest of the map. The VM now acts on a
  notify where it executes, as retail does (`cod11-gsc-language.md` 9,
  `probe_endon_late`). The same pattern opens UO's `re`, `bas`, `ctf` and
  `dom`.
- VERIFIED (measured, ours, 2026-10-14, `mp_cassino` `sd`, 2 bots, seed
  7, UO rules, `tests/bot_objectives.rs`
  `uo_bots_plant_and_defuse_on_cassino`): the attacker plants at tick 779
  and the defender defuses 224 ticks later; the next round plants at 1778
  and defuses 224 later again.

Weapons, from UO's menus (`ui_mp/scriptmenus/weapon_*.menu`,
`pakuo00.pk3`) and `weapons/mp/*` (`pakuo07.pk3`), all VERIFIED (the
files):

- UO's menus add `mg30cal_mp` to the American and British, `svt40_mp` and
  `dp28_mp` to the Russian, `gewehr43_mp` and `mg34_mp` to the German, and
  `bel`'s two-nation menus (`weapon_americangerman` and kin, `bel.gsc`
  222) offer both sides'. A UO bot answers from `bots::WEAPON_MENUS_UO`,
  a two-nation menu from both nations' lists. The three `weaponClass lmg`
  guns (`mg30cal_mp`, `mg34_mp`, `dp28_mp`; `scr_allow_mg30cal`,
  `scr_allow_dp28`, `scr_allow_mg34` in `_teams.gsc` 1592-1753) fire only
  off the deployed bipod with the sight at 0.99 or more
  (`coduo-movement.md` 6, 7). `bren_mp` and `bar_mp` are `weaponClass
  rifle`.
- An LMG bot (`Bot::bipod`): with an enemy in sight it holds the sight
  button, which deploys the bipod where it stands (on a mount, or prone),
  keeps its move keys at 0 and fires once `PlayerState::deployed` and the
  sight fraction is 0.99. A deploy that has not taken 10 ticks after the
  press (a wall ahead, no room to go prone) lets the button go for 2 s,
  and so does a target more than 50 degrees round `proneDirection` (the
  cap is `bg_lmg_yawcap`'s 55): the breakdown, then a fresh deploy facing
  it. The bipod stays down 3 s after the target is lost. An LMG is put
  away only with the bipod up (the putaway, `coduo-movement.md` 7), and
  its 1.8 s deploy is as long as a fight at pistol range, so an LMG bot
  with an enemy inside 500 units, or resting its button, draws the pistol;
  no frag is thrown with the bipod down. Whether a deployed bot looks
  right is pending a hand check. VERIFIED
  (measured, ours, 2026-10-15, `ship_stalls` `UO=1 SHOOT=1`, `mp_cassino`
  `tdm`, seeds 13, 15, 17, 33): the deployed bots stand for the stall
  count's 12 s, 952, 1 049, 951 and 1 570 stalled bot-ticks against 0,
  0, 187 and 20 with the three LMGs restricted, and their kills were 2 of
  the 25 on seed 33. `ship_stalls` and `uo_bots_fight_and_sprint_in_tdm`
  no longer count a deployed bot (`BotBody::deployed`).
- UO's grenades are `fraggrenade_mp` and its three national twins
  (`weaponSlot grenade`), `smokegrenade_mp` and `flashgrenade_mp`
  (`weaponSlot smokegrenade`, no damage) and `satchelcharge_mp`
  (`weaponSlot satchel`). A bot throws only the `grenade` slot's.
- Every UO grenade has `cookOffHold 1` and the frag's `fuseTime` is 4
  (`coduo-movement.md` 12): the fuse starts at the sight button's edge
  once the pullback runs. A UO bot's throw holds the trigger 24 to 33
  ticks and the sight button from the fifth, so it cooks 1.0 to 1.5 s and
  lets go with 2.5 s or more left; a 1.1 bot's hold is 6 to 13 ticks with
  no sight button (`bots::tests::a_uo_frag_is_spooned_after_the_pullback`).
- A throw aims (`Bot::lob_angles`, `bots::tests::a_throw_lobs_over_the_enemy`):
  through the switch, the hold and the switch back the view turns toward
  the enemy in sight and up over it, 5 to 25 degrees with the range. The
  grenade stages used to send zero angles, so on 1.1 too the frag went
  wherever `delta_angles` pointed. No throw goes at an enemy nearer than
  250: the frag's blast is 350 units, 120 damage at its centre falling to
  5 at its edge (`fraggrenade_mp`, both patches), so at 250 it still
  takes 38 from the thrower. VERIFIED (measured, ours, 2026-10-14,
  `mp_cassino` `tdm`, 6 shooting bots, 120 s, ten seeds, release): before
  the aim and the minimum, 3 of 8 frag kills over five seeds were the
  thrower's own; after, 1 of 11 over ten (seed 8).
- What still killed the thrower (VERIFIED, measured, ours, 2026-10-15,
  same runs, `crates/server/examples/obj_watch.rs` with each throw and
  missile contact logged): over seeds 1 to 19 odd, 3 of 15 frag kills.
  One frag struck a teammate's body 25 units in front of the hand and
  came straight back past the thrower (a stock frag bounces off a live
  player, and vcod's UO bounce then damped only in water; since
  2026-10-16 it keeps a tenth off a body, coduo-diff.md 21); in two the enemy had
  closed to 77 and 105 units during the hold. Over seeds 21 to 39 odd
  more: lobs from an upper floor into its ceiling or a window frame that
  dropped the frag at the thrower's feet, a short throw that rolled back,
  and throwers that ran after their enemy into their own blast. Hence:
  - `BotView::lob` (`Server::lob_probe`): of the lobs 5 to 40 degrees
    over the line from the eye, every 5, nearest first to the one the
    range asks for, the first whose flight (the throw's launch, gravity,
    50 ms steps for 1.5 s) meets no wall and no body before 0.6 of the
    range and 250 units across. No throw starts without one, the hold
    turns onto it, and the release waits for one and for the view within
    2 degrees of it in pitch and yaw. An enemy that leaves sight during
    the hold is still thrown at where it was last seen, by a lob probed
    there (`Bot::throw_target`).
  - An enemy inside 250 units at the release: the frag goes the other
    way, by a lob probed at a spot 400 off along that yaw, else 30 up.
    The release waits until the view is on that lob, or within 30
    degrees of the throw's yaw without one, 16 ticks at most (a UO frag
    cooks on meanwhile, with 2.5 s or more left).
  - `BotView::frag_near`: a live frag or satchel within 400 across and
    160 up or down, the bot's own or one it notices (`bots::notices_frag`,
    since 2026-10-21: in a clear line and within 60 degrees of the view,
    or within 150; bot-combat.md 6.5). The goal is away from it and the
    keys run that way, the view still on the fight.
  VERIFIED (measured, ours, 2026-10-15, release `ship_stalls` `UO=1
  SHOOT=1`, `mp_cassino` `tdm`, 6 bots, 120 s, seeds 1 to 39 odd): with
  the turn-away and the release wait alone, 5 of 35 frag kills were the
  thrower's own; with the flight from live frags too, 4 of 17; with the
  lob probe, 0 of 12, among 499 kills. Fewer frags kill at all: the
  probe passes on most throws from indoors, and everyone runs from one.
- Since 2026-10-21 (bot-combat.md 6.5) a throw goes only where the gun
  cannot: at an enemy in sight no round has gone at for 1.5 s, or where
  a slow one was lost 0.2 to 3 s ago; from a body at rest; never with a
  teammate within the blast of the landing (`Server::lob_probe`). A 1.1
  or 1.5 frag with no clear lob once the hold's wait is out is put back
  through a switch to the rifle rather than let go.
- By 2026-10-16 three releases still went off the probed flight
  (VERIFIED, measured, ours, release `ship_stalls` `UO=1 SHOOT=1`,
  `mp_cassino` `tdm`, 6 bots, 120 s, each throw logged): the release
  checked only the yaw, and seed 1's frag went at -22 pitch with the lob
  at -5.75, struck the wall over its enemy 240 units off and rolled 360
  back past the thrower; seed 27's enemy left sight in the hold and the
  frag went along the view into the cover; seed 39's turned-away throw
  went 30 up into a wall behind. Seeds 1 to 39 odd: 2 of 14 frag kills
  the thrower's own before, 0 of 11 after the pitch wait, the remembered
  target and the probed turn-away; seeds 41 to 79 odd, 2 of 4 before
  and 1 of 2 after. The one left (seed 73) is a throw at 269 units by a
  bot already hurt: the blast at that range is about 30, which the
  250-unit minimum accepts.
- The tick a throw started still went out with zero angles and no target
  recorded: only the stage ticks after it aimed and remembered the enemy.
  VERIFIED (measured, ours, 2026-10-19, release, `tests/bots.rs`
  `uo_bots_fight_and_sprint_in_tdm`, seed 1, each start, hold and
  release logged): bot 5's start tick turned its view from yaw 155 onto
  `delta_angles`' 45, the enemy left sight on the next tick, and with no
  target the hold neither turned nor waited: the cooked frag went flat
  along 45 and killed the thrower. VERIFIED (bisected, ours): the gate
  passed until master ace5f143 (the pickup reads the cmd byte). INFERRED
  (read off `Bot::think_grenade` and `think_cook`, whose byte names the
  frag from the start tick on): that merge changed the game's course,
  not the throw. The start tick now aims and records its target
  like the stages (`bots::tests::the_tick_a_throw_starts_aims_and_records_the_enemy`).

VERIFIED (measured, ours, 2026-10-14, `mp_cassino` `tdm`, 6 shooting
bots, seed 1, 90 s, `tests/bots.rs` `uo_bots_fight_and_sprint_in_tdm`):
24 kills, none with an LMG or by a bot's own frag, 470 sprinting
bot-ticks, no bot 12 s in one spot, no script abort (with the UO weapons
of `coduo-diff.md` 21 merged). Bots never drive or ride a vehicle.

By 2026-10-15 that gate failed on master: `tick 1083: bot 5 12 s at
[2220.4, -864.0, -7.9]`. VERIFIED (measured, ours, release, a temporary
per-shot dump of the trace's hits and the brain's aim): bot 5 held a
crouched scoped duel at about 900 units, a shot every 2 s or so (the bolt
and the scope's re-raise), and its rounds left from the crouched eye at
40 while it aimed off 60: 1.3 degrees low, so legs (60 and 72 damage)
and a low wall took them. Over seeds 1 to 23 odd the gate failed on 8 of
12 before the aim fixes above and on 2 of 12 after them and the
sidestep. Of the two, seed 5's bot 5 stood 12 s in a long duel with
enemies on a height 1 200 units off, firing and hitting, and seed 17's
bots sprinted 82 bot-ticks against the gate's 100 (sprint counts run
82 to 604 over the twelve seeds). So the gate counts a bot's stall over
its ticks without an enemy (`BotBody::engaged`: the brain tracks a
target seen in the last second), as it already skipped a deployed LMG:
a scoped shot at range stands still by design (`Bot::footwork`). With
it, 11 of 12 seeds pass; seed 17 still misses the sprint floor. Seed 1,
the gate's: 21 kills, 465 sprinting bot-ticks. `SEED=<n>` reruns the
gate on another seed.

The sprint floor of 100 bot-ticks was the seed's more than the brain's:
how many straight legs a game routes over decides it. Since 2026-10-15g
the gate counts the bot-ticks on a leg the brain sprints when it has the
breath (`BotBody::sprint_leg`) and asserts more than 100 of them and that
at least half are sprinted. VERIFIED (measured, ours, release, seeds
1-23 odd, with the gun-offset aim and the held sidestep): 242 to 1084
sprinting bot-ticks on 243 to 1242 leg ticks, 82 to 99 per cent; 17 to
25 kills; all 12 pass. Master on the same seeds: 210 to 705 sprinting
bot-ticks, 10 to 22 kills.

On 2026-10-16 the gate failed again, on its frag assertion: seed 1's
bot3 killed by its own frag (the high release in the frag list
above). VERIFIED (bisected over master's first-parent merges, ours,
release): the first failing merge is b876e696 (`fix/prone-first-lie-15`,
a 1.5 and UO spawn thinks on the client's last cmd), which touches no
grenade code; it moves every respawn, so seed 1 plays another game, one
with that throw in it. With the release fix the gate passes on seeds 1
to 15 odd. In release the gate runs in about 4 s (mp_cassino's graph
builds in under 1 s), the full 1 800 ticks.

On 2026-10-17 it failed on its frag assertion once more, at seed 1 on
`fix/uo-spawn-connectivity` (whose graph changes move every route) and
at seed 59 on master (2397f7a4; that game was not traced). VERIFIED
(measured, ours, release, a temporary per-tick dump of bot4's body and every armed missile, seed 1):
bot4 at (1485, -359, 72) at the foot of a stair, its enemy 193 units off
and 124 up on the landing, turned its throw away, up and behind; the
frag struck the wall at (1564, -196, 161), came back off it and rolled
down the steps to (1431, -275, 114), 100 units off, by tick 899.
bot4 stood still through tick 909: a thrower is in its grenade stages
(`Stage::Cook`, then `Stage::BackToRifle` through the stick grenade's
throw and the switch back), and `Bot::think` returned from those before
the run from `frag_near`. It set off at 910 and took the blast at 928,
222 units off. The run now applies to the grenade stages too
(`Bot::flee`). With it seeds 1 and 59 pass, and seeds 1 to 63 odd show
no own-frag kill.

## 7. United Offensive's `ctf`, `dom`, `hq` and `bas`

Line numbers are `maps/mp/gametypes/<gt>.gsc` in `pakuo00.pk3`; the
server reads each gametype's state off the script every frame
(`ScriptRuntime::uo_objectives`) and deals each bot a `bots::UoView`
(`Server::bot_uo_view`), its zones' standing points by `site_stand` as
for a bombzone. Roles go by rank (the bot's place among its team's bots
by slot): odd ranks keep their own side's objective, even ranks go for
the other side's, so a lone bot attacks. A visible enemy still comes
first everywhere.

### 7.1 What the scripts ask of a player

- VERIFIED (`ctf.gsc` `ctf`, `ctf_spawn_flag`, 2514-2699): each team's
  flag is the entity `ctf_flag_allies` / `ctf_flag_axis` in
  `level.allies_flag` / `level.axis_flag`, its `trigger` the
  `trigger_multiple` it targets, its `goal` the `trigger_multiple` its
  `mp_gmi_ctf_flag` spot targets, and `mobile_trigger` the one its
  `_mobile` model targets, sent 10000 down (`triggerOff`, 3424) while the
  flag is home. VERIFIED (measured, ours, mp_cassino): the axis flag's
  goal lies at the allied flag's home and the other way round, so a
  carrier captures at his own base.
- INFERRED (`ctf_think`, 2701-2814, branch conditions): a touch of the
  flag's trigger, or of the mobile trigger once it has `moved`, by a live
  player of the other team takes it (`hold_flag`: `carried_by`, `moved`,
  both triggers off); a touch of the mobile trigger by its own team
  returns it home (`reset_flag`). A player in a vehicle is passed over.
- INFERRED (`flag_carrier_atgoal_wait`, 2946-3087): the carrier's touch
  of the carried flag's `goal` captures only while his own team's flag
  is not `moved`; every other touch is passed over and the wait goes on.
- VERIFIED (`drop_flag`, 3125-3257; `flag_timeout`, 3259-3291): a
  carrier's death leaves the flag at his feet with the mobile trigger on
  it, and 20 s lying there sends it home. With `scr_ctf_showoncompass`
  0, the default (304), no compass marker follows a carried flag.
- VERIFIED (`dom.gsc` `Flag_InitTriggers`, `Flag_ZoneThink`, 1461-1750):
  the flags are the triggers `flag1` .. `flag14` by targetname, each with
  a `team` (`neutral` at the start). INFERRED (branch conditions): each
  frame of a zone with live players of one team only in it, not in a
  vehicle and not its owners, adds 0.05 s, and half again for each
  capper past the first, until `script_timer` (10 by default) turns it;
  any player of the other team in the zone stops the count.
  `Flag_AllCapturedThink` (1757-1800) ends the round when one team holds
  every flag.
- VERIFIED (`hq.gsc` `hq_setup`, 1720-1755; `hq_radio_think`, 1871-2168):
  the radios are `level.radio`, the `hqradio` script models, one at a
  time on show (`hidden` false), each with a `team` (`none` while
  neutral) and a `radius` (`script_radius`, else `level.radioradius`
  120); `level.zradioradius` is 50 (186-196). INFERRED (branch
  conditions): a playing player not in a vehicle within `radius` across
  and 50 up or down counts; a neutral radio with one team only near it
  and that team holding no other is captured over `RadioCaptureTime`,
  and a held one with the other team near it and no holder is
  neutralized over `RadioNeutralTime`. The next radio's marker is
  objective 0 at its origin before it shows (`hq_obj_think`, 1792).
- VERIFIED (`bas.gsc` `BaseAssault_GMI`, 4089-4186): the bases are
  `level.bases`, the `gmi_base` script models, each with `script_team`,
  `health` (`scr_bas_basehealth`, 24500) and `alive`; a team wins when
  the other has none alive. INFERRED (`BaseAssault_base_think`,
  3844-4060): only a player's `MOD_PROJECTILE*`, `MOD_GRENADE*` or
  `MOD_ARTILLERY*` from the other team takes health off (3954); at 0 the
  base is breached, and every attached thing with a `script_idnumber`
  (its walls, `script_brushmodel`s) is deleted. INFERRED
  (`BaseAssault_ObjectiveZone_think`, 3666-3800): a breached base's
  `objective` (a `trigger_lookat`, `BaseAssault_get_objective` 3081)
  fires for a player of the other team looking at it within 150 units
  of its `objectivemodel`; on the ground with use held he is linked and
  plants over `scr_bas_planttime` (15 s), the hold breaking past 155
  units. `BaseAssault_bomb_think` (3137-3273) is the defuse, the same for
  the base's own team over `scr_bas_defusetime` (10 s).
  `BaseAssault_bomb_countdown` (3586-3664) blows after `scr_bas_bombtime`
  with a 1000-damage `radiusDamage` on every player within 500 units,
  and kills the base.
- VERIFIED (`bas.gsc` 3137-3157, 3852): `BaseAssault_bomb_think`
  decrements `count` when its trigger fires with the match or round not
  started or the round ended, and nothing in the function sets it;
  `BaseAssault_base_think`'s twin sets `count = 1` first. INFERRED (the
  script alone): a player looking at a live charge in the end-of-round
  delay (`endRound`'s `wait` of `scr_bas_endrounddelay`, 2260-2264, 10 s
  by default, 186, between `level.roundended` and the `W;` line) stops
  that thread with a script error on any engine. At `developer 0`
  the thread runs on past it and 3153's `if(count == 0)` reads the same
  unset `count`: VERIFIED (measured, `four_shooting_uo_bots_end_a_bas_round`
  on the parked-vehicle change, bot-navigation.md "Parked vehicles"), a
  second abort. The bas gates in `tests/bots.rs` pass over the two.
- VERIFIED (`bas.gsc` 330-345): `scr_bas_planttime` 15,
  `scr_bas_defusetime` 10 and `scr_bas_bombtime` 60 by default.
  INFERRED (`BaseAssault_bomb_countdown`, 3591-3598, loop body): the
  base's `countdowntime` starts at the bomb time and drops by one after
  each `wait 1`, so it reads the whole seconds left on a charge.
- VERIFIED (mp_foy's entity lump, and the same on mp_italy, mp_kursk,
  mp_ponyri and mp_rhinevalley by the charges' heights): each base's
  charge sits in a room under the bunker, 176 units under the base's
  origin, down a stair whose way in is the base's `script_idnumber`
  walls (`*66` for mp_foy's axis hq), and whose stairwell holds the
  base's `rubble` brushes (`script_noteworthy rubble`, `*21`), which
  `BaseAssault_base_think` lifts 1000 units at the start (3908-3924) and
  `BaseAssault_KillBase` drops back (3475-3544). So nothing reaches a
  charge until its base is breached.
- VERIFIED (retail UO 1.51b, `client-probes/probe_spawnint` on mp_foy,
  2026-10-15, gate `spawnint_uo_ab`): those walls' `"script_idnumber"
  "foo"` reads 0, defined, as `strtol` leaves it. Ours left the field
  undefined and never deleted a breached base's walls.

### 7.2 The brain

- **ctf.** The carrier makes for the carried flag's goal, and waits inside it
  while his own flag is away. His own flag lying out is returned by a touch: a
  defender goes for it, and so does an attacker within 1000 units or one whose
  team's carrier is out. A defender stands by his flag at home, or by the goal
  its carrier needs while it is carried. An attacker takes the other flag, at
  home or lying out, else escorts the teammate carrying it.
- **dom.** A bot sets out for a flag its team does not hold, the rank-th
  nearest, keeps it until its team holds it, and stands inside the zone (8
  units in) looking about. With every flag held it guards one by rank.
- **hq.** Neutral or the other team's radio: stand within its radius less 16
  and 34 up or down. Its own team's: guard it. None up: wait by the next one's
  marker.
- **bas, the plant.** An attacker takes the other side's alive bases by rank,
  and after each death the next one, off the defender that killed it. A
  breached base with no charge is planted: walk within 112 of the objective
  model, rest the view on the trigger's middle, hold use 15.5 s (S&D's defuse,
  section 3, with reach and hold as parameters, the close-in over a weapon
  lying by the charge included).
- **bas, the planted charge.** Guarded from within 250 units (its room or the
  bunker over it, the way a defuser comes) while `countdowntime` reads more
  than 20 s, then from 520 to 670 off, outside the blast, the bot leaving by
  the graph's flee (`Goal::Away`). Trap: guarded from outside the blast all
  along, every plant was defused unseen once each side had a defender.
- **bas, the blast.** Any bot inside the 500-unit blast of a charge about to
  go (its own side's in the last 20 s, the other side's once a defuse can no
  longer finish) flees it before anything but a live frag, fight or not: the
  gun stays on the enemy and the keys run the graph's way out, turned to the
  aimed view, no sight, no bipod (`rekey`). Trap: with the fight steering the
  keys, a planter and a defender shot it out in the charge room and both died
  in the blast (seed 5, four times a round).
- **bas, the defuse.** Each side defuses a charge on its bases (10.5 s): a
  defender that shoots, and any other bot within 2000 units. No bot starts one
  that the clock, less the straight-line run at 190 units/s, leaves no 10.5 s
  for.
- **bas, the defender.** It keeps the charge room of a breached base of its
  side's, the only way a plant comes; with none breached it attacks with the
  rest. With its side's charge ticking on the other side's base it holds that
  charge's room beside the planter instead (one on one, the defender in its
  own room won most fights and defused), and the same once its side has
  breached the other side's last base, charged or not. Trap: called only by
  the charge, it crossed the map after the planter had died and the charge was
  defused; seed 23's last base took 20 min.
- **bas, a defender that does not shoot** (`--bots-shoot` off) stops no plant
  from the room, so it attacks with the rest and defuses only a charge within
  2000 units; a side's two such bots split over the other side's bases by rank
  (shooting, both take the same one), so two charges can tick on a side with
  one bot left. A bot that does not shoot cannot stop a defuse once the
  defuser reaches the trigger: the hold ends only on release, a death or 155
  units (`BaseAssault_bomb_think` 3210), and on mp_foy's axis charge room a
  body in the lookat's line found no free floor (the table runs to about 20
  units of the defender's spot). Trap: running home to defuse from anywhere,
  it was back for every plant (about 38 s of a 60 s fuse on mp_foy), and the
  round looped on defuses (seeds 17 and 19, 64 to 68 in 29 min).
- A standing base is breached with rockets (below). A bot in a tank (section 10)
  lays its gun on players and vehicles, not bases.

The breach:

- An attacker with no loaded rocket launcher (`BotView::rocket`: `weaponClass
  rocketlauncher`, clip or reserve, the one in hand first, else any) walks to
  the nearest launcher lying about with a round left (`BasView::launchers`,
  off `ScriptRuntime::weapon_items`) and taps use on it, the retrieval
  pickup's tap (section 4.2): walking over a weapon it does not carry takes
  nothing (`cod11-items.md` 1). It passes over one within 300 units of a
  standing base of the other side's (inside that bunker's walls).
- With both primary slots full the pickup drops the weapon in hand, or slot
  1's with the pistol out, never the pistol (`cod11-items.md` 5, cases 2 and
  4). So the bot first draws the primary it would rather lose
  (`Bot::swap_out`, `BotView::primaries`): an empty one before a loaded one,
  then one of the kind it fetches, then not the gun it fights with; with a
  slot free it draws nothing. Trap: reading only the first launcher held, an
  empty bazooka beside a loaded panzerschreck read as none, and a bot with its
  gun out traded the gun, then launcher after launcher, for five minutes with
  no breach (mp_foy, shooting, seed 15).
- With a launcher in hand it walks to a breach stand. `Server::breach_stands`
  picks once per base and graph a node in each of 16 sectors round the base,
  400 to 1000 units off (nearest 600), from whose standing eye a rocket
  strikes the base's model or bursts within 64 units of its origin
  (`breach_aim`: the world's missile trace and the meshes of script models and
  vehicles within 1564 units of a base, under the missile's clipmask). The bot
  takes the stand nearest its own side's nearest base, and the next one after
  2 s there with no shot.
- Wherever `Server::breach_shot` (from its muzzle, 400 to 1500 off) finds a
  shot, it stops, draws the launcher, rests the view on the aim, fires and
  reloads until the launcher is dry, then fetches another. With shoot on a
  visible enemy comes first: the gun, else the pistol, comes out over the
  launcher, and no rocket goes at a player.
- No breach rocket goes while another player's box crosses its line, where it
  would burst within 320 units of the muzzle (`BREACH_CLEAR`; the base's mesh
  reaches toward the shooter), or where a ray along the edge of the sighted
  spread cone (eight rays, 4 degrees, the bazooka's and panzerschreck's
  `adsSpread`; the panzerfaust's 3.5) meets anything within 320
  (`spread_clear`). Trap: a 320-unit range floor alone left a bot killed by
  its own splash in most four-bot rounds; the line's middle alone let the
  spread carry a rocket into the cover by the stand on mp_foy, and on mp_italy
  a bot fired twice into the Sherman (`script_vehicle` at (-3043, -2347, -40),
  190 units on, 1290 from the base, outside the 1200 the models were then
  taken within) and died of the second.
- Those checks hold only at the sighted floor, so the shot also waits for
  `aimSpreadScale` to return to 0 (`BotView::spread_scale`): a turn while the
  sights rise adds to it (`adjust_aim_spread_scale` adds nothing once
  `fWeaponPosFrac` is 1, and the decay takes a few frames). VERIFIED
  (measured, ours, release, UO install, four-bot shooting gate, mp_kursk seed
  11 on 85b67024): the axis bot at the stand (778, -10551, 392) turned about 7
  degrees with its sights half up, fired 3 ticks after they were up, and the
  rocket left along (-0.916, -0.420, -0.010) against the aim's (-0.953,
  -0.303, 0.011), 7 degrees off where `spread_clear` clears 4; its splash
  killed it (72, `MOD_PROJECTILE_SPLASH`, tick 15068).
  (`bots::tests::a_bas_attacker_with_a_rocket_walks_to_a_stand_and_fires`.)

Out of rounds:

- A bot with both weapons spent draws the gun it still has. With no round
  anywhere it fetches the nearest weapon lying within 1500 units with a round
  in clip or reserve (`BasView::guns`, `DRY_FETCH`;
  `ScriptRuntime::weapon_items` reads a drop empty only when count and clip
  both hold -1, `cod11-items.md` 8; a clip-only test passed over reserve-only
  drops), and uses `kill` after 10 s only with none in reach or once that
  fetch is given up. Every shoot-on `kill` in six mp_foy rounds was a dry one;
  two were a planter dry in its charge room whose charge was then defused
  (seeds 1 and 7).
- A shooting bot with only a rocket left (no gun, pistol dry) that gave up on
  an enemy it could not fire at (section 3, the futile enemy) fetches a gun,
  never a launcher, within 800 units (`Bot::unarmed`, `ARMS_FETCH`). Trap: on
  mp_foy seed 23 an allied planter and the axis defender, each with a loaded
  launcher and a dry pistol, stood in one charge room for 5 min: eight plants,
  seven defuses, no shot. The full 1500-unit reach cost seeds 7 and 11 a
  minute and more.
- A launcher or gun the body has come no 32 units nearer to in 20 s (a node
  over it, or a tap that takes nothing) is passed over for a minute.
- An LMG bot whose target's pitch lies past 45 degrees does not deploy, or
  breaks the bipod down, as for a target past the yaw cap, and draws the
  pistol when it has a round
  (`bots::tests::an_lmg_target_past_the_pitch_clamp_breaks_the_bipod_down`). A
  deployed LMG's view stops 45 degrees from level (`PM_UpdateViewAngles`,
  coduo-movement.md 7). A prone deploy measures the clamp from the torso's
  pitch, which the brain does not read; it uses level. VERIFIED (measured,
  ours, master 447d904f, mp_foy seed 21, `BAS_TRACE=1 BAS_TRACE_EVERY=20`):
  before it, a planter in the charge room at (3530, -4832, -169) and a
  defender in the bunker over it (3611 to 3642, -4705, 14), 170 to 260 apart
  across the hole, both deployed, fired 75 MG34 and 59 .30 cal rounds into the
  hole's lip (the rounds left under the line, inside the 2-degree trigger
  cone), drew no `D;` line, hid from each other by the futile-enemy rule, and
  the defender defused all three plants (ticks 7385 to 11596).

The graph over a breached base:

- The way into a breached base opens only when its walls go. The server builds
  the graph without the `rubble` brushes (`World::rubble`; out of the way
  except over a dead base) and rebuilds it on its own thread over the brushes
  the script has linked and unlinked once that count (`GameHost::brush_links`,
  bumped by `delete`, `notSolid` and `solid` on a brush model) holds still 2 s
  (`Server::relink_nav`). The cache keys on which submodels clip, so a restart
  that puts the walls back finds the old graph.
- The flood reaches 512 units past every bas entity as well as the spawns
  (`World::nav_cover`). Trap: mp_italy's western axis bunker (-4556 3760) lay
  outside the spawns' margin, the graph stopped at x -4603, its charge room
  had no node, and the four-bot shoot-off seed 11 looped on 47 defuses. The
  ignored census `every_bas_charge_room_is_on_the_graph`
  (`tests/nav_census.rs`) checks all thirty stock charges.
- Only bas entities widen the flood. Off the stock UO maps' entity lumps,
  the other gametypes' entities past the spawns' box and its margin are
  sd's `bombtrigger` `trigger_lookat`s (mp_rhinevalley's at -54237 21978,
  mp_sicily's -14244 11339, mp_foy's -11477 4450, among others) and
  mp_rhinevalley's `script_origin` `jesse1` (-7522 -8122 2299, gameobject
  `bombzone`); every `bombzone_A` and `bombzone_B` lies inside. VERIFIED.
  UO's `sd.gsc` turns the `bombtrigger` off at the start (2128-2129) and
  moves it onto the planted bomb (2244-2245), so where the map parks it is
  never walked to; no MP script names `jesse1` (VERIFIED, every `.gsc` in
  the stock UO paks). INFERRED: neither needs the graph, so `nav_cover`
  stays bas only.
- Engine rules this needed: a brush model's clip follows a bare `origin` write
  (the rubble's lift), and a respawned brush entity links its model again (a
  restart's walls).

Assets:

- VERIFIED (`pakuo00.pk3` `weapons/mp/bazooka_mp`, `panzerschreck_mp`,
  `panzerfaust_mp`): `weaponType projectile`, `weaponClass rocketlauncher`,
  `damage 1000`, `explosionRadius 256` with 100 inner and 10 outer, `clipSize
  1`; the bazooka and the panzerschreck `maxAmmo 3` and `reloadTime 6` with no
  `reloadEmptyTime` (their dry reload takes `reloadTime`, `cod11-combat.md`
  1.7), the panzerfaust one round. `scr_bas_basehealth` defaults to 24500,
  "700 tank shell * 35" (`bas.gsc` 351): 25 direct rockets a base.
- VERIFIED (mp_foy's entity lump): 32 `mpweapon_bazooka`, `_panzerschreck` and
  `_panzerfaust` lie about, most with `wait 15` (a few 25, some none); six are
  `script_gameobjectname bas` with `count 3`, one per base about 145 units
  from its origin beside its rocket crate, inside its bunker.
- INFERRED (ours, `GameHost::shot_models`): a rocket strikes a base's script
  model mesh before it reaches the origin, so the breach rides the direct
  `G_Damage` on a `takedamage` model (`cod11-combat.md`, the shell direct
  hits).

Measured: ours, release, UO install, `scr_bas_basehealth 3000` for the `bas`
gates in `tests/bots.rs` (`uo_bots_breach_bases_until_a_bas_round_ends`, two
bots, shoot off; `four_uo_bots_end_a_bas_round_without_shooting`;
`four_shooting_uo_bots_end_a_bas_round`).

- The other gametypes (2026-10-15, two bots, seed 7,
  `tests/bot_objectives.rs`): `ctf` on mp_cassino with shoot on, a flag taken
  at tick 1351 and captured 1342 ticks later (shoot off, both bots take a flag
  and wait in their goals for good); `dom` on mp_cassino, shoot off, the first
  flag at tick 843 and 4 more in the 60 s after; `hq` on mp_cassino with shoot
  on, a radio held at tick 1102 (shoot off, the two contest it for good). No
  script abort. VERIFIED (measured).
- Current numbers (2026-10-20, master a5c81855 against the weapon rules above,
  the four-bot shooting gate, all five maps, seeds 1 to 11 odd), VERIFIED
  (measured): all 60 runs end on their bases. After: mp_foy 10359 to 14239,
  mp_italy 9979 to 15059, mp_kursk 11339 to 16299, mp_ponyri 10379 to 16939,
  mp_rhinevalley 9879 to 14219, mean 12496 (before 12032). Every swap pickup
  logged (61 before, 56 after) dropped an empty weapon, and no plant or defuse
  settled on a cursor hint: the gun-for-gun trade and the swap at a charge did
  not come up, and rest on unit tests. What moved runs is the launcher list no
  longer fetching an empty launcher: on mp_rhinevalley seed 5 an allied bot
  with an empty bazooka had tapped another empty one three times, then an
  unreachable one at (1938, -2254, 2017) three times; without that it reached
  the fight earlier and the allies' third breach waited on the axis sniper's
  kills (8234 to 12953, before 8012). The old list alone restores master's
  ticks on mp_rhinevalley seeds 1 and 5 (10179, 10039).
- Shoot off and two bots (2026-10-18, master 7256c89c, seeds 1 to 11 odd, all
  five maps): every run ends on its bases, shoot off 6179 to 9559
  (mp_rhinevalley seed 7 slowest), two bots 9519 to 12539; no shoot-off run
  defuses a charge. VERIFIED (measured).
- Superseded rounds, one line each:
  - 2026-10-15: `bas` on mp_foy, two bots, shoot off, `scr_bas_basehealth 0`:
    a charge at tick 2020 on the axis hq, gone off 1199 ticks later. VERIFIED
    (measured).
  - 2026-10-17, first breach gates: two bots shoot off ended at 13539; four
    shoot off defused every plant after the first for 33 min; shooting over
    seeds 1 to 23 odd ended at 11059 to 29459. Before the reload fix a bot
    fired one rocket and stood on a launcher that never reloaded; before the
    shell direct hit a base took nothing. VERIFIED (measured).
  - 2026-10-17, before the defuse split: four bots shoot off, seeds 3 and 7,
    planted 97 and 113 times and defused nearly all for 29 min; after, seed 5
    still looped (the second bot called home 67 times); the shooting seed 7
    stood twenty minutes before the futile-enemy rule. VERIFIED (measured).
  - 2026-10-18, before the near-only defuse, the clock rule, the blast keys
    and the backing: shoot off looped on seeds 17 and 19 (64 to 68 defuses);
    after, shoot off 6459 to 7559 with no defuse, shoot on 8559 to 20059. The
    planter's guard alone (8 s) ended one seed in eight. VERIFIED (measured).
  - 2026-10-18, before the nav cover, the `delete` notify
    (cod11-gsc-object-model.md 14), the dry fetch and the rocket read:
    mp_italy shoot off seed 11 looped on 47 defuses, mp_foy shooting seed 11
    hit a `_spawnlogic::Player_WaitForDeath` script error; after, all 96 runs
    ended. VERIFIED (measured).
  - 2026-10-18, the gun fetch at 800: mp_foy shooting seeds 1 to 23 odd 9899
    to 17779, mean 12972 (master 13517), seed 23 14979; on master 35198659 two
    of 36 runs moved. VERIFIED (measured).
  - 2026-10-19, the LMG pitch rule against master 447d904f: 27 of 36 runs on
    the same tick, mean 12452 to 12176. The four slower runs, traced with the
    rule's clamp toggled to 90 degrees, part at the first round one run fires
    and the other does not, not by the rule. VERIFIED (measured).
  - 2026-10-19, the pit rule (bot-navigation.md section 3) on ee4c0d0b: 36
    runs end, mean 11590; the rule fired on none (a first form without the
    spawn test fired on mp_italy seed 5). VERIFIED (measured).
  - 2026-10-19, one generator per bot (section 8), submodel ladders and the
    closed breach cone, on c05581d6: 36 runs end, none with an own-rocket
    kill, mean 12143 (before the cone fix mp_kursk 11 ended at 19799 on one).
    VERIFIED (measured).
- Runs part on luck. INFERRED (code: `ClientSim::fire_recoil` takes
  `rand_from(&mut self.rng)` per round in `Server::replay_moves`): every round
  fired moves the shared spread and recoil draws after it, as on retail, so
  two runs part at the first round one fires and the other does not. Since
  2026-10-19 bot choices draw off each bot's own generator (section 8, "One
  generator per bot").
- mp_kursk seed 5's slow stretch (master 7256c89c, ticks 10303 to 15142; 12413
  to 18055 on 447d904f): both sides down to a breached last base, all four
  bots crossing toward each other's and fighting in the open between (2500,
  -4000) and (3700, -9700) (18 kills on 447d904f, none in a charge room); not
  a room held against attacker after attacker. VERIFIED (measured).

Rejected (ours, shooting gate):

- Regroup for a planter of the other side's breached last base (mp_kursk,
  seeds 1 to 11 odd): waiting 600 to 750 off the charge up to 30 s for a
  teammate farther out (slower; seed 5's planter waited by the axis spawn and
  died); the same only for a teammate 64 units nearer in 3 s (seed 1 10879 to
  18979); the same only after a life ending within 1000 of the charge (all 36
  runs on master's tick). VERIFIED (measured). INFERRED: a plant made at once,
  before the defender is back, wins more than it loses.
- Endgame rules, both sides down to one breached base (master 1080e30d, five
  maps, seeds 1 to 11 odd): the defender stays home (mp_kursk seed 7 never
  ended, 12 defuses in 29 min); it backs only a ticking charge (mp_kursk seed
  5 to 26199); a planter waits outside the blast while the other side has two
  alive and no teammate is near (mp_kursk seed 9 9839 to 23279). VERIFIED
  (measured).

## 8. Bot management

vcod's own extension, with nothing on retail to match beyond the paths a
bot joins and leaves by. `crate::botmgmt` holds the rules and the name
pool, `server/bot_admin.rs` the pass that runs at the top of the bots'
tick (every 500 ms, or the tick after an `addbot` or `removebot`) and
the commands. Both servers run that one copy over its `AdminHost`
hooks: CoD1's `Server` implements them there, `Cod2Server` in
`cod2/bots.rs`.

- Names. VERIFIED: 1.1d's `ClientUserinfoChanged` (game.mp.i386.so
  0x520f4) copies the userinfo `name` into the client's 32-byte buffer at
  `client+0x212c`, stopping once 31 bytes are written, and drops a `^`
  code whose low three bits are 0. Every pool name with `[BOT] ` and a
  two-digit suffix fits in 31 bytes and carries no `^`, `\`, `"`, `;` or
  `%` (the test `the_pool_holds_a_hundred_distinct_names_that_fit`). The
  draw has its own generator, so a name never moves the server's `rand()`
  draws the seeded bot gates replay.
- One generator per bot. Retail's game module and engine draw libc
  `rand()`, one process-wide sequence: the spread (`gunrandom` 1.1d
  `0x3d4ac`, combat doc 2.2), the melee roll (`rand() % 5`), 1.5's and
  UO's fire recoil (`BG_WeaponFireRecoil`, cod15-movement.md 6), a
  missile's tumble and the stuck push (VERIFIED, each in its doc). vcod
  keeps them on the server's one generator (`Server::rng`), in the
  order its tick takes them. The bots are vcod's own and have no retail
  draw: VERIFIED, `cod_lnxded` 1.1d, no call to `rand@plt` (0x804a5d8)
  in `SV_AddTestClient` (0x808753c..0x8087800) or `SV_DirectConnect`
  (0x8085498..0x8085eec), and a test client sends no usercmd
  (cod11-gsc-object-model.md 25). So each bot draws its brain's
  choices and its path's (`nav::Follower::waypoint`'s roams) off its
  own `Bot::rng`, and a bot's seed and challenge come off a stream of
  their own (`BotAdmin::seed_rng`, reseeded by `Server::test_seed_rng`):
  adding a bot moves no shared draw, and one bot's choice moves no
  other's (`server::bot_admin::tests::adding_a_bot_draws_nothing_off_the_shared_rand`).
  Until 2026-10-19 the path's draws, a bot's seed and its challenge
  came off `Server::rng`, so every roam pick by any bot moved every
  later spread and recoil draw.
- Joining. A bot connects as `--bots` always did: two draws off the
  bots' seed stream in that order, the first free slot from `sv_privateClients` up, and the
  team menu answered with its side. `--bots` deals allies, axis, allies in
  turn, which balance reproduces with no humans, so the seeded gates keep
  their games.
- Team moves. A move answers the team menu (`mr <id> <team menu> <team>`)
  on the bot's behalf and clears its answered menus, so the weapon menu the
  switch reopens is answered again. INFERRED from stock `tdm.gsc`'s menu
  handler (pak5.pk3 `maps/MP/gametypes/tdm.gsc` 303-307): a player
  answering his own team while playing is ignored, one changing team while
  playing suicides first. The planner only moves a bot whose
  `sessionstate` is `dead` or `spectator`, so no move costs a death.
- Counting. Sides come from `pers["team"]`, which the team menu sets at
  once (`sessionteam` waits for the spawn); a bot still joining counts on
  the side it asks for. A human counts toward `vc_bot_minplayers` once his
  `pers["team"]` is `allies` or `axis`. VERIFIED: stock `dm.gsc` sets
  `pers["team"]` to the team menu's answer too (pak5.pk3
  `maps/MP/gametypes/dm.gsc` 267).
- Leaving. A removed bot goes through `SV_DropClient` with
  `EXE_PLAYERKICKED`, so the game's `ClientDisconnect` and the drop notice
  run as for a kick, and the slot frees at once (a bot leaves no zombie).

## 9. Team play

vcod's own, with nothing on retail to measure against. `bots::team` holds
the rules; the server applies them once a frame with the whole team in
hand (`Server::bot_think`'s view pass) and puts the answers in each bot's
view.

- Per-round deal. The rank that deals out S&D sites and `re` objectives
  is the team shuffled by the level's `sv_serverid` (`team::round_rank`),
  which every `map_restart` moves on, so a bot that guarded A last round
  may guard B this one. Until 2026-10-20 it was the bot's place by slot,
  and the same bot went to the same site every round of a map.
- S&D attackers go together. Every attacker goes for one site, picked per
  round (`team::round_shift`, `team::site_rank`); the defenders still
  split over both. A failed plant moves the bot on to the other site as
  before.
- Plant cover. An attacker with a teammate held at a plant
  (`linkTo`, inside a zone) within 1500 units guards him from 96 to 250
  units off (`SdView::cover`) instead of making for its own site.
- `re` escort. Only the attacker nearest the carrier
  (`team::ESCORTS`, `ReView::escort`) keeps by him; the rest go ahead and
  hold the ring round the goal he makes for. The carrier himself keeps
  his path keys through a fight and only turns the view to shoot
  (`Bot::carrying`), as a `bas` bot clearing a charge's blast does: before,
  he went `Goal::To` whatever he saw and chased it off the way home.
- Route spreading. On an objective level (`sd`, `re`, UO's `ctf`, `dom`,
  `hq`, `bas`) a plan pays double length on an edge a teammate's path
  ahead still runs along (`nav::CROWD_COST`, `team::crowd`,
  `Follower::crowd`), so a way round up to twice as long is taken over
  the teammate's. Read when a plan starts; `dm`, `tdm` and `bel` plan as
  before, so the `dm` stall runs (`ship_stalls`) are unchanged.
- Noise answering. On any level with sides, `tdm` and `bel` included,
  only the two teammates nearest a noise go to it (`team::answers`);
  the rest of the side keeps its own way (`bot-combat.md` 7).

`team_tally` has since been folded into `bot_match` (`GAMETYPE=sd`,
`MAPS=`, `TEAMPLAY=` switching these rules per side in one build);
`bot-combat.md` 6.3 re-measures this table there.

VERIFIED (measured, ours, 2026-10-20, release, `--example team_tally`, 8
shooting bots, seeds 1-11 odd, 6000 ticks (300 s) per run, the ten 1.1 maps
that run both gametypes, totals over 60 runs each). Columns: plants,
defuses, pickups, captures, the attackers' round wins, the defenders',
kills. The machine was under heavy load; the runs are tick-driven and
seeded, so load does not move them.

| Build | sd plant | defuse | att W | def W | re pickup | capture | att W | def W |
|---|---|---|---|---|---|---|---|---|
| master (6f1b7fb2) | 217 | 80 | 138 | 154 | 234 | 9 | 164 | 198 |
| shuffle + cover + escort + crowd, attackers split | 186 | 75 | 117 | 160 | 221 | 9 | 184 | 175 |
| crowd alone | 215 | 87 | 124 | 149 | 232 | 10 | 176 | 183 |
| shuffle + cover + escort alone | 211 | 78 | 130 | 149 | 239 | 5 | 182 | 184 |
| all, attackers on one site (S&D) | 232 | 65 | 168 | 105 | | | | |
| all + carrier keeps running (`re`) | | | | | 231 | 18 | 164 | 189 |

The last two rows are the shipped build: the one-site rule touches no
`re` level and the carrier rule no `sd` one.

Spreading attackers over both sites lost S&D rounds: one or two bots at a
site met the defenders' half alone. Sent together they won 62% of the
rounds that ended (168 of 273) against 47% on master. In `re` the
captures doubled (9 to 18); pickups and round wins stay inside the run to
run spread. Captures are still rare: most carriers die on the way, and
the rounds end on eliminations (section 4.2).

### 9.1 Keeping apart (2026-11-11)

A playtest report: in `sd` a whole team moved as one, in a line, bumped
into each other and fell to one burst. Four rules on top of section 9,
on any level with a team objective (`driver::team_objective`), with team
play on, switched together by `Bot::squad` (`bot_match` `SQUAD=`):

- Route noise. Each bot plans with its own cost bias: every 384-unit
  square of the map (`nav::NOISE_CELL`) costs it up to 25% more or less
  (`nav::ROUTE_NOISE`, `Search::noisy`), drawn from a seed of the round,
  team and slot (`team::route_seed`). Two ways of about one length split
  the team; a way twice as long is never taken for it (`nav` test
  `route_noise_splits_plans_over_ways_of_a_length`). INFERRED: a draw
  per edge would average out over a path's many short edges, hence the
  squares. Only where the route pays the crowd cost (`BotWorld::YIELDS`:
  CoD1, 1.5, UO).
- Spacing. Out of a fight and a retreat, not carrying, with no bomb down,
  a bot on a plain run eases its move keys to half (`EASE_KEY`,
  `Bot::spaced`) while a teammate walking the same way (over 60 units/s,
  within 60 degrees of its heading) stands under 200 units ahead of it
  across and 96 up or down, within 45 degrees of its way
  (`team::mate_ahead`, `BotView::mate_ahead`). The keys never go to
  zero, so nothing stalls and no doorway holds.
- Staggered start. The first think alive of a one-life round draws a
  hold of 0 to 2 s (`team::START_HOLD`) off a copy of the bot's
  generator, so its other draws stay where they were (`Bot::start_draw`).
  The bot stands meanwhile; an enemy in sight ends the hold, and a
  carrier never holds.
- S&D attack parts. The round's attackers, ranked by `team::round_rank`,
  get a part (`team::squad`, dealt in `driver::deal_squads`): one
  flanker from three attackers and two from six, and from four up a rear
  guard. A flanker goes for the same site but pays `nav::FLANK_COST` (4,
  against `CROWD_COST`'s 1) on the main group's paths, so it takes a way
  round up to five times as long (where the route pays the crowd cost,
  as the noise), and holds 0.5 to 2 s first so the main group's paths
  are there to plan round. The rear guard holds 2 to 4 s
  (`team::REAR_HOLD`). Defenders keep section 9's site split and get
  the other three rules.

`dm`, `tdm` and `bel` have no team objective: VERIFIED (measured, ours,
`bot_match` `DIGEST=1`, 12 bots, `tdm`, mp_carentan and mp_harbor, seeds 1
and 2, 6000 ticks) their digests match master's bit for bit, and so does
an `sd` run with `SQUAD=none` (mp_carentan, 10 bots, seed 1, 3000 ticks).

Measured: `bot_match`, `GAMETYPE=sd`, 10 bots, the ten 1.1 maps that run
`sd`, seeds 1..=6, 12000 ticks (60 matches a row), retreat, cover and
team play on. The attackers are allies, axis on mp_pavlov. Mate and
packed are the mean over the ten maps of the attacking side's (or the
defending side's) `bot_match` columns. Each leave-one-out row is the
full set with one rule off; "parts off" puts every attacker in the main
group (a flanker still drew its hold from 0.5 to 2 s). VERIFIED
(measured, ours, `bot_match`, master 313e5a35 and `feat/bot-squad`):

| Run | att W | def W | att share | plants | defuses | kills | att mate | att packed | def mate | def packed |
|---|---|---|---|---|---|---|---|---|---|---|
| master | 297 | 259 | 53% | 501 | 214 | 4181 | 147 | 82.9% | 179 | 51.4% |
| all four rules | 283 | 251 | 53% | 470 | 205 | 3984 | 237 | 65.3% | 212 | 49.6% |
| spacing off | 285 | 244 | 54% | 475 | 213 | 4030 | 213 | 69.7% | 196 | 50.7% |
| noise off | 262 | 272 | 49% | 477 | 220 | 4048 | 208 | 70.6% | 196 | 49.9% |
| start hold off | 245 | 275 | 47% | 481 | 243 | 4031 | 228 | 67.3% | 216 | 47.6% |
| parts off | 240 | 277 | 46% | 456 | 225 | 3954 | 230 | 67.0% | 212 | 49.9% |

- VERIFIED (the table): the attackers' packed share falls from 83% to
  65% and their nearest teammate stands 237 units off against 147. It
  fell on every map, least on mp_pavlov (75% to 72%), most on
  mp_hurtgen (61% to 38%) and mp_carentan (93% to 71%).
- VERIFIED (the table): the attackers keep their 53% of rounds. Plants
  and kills per round barely move (0.90 and 7.5 on master, 0.88 and 7.5
  with the rules); there are 4% fewer rounds, INFERRED: the holds and
  the walk make a round longer.
- VERIFIED (the table): each rule left out raises the packed share
  (by 1.7 to 5.3 points) and, but for spacing, cuts the attackers' share
  of rounds by 4 to 7 points. INFERRED: with about 530 rounds a row one
  standard error on the share is about 2 points, about 3 on a difference
  of two rows, so only the parts' and the start hold's drops pass two of
  them. Spacing buys spread alone. None of the four is dropped.
- VERIFIED (the table): the defenders, already split over two sites,
  barely move (51% to 50% packed).

`re`, same maps, bots and length, seeds 1..=4 (40 matches a row); the
attackers are allies, axis on mp_dawnville and mp_railyard. VERIFIED
(measured, ours, `bot_match`, same builds):

| Run | att W | def W | att share | pickups | captures | kills | att mate | att packed | def mate | def packed |
|---|---|---|---|---|---|---|---|---|---|---|
| master | 317 | 116 | 73% | 454 | 83 | 3124 | 242 | 68.2% | 168 | 75.9% |
| all four rules | 296 | 134 | 69% | 444 | 92 | 3123 | 334 | 50.8% | 190 | 73.3% |

- VERIFIED (the table): the `re` attackers spread as the `sd` ones do
  (68% to 51% packed); captures and kills hold. INFERRED: their share of
  rounds, 4 points down on about 430 rounds, is inside the noise of a
  difference (about 3 points a standard error); a longer run would
  settle it.

## 10. United Offensive vehicles

vcod's own, like section 8: retail has no bots. A bot uses a vehicle
through the keys a client sends from a seat (coduo-vehicles.md 1 and 2),
nothing else. The brain is `crates/server/src/bots/vehicle.rs`; the
server half, the seat and the vehicles near each bot, and the driver's
route, is `crates/server/src/server/bot_vehicles.rs`. `vc_bot_vehicles 0`
stops boarding.

### 10.1 What the server tells the brain

- `BotView::vehicles`: each linked vehicle whose box stands within 900
  units across of the bot and that `seat::usable` passes for its slot
  (`G_IsVehicleUsable`: a free seat, under 100 units/s, alive, no grenade
  in hand, every rider a teammate), in pool order. Only for a bot on foot.
- `BotView::seat`: the seat, the hull's origin, yaw and forward speed,
  whether anyone drives, whether another bot walks up to board it, and the
  world yaw of the seat's gun: a tank's turret off the hull
  (`angles2[1]`) for its driver, the gunner's MG off the turret
  (`origin2[1]`) for seat 2, none for a four-wheel's driver or a
  passenger; that gun's round speed (`projectileSpeed` of a
  `weaponType projectile` file, the tanks' shells 10000; 0 for the MGs'
  bullets); and for the driver, whether a teammate on foot stands in the
  hull's way (`Crowd`, 10.3).
- `BotView::armor`: for a bot on foot with a loaded launcher, or one
  driving a tank, the nearest vehicle with a hostile rider (team rules as
  `bot_enemy`'s), its box's middle, velocity and riders, with a clear shot
  trace from the eye to the middle (or stopping within 24 units of the
  box: the hull's own clip). On foot only from 320 to 1500 units, past
  the launchers' 256-unit splash (`BREACH_CLEAR`, `BREACH_RANGE`); in a
  tank within the gun's 3000.

### 10.2 Boarding

- A bot with no enemy, no frag near, no plant or defuse under way and no
  throw in hand looks for one. With no objective (`dm`, `tdm`) any vehicle
  in the list will do; with an objective more than 1500 units off only one
  within 400; with an objective nearer it walks on. The nearest wins and
  the graph walks the bot to it (`Bot::goal`, ahead of the objective),
  to the clear node nearest it when the vehicle is parked over its own
  (bot-navigation.md, "Parked vehicles").
- Within 90 units of the hull the bot faces the box's centre, closes to 30
  and taps use on every other tick: the server boards on the press's edge
  (`item_pass`) and takes the first free seat from 1, so a second bot
  boarding a driven tank gets the gunner's seat.
- 20 s of walking or 2 s of pressing without a seat gives the vehicle up
  for 30 s; so does every exit.

### 10.3 Driving

- The driver's waypoint is not the walk's follower: infantry passes
  (leaps, flings, ladder holds) prove walks from where a body stands, and
  a hull is not a body. `drive_waypoint` runs A* over the graph's nodes a
  box of 0.75 of the vehicle's half width fits on (clear of the world and
  the vehicle clip, contents 0x201, from 18 over the node up 48), leaving
  out jump and leap edges and any edge rising more than a step; a goal out
  of reach routes to the node nearest it. The waypoint is the first route
  node 220 or more across from the hull; nodes within 110 are passed. A
  route goes again when its goal moves 300, after 5 s without closing on
  its next node, and every 15 s.
- A node's fit is a box position test (about 3.5 us on mp_foy), tested
  as the node comes off A*'s heap, not as it goes on, and cached by node
  point and box size. The cache outlives a graph rebuild: a rebuild
  follows a script brush link change, and only the fits whose box meets a
  toggled submodel's bounds (grown 2) are dropped. A warm pass fills it
  in node order for every live vehicle's box, at most 600 tests a tick
  (`Server::warm_hull_fits`), so a plan finds its tests run. A fit is the
  same whenever it is taken, so routes match a cold cache's: `bmuo_*`,
  `dguo*` and `it_bot_vehicles_uo` match the base recording
  (cod11-map-cycle.md 4.9).
- The goal is the brain's own (`Bot::goal`): an enemy, the objective or a
  roam, which picks a node a hull fits on at least 1200 away.
- `rightmove` steers both kinds toward the waypoint's bearing past 6
  degrees (positive lowers the yaw: `physics::targets`, the port of
  0x7d709, raises it for `rightmove < 0`);
  a tank more than 50 degrees off turns in place. A driver whose keys
  took the hull under 45 units in 1.5 s (30 units/s), or turned a tank
  in place under 15 degrees, backs off for 1.5 s with the steer held one
  way; four back-offs, or 8 s of driving, with no 300 units made get it
  out. The window counts ground made, not speed: a hull wedged on a wall
  bounces, and read off its speed it never stalled. Ground made counts
  from where a hold (a fight, a crew wait) left the hull: from the spot
  before a 5 s fight, seed 25's tank read its drive back across it as
  headway and dithered round (2540, -2600) past the 12 s gate.
- The hull's unbroken contact (`SeatView::contact_ms`, the stuck clock
  coduo-vehicles.md 12 "Self-damage" kills it on at 7 s) backs the
  driver off at 2.5 s and gets every rider out at 5 s: a tank sliding
  into a wall on a slope keeps the clock running with nobody at the
  keys, and the stuck kill's death walk kills its riders with the tank
  as attacker, which stock `tdm.gsc` 815 cannot score
  (cod11-gsc-language.md, "Runtime errors below developer 1").
- A tank's driver holds the hull still while it has an enemy or an enemy
  vehicle to shoot, and every driver waits up to 10 s for a teammate
  walking up to board.
- A teammate on foot (not seated, axis or allies only) is in the way when
  he stands, in the hull's frame, within 40 units wide of its sides and
  ahead of its front by 120 plus 0.6 s of its speed (or as far behind its
  back, reversing), or within the corner's radius plus 40 of the origin
  for a turn in place. The driver lets the keys go for him; held 2 s it
  backs off 1.5 s straight when the way back is clear, or rolls on ahead
  1.5 s when somebody stands behind and the way ahead is clear, and the
  back-off waits while anyone is in its way. A teammate milling round the
  hull clears it a tick at a time, so the hold's count runs down a tick
  per clear tick rather than starting over. A crush (`MOD_CRUSH_TANK`) of a teammate
  was every driver's in the measurement before this rule (10.6).

### 10.4 Guns and getting out

- The view goes to the enemy's chest, ahead along his velocity by the
  round's flight (`vehicle::lead`: a shell at 10000 units/s; the MG's
  bullets, and a passenger's own weapon, aim at the body); the server
  steps the turret or MG toward it (coduo-vehicles.md 2), and `buttons` 0x1 fires once the seat's
  gun yaw is within 4 degrees and nobody else's body is in the line. A crew
  mate in the same vehicle is left out of the friend check
  (`bot_friend_in_line`). A passenger fires his own weapon a tap at a time.
  An enemy no round goes at is given up on as on foot (`count_futile`).
- With no enemy in range, a tank's driver lays its gun on
  `BotView::armor` the same way (an enemy tank's driver is out of every
  round's reach, coduo-vehicles.md 13, so only the hull is a target).
- On foot, a bot with a loaded launcher and `BotView::armor`, and no
  other enemy nearer than the vehicle (an enemy riding in it is the
  vehicle), draws the launcher, stops its feet, turns at its skill's
  rate onto the hull's middle led by the rocket's 2800 units/s flight,
  and fires once the view rests within 2 degrees, released between shots
  and held while a teammate is in the line (`Bot::think_armor`). The
  launcher stays out for it (`off_launcher`).
- A seated bot with an enemy in range reads as engaged
  (`BotBody::engaged`), as a bot holding still to aim on foot does.
- Out: a route's end, 5 s in a standing vehicle with nobody to shoot (a
  rider) or with nothing to drive to (a driver), 90 s in the seat, four
  stalls or 8 s of driving under 300 units, the same 8 s for a rider
  with nobody to shoot, 2 s idle in a vehicle nobody drives, or 5 s of
  the hull's unbroken contact (10.3). Seed 45's gunner, without the
  rider's own headway window, sat out the driver's 8 s and then the
  undriven seat's 2 s and walked off past the 12 s gate. The bot taps
  use until the server lets it out, on one frame in four, its seat's
  (`level.time / 50 % 4 == seat - 1`, 10.7); a tank holds its driver for
  1.5 s after the entry (`VEH_UnlinkPlayer`).

### 10.5 The jeep self-destruct timer outlives no jeep

INFERRED (the stock script, `pakuo01.pk3` `maps/mp/_jeepdrive_gmi.gsc`
435-470): `process_deactivate`, run on each rider's exit, starts a 90 s
self-destruct once `riding_count` is 0, ended by `stop_self_destruct`
(a boarding, `delayed_process_activate`). A jeep destroyed in those 90 s,
or with a rider in it (the death's unlink is his exit, coduo-vehicles.md
11.2), is deleted while the timer waits. VERIFIED on UO 1.51b
(`client-probes/probe_jeepdeact`, `crates/server/tests/fixtures/vehicles/
mp_foy-jeep-deactivate-uo.txt`): the free ends the timer's thread, whose
`self` the jeep is, so its `dodamage` at 469 never runs and the console
shows no error (cod11-gsc-language.md, "Freeing an object ends every
thread whose `self` it is"). vcod raised a dead-entity script error there
until 2026-10-21; the `bas` gates in `tests/bots.rs` no longer leave one
out.

### 10.6 Measured

`cargo run --release -p vcod-server --example bot_vehicles [seed...]` with
`COD_DIR` at a UO install plays one game with `vc_bot_vehicles 0` and one with
1 per seed (six shooting bots, `tdm` on mp_cassino, 3 min; `MAP=`,
`GAMETYPE=`, `BOTS=`, `TICKS=` change that, `ON_ONLY=1` skips the vehicles-off
games, `VEH_LOG=1` prints the crush and rocket lines). It prints kills, kills
off a vehicle's guns (`*turret*`, `*gunner*`) and their `D;` lines, the crush
`D;` lines on the driver's own team (with damage) and the other's, rockets
fired on foot at vehicles, vehicles gone, boardings, bot-ticks seated and
units driven. `Server::test_seed_rng` keeps the seed's low bit set (`seed |
1`), so seeds 2k and 2k+1 play one game.

The runs are ours, release, on the stock UO paks.

Vehicles off against on (2026-10-20, the first vehicle round). VERIFIED
(measured):

| seed | vehicles | kills | off the guns | boardings | bot-ticks seated | units driven |
|---|---|---|---|---|---|---|
| 1 | off | 39 | 0 | 0 | 0 | 0 |
| 1 | on | 24 | 10 | 12 | 7713 | 19070 |
| 3 | off | 32 | 0 | 0 | 0 | 0 |
| 3 | on | 25 | 11 | 10 | 6326 | 18315 |
| 5 | off | 41 | 0 | 0 | 0 | 0 |
| 5 | on | 30 | 12 | 10 | 7300 | 20947 |
| 7 | off | 39 | 0 | 0 | 0 | 0 |
| 7 | on | 30 | 12 | 6 | 6925 | 21216 |

With vehicles on a third of the bots' time is seated (of 21600 bot-ticks a
game) and the tanks' guns take about 40 per cent of the kills, but a game has
about 30 per cent fewer: a tank's driver is out of every round's reach
(coduo-vehicles.md 13) and a tank is slow to lay its gun. INFERRED. The `bas`
gates on mp_foy (`tests/bots.rs`) end on their bases with 9 to 18 boardings.
VERIFIED (measured).

The crowd rule, the lead and the armor fights (2026-10-21, vehicles on, the
seven games of seeds 1 to 12). VERIFIED (measured):

| | kills | off the guns | gun hits | team crush hits | their damage | boardings | bot-ticks seated |
|---|---|---|---|---|---|---|---|
| before | 170 | 73 | 105 | 17 | 289 | 64 | 40313 |
| crowd rule only | 159 | 64 | 82 | 17 | 119 | 70 | 43579 |
| after | 167 | 58 | 71 | 13 | 74 | 67 | 45886 |

- Team crush damage falls by three quarters; what is left is mostly 1-point
  bumps of a teammate walking into a hull. Kills and gun kills move within the
  run-to-run spread (the crowd rule alone, guns untouched, gave 64 against
  73), and seed 4 carries a third of the gun kills either way. VERIFIED
  (measured).
- No lead scored better. Over the twelve seeds with the crowd rule and armor
  off, gun kills were 73 aimed at the body, 69 led by flight and the 100 ms
  enemy view lag, 50 by flight alone, 41 with a shell's 400 t² fall
  (coduo-vehicles.md 13) also lifted out, 47 with the flight lead aimed 40
  under the chest. VERIFIED (measured).
- A tank shell bursts with a 200-unit splash at 10000 units/s, so its lead is
  under 60 units at 3000; the MG is hitscan. VERIFIED
  (`weapons/mp/PanzerIV_turret_mp`: `explosionRadius 200`, `projectileSpeed
  10000`; `mg42_turret_mp`: `weaponType bullet`; UO paks). vcod keeps the
  flight lead, which a 2800 units/s rocket on a moving hull needs (a second's
  flight is 200 units at a jeep's 200 units/s), and leaves the fall and the
  lag out.
- No bot picks a launcher from the weapon menus, so in `tdm` no rocket goes at
  a vehicle. On mp_foy `bas` (`MAP=mp_foy GAMETYPE=bas BOTS=8 TICKS=6000`,
  seeds 1, 2, 4), where bots fetch launchers to breach, two rockets went at
  enemy vehicles in three games. VERIFIED (measured).
  `tests/bot_vehicles.rs`'s `a_bot_fires_rockets_at_an_enemy_tank`: a bot with
  a bazooka behind a tank an enemy bot drives fires 1 s after it is set down,
  and the tank burns out 11 s on.

Stuck drivers (2026-10-21, seeds 1 to 13 odd, 3 min each; `stuck_kills` is
`Server::test_stuck_kills`, `stuck_drivers` a driver neither fighting nor
waiting for crew whose seat stayed within 150 units of one spot for 12 s, the
`uo_bots_fight_and_sprint_in_tdm` rule). VERIFIED (measured):

| | stuck kills | stuck drivers | kills | boardings | bot-ticks seated |
|---|---|---|---|---|---|
| speed stall | 5 | 3 | 176 | 59 | 38251 |
| contact, ground-made and turn stalls, 10 s headway | 1 | 1 | 197 | 58 | 27905 |
| 10.3's rules, on master's later bots | 0 | 2 | 187 | 71 | 31542 |

The middle row's stuck kill is seed 5's Sherman sliding into a wall on the
slope at (2190, -2278, 40) after both riders got out. Riders give up a
struggling hull sooner, so fewer bot-ticks are seated. The last row's two 12 s
spells (seeds 9 and 11) are not traced. Each earlier stuck kill killed its
riders with the tank as attacker, the stock tdm 815 error
`uo_bots_fight_and_sprint_in_tdm` failed on; that gate plays with vehicles on
again. VERIFIED (measured).

That gate over odd seeds, vehicles on and off (2026-10-21). VERIFIED
(measured):

| | on, 1 to 49 | off, 1 to 49 | on, 51 to 63 | off, 51 to 63 |
|---|---|---|---|---|
| c8c3fd1f | 17, 19, 33, 45 fail | 1, 13, 19, 31, 33 fail | | |
| f47dafed | 29, 45 fail | none fail | 63 fails | none fail |

- On c8c3fd1f (superseded): 17 a ledge under a jump edge (bot-navigation.md
  "Jumps"), 13, 19 and 33 too few sprint legs (bot-navigation.md 4,
  "Pre-aim"), 45 the gunner's wait (10.4), 1 a plan across a wall
  (bot-navigation.md 3); 15 and 25 failed on the way on 10.7's lost exit and
  the stale headway anchor (10.3). INFERRED (the traces).
- On f47dafed: 29 a bot on mp_cassino's roofs (4850 to 4950, -1290 to -1440,
  256 to 303), a chain of 24-unit jump edges each waited on for UO's jump
  penalty, one jump bouncing off the step (seed 45 stalled there on c8c3fd1f
  too). 45 a tank driven back and forth over 350 units near (-150, -400), its
  ground made from the anchor reaching 300 each reversal, then the driver's
  walk in the same spot. 63 a bot's own frag. INFERRED (the stall lines); not
  traced further.
- `uo_tdm_games_that_stalled_or_never_sprinted` replays seeds 43, 17, 13, 19,
  25 and 33 with vehicles and 1 and 31 without.
- Superseded: with `bot-combat.md` 8's sight cone and target choice alone,
  seeds 1 and 19 pinned bot 0 on the roofs below from its spawn (4938, -1317, 256)
  for 12 s; with the grenade rules (`bot-combat.md` 6.5) and the noise
  answering (section 9, `bot-combat.md` 7) seed 1 passes and seed 13 is the
  game starved of sprint legs (331 bot-ticks on them, 330 sprinted). VERIFIED
  (measured).

The roofs, traced (`ship_stalls` with `UO=1 MAP=mp_cassino GAMETYPE=tdm
SHOOT=1 TICKS=1800`, per-tick dumps of the follower, edges from `nav_build`'s
`NEAR=` with `EDGES=1`). Each fault is a walk the graph proves that the bot's
own run does not reproduce. VERIFIED (measured, seed 17's bot 3 on 973e0e94):

- The jump edge from (4992, -1338, 295) to (4938, -1393, 303) crosses a trench
  at 256. The graph's walk (`as_a_bot`, with the walks' running start) lands
  it; from rest on the foot the same walk falls into the trench (both under
  UO's rules). A bot waiting out UO's jump penalty at rest
  (`Follower::jump_wait`) set off from rest, dropped in, and jumped at the
  roof's face 47 over it until the follower planned round (seed 17 on
  973e0e94, ticks 1544 to 1596). Now a bot held for a jump walks it again from
  the wait's spot at rest, once per edge (`Follower::rest_jump_fails`), and
  plans round one that does not land from there; an edge already avoided had
  no way round and is waited on as before.
- The dent at (4942 to 4954, -1404 to -1422, 280) stands 23 under the roof
  round it (box drops on a 6-unit grid), more than a step, and no 48-unit grid
  point falls in it, so it has no node. The walk from (4938, -1393, 303) to
  (4991, -1440, 303) crosses its corner, and the bot dropped in (tick 1702).
  No walk leaves without a jump, so `NavGraph::nearest_walkable` fell back to
  (4896, -1384, 280), behind the dent's west wall, and the bot pressed at it
  until the 12 s ran out (ticks 1708 to 1745, at (4938.8, -1410.0, 280.1)).
  Now, once no plain walk arrives, `NavGraph::walkable_start` tries a bot's
  walk from rest up to each of the nearest few nodes more than a step over the
  body, and a plan from one reached with a jump runs that leg as a jump edge
  (`Follower::jumping`), penalty wait included.
  `a_uo_bot_in_mp_cassinos_roof_dent_jumps_out` (`tests/bots.rs`) sets a bot
  in the dent at two spots: on 973e0e94 it held for all 15 s, now it is out of
  both.
- In `Follower::follow`, a landing after a tick off the ground and rungs, more
  than a step under both ends of a level non-jump edge, drops the plan, avoids
  the edge and plans again from `walkable_start`; every stuck replan and the
  unstick spell (`Follower::stuck`) start there too. Trap, VERIFIED (measured,
  seed 25 on 6b2ab6d1): a 23-unit fall into the dent, short of `FELL_BELOW`'s
  48, replanned from `NavGraph::nearest`, the node behind the wall, which
  `off_floor` passed as level; the bot pressed it (keys forward 115, between
  (4938.8, -1410.0) and (4957.1, -1405.5)) until the stuck timeout's replan
  jumped it out at tick 782. Now it lands at 671 and is out at 676.

After, over seeds 1 to 63 odd. VERIFIED (measured): on 973e0e94 seed 17 was
the only stall in the roofs' box (4800 to 5000, -1200 to -1500); on 2180b31d
with both fixes none stalls there, and `uo_bots_fight_and_sprint_in_tdm`
passes `SEED=17` and seeds 1, 19, 21, 25, 27, 33, 51. With the follower's
landing rule (6b2ab6d1 against its branch tip) stalled bot-ticks went 5789 to
5366, the stall at (4957, -1405, 280) and seed 17's on the stair at (1440,
-710) (bot-navigation.md "Jumps", "A goal that flickers") gone; one new stall
in the box (seed 13, bot 5 at (4859, -1258, 256)) is `engaged` throughout, a
fight the gates do not count, and the gate passes `SEED=25` and `SEED=17`.
Neither fix touches the graph build.

Rejected: a jump cue that skipped a tick whose closing pace still gained. A
build with the dent's fix alone stalled seed 59 in the box: a bot turning onto
the jump edge from (4899, -1296, 256) to (4896, -1337, 280) still carried its
190 units/s away from the top, which `Bot::think`'s cue reads as pinned (under
2 units a tick of closing pace), and jumped 17 short with no pace on (INFERRED
from the per-tick dump: the jump key on the second cmd after the waypoint
changed, at 93 then 8 units/s). The cue moved every game and raised the 32
games' stalled bot-ticks from 5917 to 7024, two of its stalls in the box, so
it was dropped. VERIFIED (measured).

Rockets by the muzzle (2026-10-21). INFERRED (reported on another branch's
draws for `four_shooting_uo_bots_end_a_bas_round`, mp_foy seed 7, not
reproduced here): a bot died of its own panzerschreck fired from
`Bot::think_armor` at an enemy vehicle about 1300 off. `Server::armor_view`
checked a clear line from the eye to the hull's middle and a teammate across
it, but the rocket leaves the muzzle along the view, led, and spread from the
hip.

- VERIFIED (`weapons/mp/panzerschreck_mp`, `bazooka_mp`, `panzerfaust_mp` in
  `pakuo04.pk3`): `explosionRadius 256` on all three; `hipSpreadStandMin` 7
  and `hipSpreadMax` 12 for the first two, 5.5 and 7 for the panzerfaust.
- `ArmorView::muzzle_clear` traces the view and eight rays round the cone at
  the next shot's spread (`combat::hip_spread_now`) out to the splash plus
  `ROCKET_CLEAR_MARGIN` (32), against the world, script models and hulls
  (`cone_clear`, the breach stand's sighted check made general). With it shut
  no rocket goes and the keys keep the walk.
- The other rocket paths: a breach (`Bot::breach`) fires sighted only from
  stands whose cone is clear to `BREACH_CLEAR` (320) (`breach_aim`);
  `Bot::trigger` fires no launcher at a player; 1.1's and 1.5's panzerfaust
  has no bot path that fires it (no vehicles, no `bas`).

### 10.7 Two exits on one frame

INFERRED (the stock script, `pakuo01.pk3` `maps/mp/_tankdrive_gmi.gsc`,
and the engine notify rule in cod11-gsc-language.md "Engine notifies and
the trigger queue"): `delayed_process_activate` starts one
`deactivated(activator)` thread per rider (line 280). Each waits for the
tank's `"deactivated"`, and one woken by another rider's exit loops back
to its `waittill` (lines 487-493). `VEH_UnlinkPlayer` raises that notify
inside the leaving client's cmd (coduo-vehicles.md 11.2), and an engine
notify only wakes its waiters; they run at the frame's
`Scr_RunCurrentThreads`. So when two riders leave on one frame, the
thread the first exit woke is not waiting when the second exit's notify
goes out, and the second rider's `process_deactivate` never runs: no
`stop_tank_hud`, `riding_count` not decremented, the seat's field
(`self.driver` or `self.gunner`) left set. The HUD itself goes, off the
player's own `vehicle_deactivated` (`tank_hud_destroy_thread`, line 653),
so the driver's `tank_hud_fireicon_run` lives on and, at the next driver's
shot, reads the destroyed fire icons (lines 856-869) and threads
`fadeoff` on them (827-829): VERIFIED (measured, ours,
`uo_bots_fight_and_sprint_in_tdm` `SEED=15`, 2026-10-21, with the
riders' headway window in and the seat's exit frames not yet) 75 `dead entity is not an object`
aborts from tick 1037, after a driver and a gunner left one tank on tick
938. Not measured on retail, which needs two clients' exits on one
frame. Bots now press use on frames of their own seat's, so none of
theirs coincide.

## 11. CoD2's `sd`, `ctf` and `hq`

CoD2 bots (`crates/server/src/bots/cod2.rs`) play the three with the
brain of sections 3 and 7.2. The server half reads the stock scripts
(`maps/mp/gametypes/*.gsc` in `iw_15.iwd`, line numbers below) in
`crates/server/src/game/cod2_objectives.rs` and deals the views as the
CoD1 driver does (rank, lead, cover; section 9).

### 11.1 What the scripts ask of a player

- VERIFIED (`sd.gsc` `bombzones`, 1433-1551; the mp_toujane ents lump):
  the sites are the `trigger_use_touch` entities with targetname
  `bombzone` whose `script_bombmode_original` key is set (the
  `level.bombmode` 0 default; `_single` for 1, `_dual` for 2), A and B by
  `script_label`. The defuse trigger `bombtrigger` is a
  `trigger_use_touch` too, sent away by `triggerOff` (1572) and moved
  onto the bomb at the plant (1754).
- INFERRED (`bombzone_think`, 1638-1700, branch conditions): a plant
  starts on the zone's `"trigger"` from an attacker on the ground, and
  runs while he stays alive with use held: the `"trigger"` is the use
  press's, the touch trigger's -256 score in the use list
  (`game/cod2_use.rs`), so the view does not matter. The script then
  `linkTo`s him to the zone (1697). The defuse (`bomb_think`, 1863-1917)
  is the same on `bombtrigger` with no `isLookingAt`.
- VERIFIED (`sd.gsc` 1768, 1943): a plant and a defuse log `A;...;bomb_plant`
  and `A;...;bomb_defuse`, what `bot_match` counts.
- VERIFIED (`ctf.gsc` `initFlags`, 1051-1083; the mp_toujane ents
  lump): each flag is a `trigger_radius` (radius 48, height 128),
  targetname `allied_flag` or `axis_flag`, carrying `team`, `atbase` and
  `home_origin`.
- INFERRED (`ctf.gsc` `flag`, 1095-1166, branch conditions): an enemy's
  touch takes the flag; its own team's touch returns it when not
  `atbase`, and, when `atbase`, captures the other flag if the toucher
  carries it (`.flag`). `pickupFlag` (1176) moves the trigger 10000 down
  while carried; `dropFlag` (1204) puts it where the carrier fell.
- VERIFIED (`hq.gsc` 1096-1128, 214, 226): the radios are `level.radio`,
  each with `team`, `hidden` and `radius` (`level.radioradius` 120), and
  `level.zradioradius` is 72, the fields United Offensive's `hq.gsc` has,
  so `ScriptRuntime::uo_objectives("hq")` reads them unchanged.

### 11.2 What the server tells the brain

- `sd`: `SdView` as in section 2, off the zones above; the standing
  point is `NavGraph::zone_stand`, the CoD1 `site_stand` rule.
- `ctf`: `UoView::Ctf` (section 7.2). A flag's `home` is its trigger
  while `atbase` and carried by nobody, `dropped` while not `atbase` and
  carried by nobody, and its `goal` the other flag's trigger at its
  `home_origin`: a carrier captures at his own flag, which must be home.
- `hq`: `UoView::Hq`, off `uo_objectives`.
- `linked`: `pm_type` 1. VERIFIED (measured, ours, mp_toujane `sd`, two
  bots shoot off, 2026-10-22): before this change vcod-server's CoD2
  level never drained the script's `linkTo` and `unlink` of a player
  (`GameHost::client_link_ops`), so the plant ran (the zone's `planting`
  read 1) while the bot, seeing no link after 20 ticks, let go. The level
  now sets `pm_type` 1 on a link and 0 on an unlink (`Cod2Level::
  apply_link_ops`); the body is not carried with its parent.

### 11.3 Measured

`bot_match` with `GAME=cod2`, docs/bots.md "CoD2": with six shooting bots
over seeds 1 and 2, mp_toujane `sd` planted 14 times and defused 6, and
`ctf` and `hq` scored where the bots without objectives scored nothing.
`bots::cod2::tests::cod2_bots_plant_and_defuse_on_mp_toujane` holds the
plant and the defuse with two bots that do not shoot.

### 11.4 The stall census over six seeds, 2026-10-27

The census of 12.12 on CoD2: `bot_match` (`GAME=cod2`, six shooting bots,
`TICKS=6000`, `STALLS=1`, default limits) on the 15 stock maps under `dm`,
`tdm`, `sd`, `ctf` and `hq`, seeds 1 to 6, 450 matches a setting. `bot_match`
counts a bot the script froze (`pm_flags` 0x8000) as held, as CoD4's does, and
with `STALLS=1` prints a stalled bot-tick's spot. Each stall spot in 11.4 to
11.8 was read off a per-tick trace of the bot's body, cmd and follower, and
each reading carries its label.

Noise: one setting's kills move by 50 a seed (standard deviation over the six
seeds), the per-seed difference between two settings by 31. VERIFIED (the
runs).

Current numbers, after 11.8. VERIFIED (the runs):

| gametype | kills | stalled bot-ticks |
|---|---|---|
| dm | 5572 | 213 |
| tdm | 5046 | 72 |
| sd | 1989 | 0 |
| ctf | 3252 | 318 |
| hq | 3472 | 216 |
| all | 19331 | 819 |

Totals by round, each VERIFIED (the runs): before 11.4, 19028 kills and 19843
stalled bot-ticks; after 11.4, 19220 and 4315; 11.5, 19340 and 1856; 11.6,
19389 and 1786; 11.7, 19242 and 1639; 11.8, 19331 and 819. No match is without
a kill.

The fixes of this section, all on CoD2's side of the bots:

- A mantle onto a ledge too low to stand on. CoD2's pmove raises
  `EV_STANCE_FORCE_CROUCH` through the climb (cod2-movement.md 9) and the
  client holds crouch after it. A bot now does too, standing once a standing
  box fits on the ground (`Cod2Bots::forced_crouch`). Before, an mp_breakout
  `sd` bot mantled from (5843, 4944, 12) and ended standing at (5809, 4944,
  96), off the ground with no velocity, for the round (2600 stalled bot-ticks,
  seed 2); now it crouches, drops to 68 and runs on. mp_dawnville's (1810,
  -16280, 62), a bot as still in the air, is likely the same. INFERRED (the
  traces).
- A ladder whose rungs let go only past their top. CoD2's ladder takes no flat
  move into the face (cod2-movement.md 11), so a climber leaves the rungs only
  above them. A CoD2 view (`BotView::tops_out`) climbs on to any head across
  the face; CoD1, UO and CoD4 keep the half-body rule (`Bot::steer` backs onto
  a waypoint below on the rungs unless it lies within half a body across).
  Before, on mp_matmata's ladder at (5025, 6749), rising to a roof at 169 with
  the graph (proved under 1.5's rules) stepping off to the floor at 130, 77
  across, the bot went up and down between 45 and 138 (1590 stalled bot-ticks
  in `dm`, more elsewhere). INFERRED (the traces).
- Mantle edges landing past the follower's off-path reach.
  `BotWorld::LONG_MANTLES` is on for CoD2, as for CoD4 (12.4). Before,
  mp_brecourt's mantle from (-528, -816, 1) to (-340, -813, 0), 192 across,
  had the follower clear the path at its foot every tick (1942 stalled
  bot-ticks in `hq`, more in `ctf`, `tdm`, `sd`). INFERRED (the traces).
  bot-combat.md 17.1 had kept it off because it grew mp_toujane's roof stall;
  on this code mp_toujane `hq` (1200, 1900, 70) reads 45 with it, 3017 before.
  The first two fixes alone gave 19204 kills and 10847 stalled bot-ticks,
  mp_brecourt and mp_decoy `hq` at 2195 and 2020, and one mp_harbor `tdm` seed
  rose from 31 to 1100 at (-6857, -7359, 37). VERIFIED (the runs).

No nav graph code changed in 11.4 to 11.7, so the `nav_build` digests stand:
the fixes are in the brain and the follower's driver. INFERRED (the diffs).

### 11.5 Teammates parted, 2026-10-28

CoD4's rules for a still teammate in the way (12.7, `BotWorld::PARTS_MATES`:
`mates_in_way` plans round him, `give_way` steps him aside) are on for CoD2;
`YIELDS` stays off. The stall it fixed: on mp_decoy `tdm` two bots of one side
at (8600 to 8690, -12540 to -12590, -530), 30 units apart, one waiting at a
jump edge's foot (node 9194, jump to 8202 at (8607, -12583, -492)), the other
running through him, the two swapping the foot (seed 6 788 stalled bot-ticks,
seed 1 527). INFERRED (the traces).

Before is 11.4's after. VERIFIED (the runs):

- Stalls 4315 → 1856, kills 19220 → 19340; per-seed kill difference (28, 51,
  23, 19, -3, 2) inside the noise. `dm` has no teammates and its lines are
  byte-identical.
- Down: mp_decoy `tdm` 1629 → 254, mp_decoy `hq` 441 → 0, mp_breakout `ctf`
  338 → 0, mp_brecourt `sd` 286 → 0, mp_toujane `tdm` 219 → 0, mp_trainstation
  `sd` 194 → 0.
- `YIELDS` on as well gave back most of the gain (all 4174, `hq` 2175), as
  bot-combat.md 15 found.

New or grown, mp_trainstation `hq` 221 → 442, mp_leningrad `ctf` 0 → 152,
mp_farmhouse `tdm` 0 → 98, mp_breakout `hq` 0 → 91, went to 11.6. INFERRED
(the stall lines). The diff is CoD2's `BotWorld` consts; CoD1, UO and CoD4 run
as before.

### 11.6 The radio's stand, a bot hung on a teammate, 2026-10-28

- The radio's capture. CoD2's is a 3D distance within `radio.radius` and a z
  difference within `level.zradioradius` (72); United Offensive's height is 50.
  VERIFIED (`hq.gsc` 214, 226, 1255, iw_15.iwd:
  `distance(players[i].origin, radio.origin) <= radio.radius`, the z
  difference `<= level.zradioradius`).
- A CoD2 radio carries `HqRadioView::height` 72 and a stand,
  `NavGraph::radio_stand`: the node of `zone_stand`'s component nearest the
  radio within 88 across and 56 up or down (`zone_stand` split into
  `stand_where`, its pick unchanged). The brain counts a bot inside 104 across
  and 56 up or down, which retail's check holds (118 at most). United
  Offensive keeps 50 and the radio's own point. Before, mp_trainstation's
  first radio at (7680.83, -3602.28, 18) (`maps/mp/mp_trainstation.gsc` 23),
  50 over the floor at -32, drew bots to its nearest node 6479 on a platform
  at (7577, -3549, 10), 116 across, where they paced 100 to 130 off it for
  minutes (442, seeds 1 to 3); now the stand is on the floor
  (`bots::cod2::tests::a_cod2_radio_stand_is_inside_the_capture`). INFERRED
  (the traces).
- A bot hung on a teammate. With CoD2's `BotWorld::PARTS_MATES_ALOFT`, a body
  off the ground and off a ladder beside a still teammate counts as held up;
  off for CoD1, UO and CoD4. Before, on mp_leningrad `ctf` (152, seed 1) an
  allied carrier waited in his capture zone at (-569, -626, 18), a teammate
  hung on the step below at (-598, -617, 14) with his capsule on the
  carrier's, off the ground nearly every tick, so `mates_in_way` (which asks
  the held-up bot to stand) never counted him, and a third queued behind; now
  he plans round and the carrier gives way. INFERRED (the traces).
- Not fixed: a bot pressing a mantle edge it never climbs until the follower
  goes round, mp_farmhouse `tdm` (98, seeds 4 and 5; 6541 → 6843 from (-2304,
  294, -29) to (-2208, 285, -26)) and mp_breakout `hq` (91, seed 5; from
  (5799, 4930, 40) down to (5856 to 5888, 4930, 10)). No teammate in either.
  INFERRED (the traces). 11.8 has where it went.

Before is 11.5's after. VERIFIED (the runs):

- Stalls 1856 → 1786, kills 19340 → 19389; per-seed kill difference -8, -6,
  33, -6, 14, 22. `dm` and `sd` byte-identical.
- Down: mp_trainstation `hq` 442 → 0, mp_leningrad `ctf` 152 → 0, mp_burgundy
  `hq` 120 → 0, mp_breakout `hq` 91 → 10, mp_decoy `tdm` 254 → 36, mp_toujane
  `ctf` 30 → 0.
- New: mp_brecourt `hq` 0 → 419 (seed 1, (-1444, -568, 28), no teammate within
  1400), mp_decoy `hq` 0 → 364 (seeds 1, 5, 6 at (8650, -12570, -530), off the
  ground, alone at 11.5's jump foot), mp_toujane `hq` 0 → 154 (seed 4, (1540,
  922, 71)), mp_trainstation `ctf` 47 → 83. None within 300 of a radio: the
  `hq` bots walk to other points and the matches run other routes. INFERRED
  (the stall lines against the radios).

### 11.7 A jump edge's top read off the path, the climbs 1.5 proved, 2026-10-29

- A jump edge at the edge of reach. With CoD2's `BotWorld::FOOT_SLACK` a
  waypoint up to `REACH` (24) past the build's off-path reach
  (`OFF_PATH_STEPS` grid steps, 4 times 48, 192;
  `NavGraph::drop_out_of_reach`) stays on the path (`Follower::foot_slack`);
  off for CoD1, UO and CoD4. Before, mp_brecourt's jump edge from (-1440,
  -570, 28) to (-1250, -576, 21), 190.4 across, put a body passing 3 short of
  the foot 193 off the top; the follower cleared the path every tick and held
  the bot for the rest of the match (`hq` 419, `tdm` 560). INFERRED (the
  traces).
- mp_decoy's pit (8650, -12570, -530), 35 to 55 under the ledges round it,
  left only by edges 30 to 39 up: 11.7 read it as CoD2's jump cap (the
  airborne step capped by `Jump_GetStepHeight` 0x80dc73a, a step over the top
  reverted by `Jump_IsPlayerAboveMax` 0x80dc7c8, cod2-movement.md 6). INFERRED
  (the traces), and wrong: 11.8 found the edges were leaps. mp_toujane `hq`
  (1552, 922, 72), a bot hanging on a lip under a node 18 up, went to 11.8.
  INFERRED (the traces).
- mp_farmhouse `tdm` (-2275, 300, -29): the bot on mantle edge 6541 → 6843 is
  pinned at x -2275.1, view yaw -8 to -18, and never gets the hint (mantle
  flags 0x5, no 0x8); `mantle_walk` over the bare world there climbs at yaw
  -13 and -20, not -8 or 0. INFERRED (the traces). 11.8 has the cause.
- Rejected: walking every jump edge and every edge rising more than 12 again
  under CoD2's pmove (`Cod2Walk`) after the build, dropping the ones no aim
  lands (75 to 128 a map). The traced spots went to 0 to 46, but mp_decoy
  `tdm` 36 → 683, mp_railyard `ctf` 0 → 322, mp_toujane `ctf` 0 → 303, and
  every graph changed, `dm`'s too; with the slack, 19344 kills and 1793
  stalled bot-ticks. VERIFIED (the runs).

The slack alone, before is 11.6's after, reproduced to the tick. VERIFIED (the
runs):

- Stalls 1786 → 1639, kills 19389 → 19242; per-seed kill difference -83, -40,
  -34, 14, -34, 30, inside the noise but for seed 1.
- Down: mp_brecourt `tdm` 560 → 0 and `hq` 419 → 0, mp_trainstation `ctf` 83 →
  0, mp_farmhouse `tdm` 98 → 55, mp_harbor `tdm` 33 → 0.
- Up: mp_decoy `hq` 364 → 535 and `tdm` 36 → 371 (the pit), mp_toujane `ctf` 0
  → 303 (seed 1 at (50, 910, -14), both arms), `hq` 154 → 185 (the lip),
  mp_trainstation `dm` 0 → 66. The slack changes every route, `dm`'s too.
  INFERRED (the stall lines).

### 11.8 The pit's leaps, the climbs' re-walk, the mantle strip, 2026-10-29

- A CoD2 graph is built without the lip (`bots::cod2::LIP`, as
  `bots::cod4::LIP`): leaps are cleared before `NavGraph::reprove_jumps` walks
  each again as a plain jump edge, edges past the follower's reach are
  dropped, and the mantle probe runs again off the floors the last mantles
  reached (`NavGraph::build_opts`). Every map loses its leaps (mp_decoy 162,
  mp_burgundy 346), gains 0 to 120 mantle edges (mp_farmhouse 785 → 905) and
  keeps every spawn in its component (`nav_build GAME=cod2`, `GAMETYPE=tdm`).
  VERIFIED (the builds). CoD1, 1.5 and UO build with the lip, so their
  `nav_build` digests stand.
- Why: the edges out of mp_decoy's pit (6408 → 9194, 6407 → 9193, 6407 → 9459
  on seed 2) were leaps (`NavGraph::link_leaps`), and the bot never pressed
  jump on one (buttons 0 all run at the ledge), falling back off the face.
  INFERRED (the traces). A bot jumps a leap only when `BotView::lip` says the
  lip is ahead, filled only on a route with pmove probes; CoD2's has
  `BotWorld::PMOVE_PROBES` off, so a CoD2 bot never jumps a leap (as 12.14
  found for CoD4).
- Why 11.7's re-walk moved the stalls: it walked each leap again as a leap
  (`Gait::Leap`), which lands, so the leaps stayed (built with the lip,
  mp_decoy `hq` 535 → 78 but `tdm` 371 → 683); and it walked only the climbs,
  pushing routes onto edges proved under 1.5's rules alone. On mp_railyard
  `tdm` and `ctf` (seed 6, 0 → 397 and 322) a bot paced (-1150, -462, 20): a
  CoD2 body run west from (-1120, -450, 14.5) along the plain edges to (-1216,
  -479, -9) stops at x -1150.9 on the level's collision, where 1.5's walk went
  on. VERIFIED (measured, ours, `vcod_cod2::pmove::player_step` over the bare
  world).
- Rejected, the graph stays proved under 1.5's rules: the climbs' re-walk
  without the lip (819 → 1585 stalls over the census), and the whole build
  under CoD2's step (`Cod2Walk` as the build's `WalkStep`: 1891,
  mp_trainstation `tdm` and `dm` 579 and 466). VERIFIED (the runs).
- mp_farmhouse's mantle strip (-2275, 300, -29), edge 6541 → 6843: no script
  brush or entity stands there (the bare world from the live playerstate gives
  the same flags, 0x5), and it is not the yaw. A step from rest at x -2275.1
  gets the hint only on y 262 to 286 of that wall, at every yaw from -40 to 40.
  The probe climbed at yaw -45 (`link_mantles` tries eight compass ways)
  from the foot (-2304, 294), sliding along the wall into that strip; a bot
  runs at the top (-2208, 285), meets the wall at y 307, slides south under 30
  units/s, slower as it turns square, and gives up at y 290, 4 short. VERIFIED
  (measured, ours).
- Rejected: running a rising mantle edge along its probe's heading from the
  foot (987 stalls against the no-lip graph's 819). Without the lip the routes
  no longer stop there: mp_farmhouse `tdm` 55 → 0, mp_breakout `hq` 14 → 0.
  VERIFIED (the runs).

The graph without the lip, before is 11.7's after, reproduced to the tick.
VERIFIED (the runs):

- Stalls 1639 → 819, kills 19242 → 19331 (the current table in 11.4); per-seed
  kill difference 41, 48, 33, -83, 20, 30 (standard deviation 45), inside the
  noise but for seed 4.
- Down: mp_decoy `hq` 535 → 0 and `tdm` 371 → 8, mp_farmhouse `tdm` 55 → 0,
  mp_leningrad `ctf` 39 → 0, mp_matmata `tdm` 27 → 0.
- Up: mp_trainstation `dm` 66 → 201 (seed 4, one bot at (6629, -2565, 128)),
  mp_brecourt `tdm` 0 → 64, mp_trainstation `hq` 0 → 31.

Left. INFERRED (the stall lines and a trace of seed 5):

- mp_toujane `ctf` 303, seed 1, two teammates 30 apart at (54, 911, -14).
- mp_toujane `hq` 185, the lip at (1470 to 1550, 925, 57 to 74): a jump edge
  from (1472, 932, 57) to (1470, 901, 82) the bot jumps and hangs on. The
  climbs' re-walk drops it (185 → 0 with the lip, 17 without) but is rejected
  above.

## 12. CoD4's `sd`, `sab`, `dom` and `koth`

CoD4 bots (`crates/server/src/bots/cod4.rs`) play the four with the
brain of sections 2, 3 and 7.2. The server half reads the stock scripts'
game objects (`maps/mp/gametypes/*.gsc` and `_gameobjects.gsc` from the
zones) in `crates/server/src/game/cod4/bot_feed.rs`
(`Cod4Level::objectives`) and deals the views as the CoD1 driver does
(rank, lead, cover; section 9). What the engine had to do for a player to
take, plant, defuse and capture at all is `cod4-gsc.md` 9.

### 12.1 What the scripts ask of a player

- VERIFIED (`sd.gsc` `bombs`): the bomb is a carry object on the
  `sd_bomb_pickup_trig` trigger, carried by the attackers only
  (`allowCarry("friendly")`); the zones are use objects on the
  `bombzone` triggers keyed to it (`setKeyObject(level.sdBomb)`), so only
  its carrier plants, with `briefcase_bomb_mp` as the use weapon and
  `level.plantTime` (5 s by default) as the use time. The
  plant moves each zone's `bombDefuseTrig` onto the bomb as the defuse
  object (`level.defuseTime`, 5 s by default, `briefcase_bomb_defuse_mp`) and
  disables the other zones.
- VERIFIED (`sab.gsc` `bombs`, `createBombZone`, `onPickup`, `onUse`):
  one neutral bomb (`sab_bomb_pickup_trig`, `allowCarry("any")`) and a
  zone per team, `level.bombZones[team]` its own, planted at by the
  other side; the plant turns the same zone into the defuse
  (`setUpForDefusing`), `level.bombPlantedBy` names the planters, and a
  defuse hands the bomb to the defuser.
- VERIFIED (`dom.gsc` `domFlags`): each flag is a proximity use object on
  a `trigger_radius` (`flag_primary`, `flag_secondary`), taken in 10 s
  (`setUseTime(10.0)`) by a team standing in it with nobody of the other
  team in it (`_gameobjects.gsc` `updateUseRate`).
- VERIFIED (`koth.gsc`, the radio loop): `level.radioObject` is the radio
  up now, a proximity use object on its `radiotrigger`; for
  `level.hqSpawnTime` after it is picked its trigger is disabled (sent
  50000 down by `updateTrigger`), then any team takes it by standing in
  it and the other side destroys it the same way.
- VERIFIED (`_gameobjects.gsc` `useObjectUseThink`, `useHoldThink`): a
  use object starts on its trigger's `"trigger"` from a player on the
  ground whose team may use it and who carries its key object, links him
  to the trigger, switches him to the use weapon and fills only while it
  is in hand and use stays held.
- VERIFIED (`_gameobjects.gsc` `createCarryObject`, `setPickedUp`,
  `setDropped`): a carry object's `carrier`, `curOrigin` (where it lies)
  and `interactTeam` are fields on its struct, and `updateTrigger` sends a
  trigger no team may use 50000 down.
- INFERRED, not yet confirmed on 1.7 (`ScrCmd_LogString` reads its
  string and writes nothing): `logString` logs nothing, and vcod's CoD4
  builtin is a no-op, so a CoD4 level logs no `A;` action; `bot_match`
  counts the objectives' changes off the scripts instead.

### 12.2 What the server tells the brain

- `sd`, `sab`: `SdView` as in section 2. A team attacks (its zones are
  the sites) while it carries the bomb or may take it lying, and defends
  the zones the other side plants at otherwise; once the bomb is down the
  planters guard it and the other side's bot nearest it is the lead that
  defuses. The bomb is a key object, `SdView::key` (`KeyBomb`): the
  team's playing bot nearest the bomb lying walks into it
  (`ObjTarget::Deliver`; the touch takes it), only the carrier plants
  (and keeps for its site through a fight, as a carrier does, section
  9), the other attackers guard their site. A plant and a defuse hold use
  for the script's use time plus 2 s, the briefcase's switch. CoD1's and
  CoD2's views carry no key and play as before.
- `dom`, `koth`: `UoView::Dom` (section 7.2), each flag or the radio a
  zone off its trigger's bounds (a round trigger's inner square). A radio
  not up yet reads as held, so the bots wait round it.
- A script's `switchToWeapon` reaches a bot as `a <weapon>` in 1.7's
  numbers; the bot asks for it in the cmd's weapon byte until it is in
  hand, past the brain's own weapon numbering (`bots::cod4::Arms`).

### 12.3 Measured

`bot_match` with `GAME=cod4` (docs/bots.md "CoD4"): six shooting
bots, `TICKS=6000` (5 min, the prematch and intermission included),
seeds 1 and 2 summed per map, 2026-10-25, the stock limits (`sd` and
`sab` a match of rounds, a round over at its end). Before is e6c7bf06
(no touch pass, no use key: nothing could be taken or planted), after is
this change; `bot_match` read the objectives' changes off the scripts
for the after arm only, so the before arm's zeros are by construction.
A run of this change with `OBJ=0` (bots off the objectives, the engine
fixes in) matches the before arm's kills, levels and stalls on every
`sd`, `sab` and `koth` run, and picked the bomb up 2 to 10 times a map
and took 1 to 4 `dom` flags in passing.

Before, every objective count was 0. After, summed over mp_backlot,
mp_crash and mp_strike (VERIFIED, measured): `sd` 33 pickups, 17 plants,
4 defuses, 7 explodes, 11 of 22 levels ended by the bomb; `sab` 45
pickups, 14 plants, 7 defuses, 7 explodes, 7 levels ended, all by the
bomb (none before); `dom` 29, 28 and 33 captures; `koth` 7, 1 and 8
captures. Stalls rose in `sd` (mp_crash 954 to 3571, mp_strike 0 to
4142), resolved in 12.4. 12.4's table carries the per-map counts forward.

- `sd`: every map plants and the bomb blows up or is defused; 6 of 10
  levels on mp_backlot, 3 of 6 on mp_crash and 2 of 6 on mp_strike end
  on the bomb rather than a team's last death. Rounds run longer (the 45 s bomb timer, the walk to
  the bomb), so fewer end in the 5 minutes on mp_crash and mp_strike.
- `sab`: each map ends at least two matches on an explosion; before,
  no `sab` level ended in 5 minutes.
- `dom`: 28 to 33 captures a map against none.
- `koth`: the radio is taken on every map but rarely on mp_crash, where
  both sides reach it and stand in it together, which holds the capture
  (`updateUseRate`) until one side is dead. Fewer bot-ticks play
  (mp_backlot 65 726 before, 50 443 after); not looked into.
- The stalls left in `sd` are defenders short of a zone's stand: on
  mp_strike seed 2 every stalled sample was two axis bots at
  (1573, 643, 16), 114 units outside zone B's box, out of reach of a
  stand inside it. Resolved in 12.4: they were dead bots following a
  teammate.
- `cod4::bots::tests::cod4_bots_take_plant_and_defuse_the_sd_bomb` and
  `_the_sab_bomb` (two bots that do not shoot, mp_crash),
  `cod4_bots_capture_dom_flags` (mp_crash) and
  `cod4_bots_capture_the_koth_radio` (one bot, mp_backlot) hold each
  objective on a live level.

### 12.4 Followers, mantles, the radio's floor and frags

What held the bots of 12.3 back, each read off a traced `bot_match` run
(`GAME=cod4`, seeds 1 and 2):

- The `sd` stalls were not bots. A spectator following another client
  is sent that client's playerstate whole, `pm_type` 0 and origin with
  it (`game::cod4::level`'s follow, cod4-hud.md 5.1), so a dead bot read
  as playing where the teammate it followed stood guard, was dealt a
  role and counted as stalled. Both mp_strike axis bots at (1573, 641,
  16) followed the defender guarding zone B. The feed and `bot_match`
  now count a client as playing only with `POF_PLAYER` (4) in its own
  `otherFlags`. INFERRED (the bots' playerstates at the stalled ticks).
- mp_crash's `sd` stalls were a mantle the follower dropped: the edge
  from (291, -1439, 84) lands at (385, -1307, 128), 162 across, past the
  follower's off-path reach of four grid steps, so the path was cleared
  every tick and the bot stood at the edge's foot heading for zone B. On
  CoD4 a mantle edge is now followed to its landing however far it lies
  (`BotWorld::LONG_MANTLES`; off on CoD2, whose `bot_match` digests it
  moved; 56 of
  mp_crash's 1222 mantle edges are longer than the reach, 12 of
  mp_backlot's 1498, 2 of mp_strike's 615). INFERRED (the follower's
  state at the stalled ticks; `nav_build`'s `ROUTE=` with `GAME=cod4`).
- On mp_crash `koth` nobody took the radio because nobody touched its
  trigger. The radio at (310.5, 1267, 303) has its `radiotrigger` (`*25`,
  z 271..393) over a stair whose nodes at 201..209 were the zone's
  stand; the brain read a body there as in the zone (its box top reaches
  the trigger's bottom), the stock script never counted one
  (`numTouching` stayed 0 for the whole match). The two teams never
  stood in it together, so there was no contest to fight out. A `dom`
  or `koth` site now starts a standing body's height (70) over the
  trigger's bottom: its stand and its test are feet on a floor inside the
  trigger. VERIFIED (`level.radioObject.numTouching` read each tick of
  the run); `_gameobjects.gsc` `triggerTouchThink` counts a body only
  while `isTouching`. The holders cannot respawn while they hold it
  (`koth.gsc`), so a koth level with the radio taken has fewer kills.
- No frag ever left a bot's hand on purpose: the feed read 1.7's
  `OFFHAND_HOLD` (17) as the pin out, but `PM_Weapon_OffHandHold` moves
  to `OFFHAND_START` (18) once the pin is out and the release goes
  through 17 (vcod_cod4's `pmove::weapon`, CoD2's names swapped). The
  brain waited for the pin and put the frag back.
- The kill rate. Six bots, 6000 ticks, seeds 1 and 2 summed, kills per
  playing bot-minute: CoD2 `tdm` on mp_toujane 2.59 (140 kills); CoD4
  `war` on mp_crash 1.27 before, 1.50 after. CoD4's bots fire more
  rounds (1286 against CoD2's 817) and land 0.23 of them, at 3.7 hits a
  kill (100 health against 30-40 a round), so the gap left is the
  weapons' damage, not the feed. CoD2's `bot_match` counts its hits off
  `D;` lines its scripts log only on some hits (133 hits for 140 kills),
  so the two hit counts do not compare. On both games the view follows
  the brain's aim to within 0.003 degrees (median), and every weapon of
  the five classes, the frag and the offhands, takes a brain number.
  INFERRED (the brain's `PONYRI_DBG` lines and the feed's weapon list,
  mp_crash seed 1).
- CoD4's `positionWouldTelefrag` now sees a body spawned this frame: 1.7
  writes `pm_type` in `ClientSpawn` and reads alive as below `PM_DEAD`
  (7). It moved no number here (the `sd` overlaps were followers).
  INFERRED (`GScr_positionWouldTelefrag`).

Before is 91bcd359, after is this change, `bot_match` as 12.3 (six
shooting bots, `TICKS=6000`, seeds 1 and 2 summed, 2026-10-25). The
after arm counts no follower as playing, so its playing bot-ticks drop
where the dead wait (`sd`, `koth`) and its stalls with them.

Per-map table (91bcd359 to this change) superseded by 12.5's; VERIFIED (measured) headlines: throws 0 to 2 a map before, 6 to 36 after; `sd` mp_crash 9267 stalled bot-ticks to 595; `koth` mp_crash captures 1 to 7.

- Frags fly on every map (0 to 2 a map before, 6 to 36 after).
- `sd` mp_crash: 9267 stalled bot-ticks to 595 and twice the kills.
- `koth` mp_crash: the radio taken 7 times against once; its holders
  wait dead, so kills fall from 86 to 15 and the playing bot-ticks from
  60 942 to 41 008. mp_crash's koth stalls (851) are attackers short of
  the radio room: bots wedged on a window sill, 12.5.
- `sab` mp_strike's suicides rose from 4 to 14: frags cooked off in the
  hand of a bot killed mid-throw, and the bomb, 12.5.
- `cod4::bots::tests::cod4_bots_capture_the_koth_radio_on_mp_crash`
  holds the radio's floor on a live level.

### 12.5 The sill, the stance events and the frag after a death

Read off traced `bot_match` runs (`GAME=cod4`, `STALLS=1` prints a
stalled sample's spot, `pm_flags`, ground and mantle state):

- The mp_crash `koth` stalls of 12.4 were bots wedged in a window sill
  of the radio room, at (-536, 1952, 290) for the rest of the match. The
  sill (brushes x -528..-520, y 1924..2052, top 290, a lintel over it) is
  a `MANTLE_OVER` east to west, which the graph has as the mantle edge
  (-504, 1919, 251) to (-544, 1953, 254). A bot came down from the climb
  in the air 8 past the landing, turned back for the node, and its feed
  pressed the jump key on the hint the sill's west face offered: a climb
  onto the 8-unit top with `flag::CROUCH`, the standing box not fitting
  under the lintel. The bot kept standing and ended all in solid, in the
  air with no velocity. INFERRED (the follower's path and the bot's
  playerstate each tick); VERIFIED that a standing body at the spot
  starts all solid and a crouched one 0.125 up does not, and that 1.7's
  pmove ends the climb from the west on the sill's top (the clip map
  traced and stepped through `vcod_cod4::pmove`).
- So the feed climbs a mantle edge once: the jump key goes on the hint
  only until `PMF_MANTLE` was seen on that edge
  (`bots::cod4`'s `climbed`). INFERRED.
- And a bot honours `EV_STANCE_FORCE_CROUCH` (7) in its own ring as the
  client game does a player's (`CL_SetStance(CL_STANCE_CROUCH)`,
  `CG_EntityEvent`): it holds crouch until a standing body fits
  where it stands on the ground. Stepped through 1.7's pmove from (-566,
  1952, 254) at the sill, a body that crouches on the event walks off the
  top and down the far side, one that does not stays wedged. VERIFIED (the
  clip map stepped); that 1.7's client game does the same is INFERRED.
- The `sab` suicides of 12.4 were frags cooked off in the hand, each by
  one bot every 268 ticks: a bot killed mid-throw read its frag as the
  weapon in hand (the feed reports the frag in hand while its key is held,
  `offhand_cmd`), which became the brain's armed weapon, so the next life
  asked for the frag from its first cmd, held its key with the pin out
  and never pressed the trigger, the brain's throw having ended with the
  death, until `EV_GRENADE_SUICIDE`. The feed now reports the frag in
  hand only while the brain is in its throw (`Bot::in_throw`), and a
  body out of play presses no frag key. Over `sab` on the three maps,
  seeds 1 to 8: 23 `MOD_SUICIDE` frag deaths before, none after; frag
  splash on the thrower 3 before, 6 after; throws 222 before, 265 after.
  INFERRED (each bot's cmd and playerstate per tick).
- The other `sab` "suicides" are the bomb: `sab.gsc` passes
  `level.bombowner` as the blast's attacker, which no stock script sets,
  so its `radiusDamage` kills with the world as attacker
  (`MOD_EXPLOSIVE`). VERIFIED (the script); retail's as ours.

Before is d9584100, after is this change; `bot_match` as 12.3 (six
shooting bots, `TICKS=6000`, seeds 1 and 2 summed, 2026-10-25):

Per-map table (d9584100 to this change) superseded by 12.6's; VERIFIED (measured) headlines: `war` mp_crash 2577 stalled bot-ticks to 57, `koth` mp_crash 3105 to 569, `sab` mp_crash 715 to 0, `war` mp_strike 820 to 0; `sab` suicides 15 to 4 over the three maps.

- `war` mp_crash 2577 stalled bot-ticks to 57, `koth` mp_crash 3105 to
  569, `sab` mp_crash 715 to 0, `war` mp_strike 820 to 0 (mp_crash's
  were the sill; mp_strike's were not traced). What stalls on mp_crash now is a bot pacing a few
  steps round its spot: a defender at (745, 965, 174) in `sd`, which
  climbs to a ledge and drops off it again, and the bots waiting
  round the first radio before it is up in `koth`. Not looked into.
- `sab`: no frag goes off in a hand; mp_strike's kills 63 to 85.
- The 3.7 rounds that hit a kill of 12.4 are not the weapons' damage:
  ours deals the stock files' numbers, 30 to 56 a round against 100
  health, and the kill gate matches retail's health after every round;
  the rounds that hit a body that lives are spent, as health comes back
  5 s after the last hit (cod4-combat.md 4). VERIFIED (the weapon files,
  the scripts, `cod4_combat_ab`).

### 12.6 Mantles a bot can climb, contested zones, frozen bodies

Read off traced `bot_match` runs (`GAME=cod4`, `STALLS=1`) on 71a62e20:

- The `sd` stall on mp_crash (12.5's defender "pacing" at (745, 965,
  174)) is attackers at the foot of a mantle edge, (672, 964, 174) to
  (800, 964, 139) and its neighbours off (704, 964) and (736, 967). The
  graph's probe found the climb facing 315: its walk bounced along a
  stair and met the wall at y 959, where `Mantle_Check` offers it. A bot
  runs straight at the landing, facing 0 to 2 degrees, and at y 964 to
  965 the hint never came: it ran at the wall, fell back off the stair
  and tried again until the round ended. VERIFIED (1.7's pmove stepped
  over the clip map from those spots: facing 315 off (676, 964, 174)
  climbs, facing the landing does not). The probe now keeps a climb only
  when the walk again facing its landing, as a bot follows the edge,
  climbs it and lands within a grid cell of it (`bots::cod4`'s
  `climbs_facing`): 1222 mantle edges to 1079 on mp_crash, 1498 to 1347
  on mp_backlot, 615 to 508 on mp_strike, every spawn still in one
  component. A stricter test (the walk from 4 units either side too)
  left 862, 1053 and 400 and moved the jams elsewhere: `sd` mp_crash 486
  stalled bot-ticks against this one's 25, attackers queued in a door
  behind teammates standing guard. INFERRED (that run's trace).
- With the mantle change alone `koth` stalled 1001 bot-ticks on
  mp_backlot (5 before) and 961 on mp_crash (630): on mp_backlot two
  axis bots at (534, -470, 204) in the door of the radio's L-shaped room (`radiotrigger` `*14`, two
  brushes), behind an axis teammate standing in one leg of it while an
  allies bot stood in the other. Both teams touched the trigger, so
  `_gameobjects.gsc` `updateUseRate` held the capture at 0, and neither
  bot saw the other round the corner, for the rest of the match. A
  player in the zone sees his capture bar stop. The feed now reads the
  game object's `numTouching` for both teams (`Zone::contested`), and a
  bot in a contested zone walks to the zone box's far side and back
  until the contest ends (`Bot::sweep_contested`; `DomFlagView::contested`,
  never set on United Offensive). VERIFIED (`koth.gsc`,
  `_gameobjects.gsc` `updateUseRate`, the trigger's brushes off the clip
  map); the stand-off INFERRED (each bot's view and the others' bodies
  per tick).
- `bot_match` counted a body `freezeControls` holds (`pm_flags`
  `FROZEN`, 0x800: an `sd` or `sab` round's end) as stalled. It counts
  it as held now: 145 of mp_crash's 946 `sd` stalled bot-ticks, all of
  mp_strike's 67 and `sab`'s 44 and 122. VERIFIED (the stalled samples'
  `pm_flags`).
- `koth` raised no script error on any level: the bots' 3 s wait before
  their first spawn holds (cod4-gsc.md 8). VERIFIED (the runs below).

Before is 71a62e20 with the `FROZEN` count (the `sd` and `sab` rows) or
without it (the others, where nothing is frozen in play), after is this
change; six shooting bots, `TICKS=6000`, seeds 1 and 2 summed,
2026-10-26:

Per-map table (71a62e20 to this change, `war`, `dm`, `sd`, `sab`, `dom`, `koth` on mp_backlot, mp_crash, mp_strike) superseded by 12.7's; VERIFIED (measured) headlines: `sd` mp_crash 801 stalled bot-ticks to 25, `koth` mp_crash 630 to 0, mp_strike 227 to 2, mp_backlot 5 to 74; mantles climbed fell on most maps (mp_crash `war` 138 to 79).

- `war` kills 229 to 260 over seeds 1 and 2, 268 to 269 over 3 and 4;
  `dm` 332 to 317, 316 to 301 and, over seeds 5 to 8, 647 to 641: 2.8%
  fewer `dm` kills over eight seeds, within the spread between seeds
  of one arm (mp_crash seed 1 against seed 2 differs by 20%).
- What stalls is left: `koth` mp_backlot's 74 and `sd` mp_crash's 25
  are bots behind teammates in a door, which the follower's crowd cost
  does not part. Not looked into. The contested walk moved no `dom`
  number: no run had both teams in a flag out of each other's sight.

### 12.7 Teammates held up behind a still one, the mounted guns

Read off traced `bot_match` runs (`GAME=cod4`, `STALLS=1`, whose stall
lines now name the nearest teammate, its distance and speed) on
b7630cbb, six shooting bots, `TICKS=6000`, seeds 1 and 2:

- What stalled was not where 12.6 put it. `sd` mp_crash's 25 is an
  allies bot mantling at (760, 894, 232) with its nearest teammate 697
  units off. `koth` mp_backlot's 74 is an axis bot upstairs at (-1129 to
  -1041, -564, 258) walking round two still teammates 32 to 42 units
  off, `koth` mp_strike's 31 one beside a teammate 159 off. INFERRED
  (the stall lines).
- With a bot that plans round a still teammate it is held up by
  (below), `koth` mp_crash seed 2 stalled 310 bot-ticks: two axis bots
  stood still in the radio room's door at (1653, -1762, 226) and (1683,
  -1758, 226), inside the zone, and a third paced between the door and
  the stairs down at (1650, -1900, 202) for the rest of the match. No way
  round the two reached the zone, so the plan went through them again.
  INFERRED (the stall lines, each bot's goal per tick).
- So two rules, CoD4 only (`BotWorld::PARTS_MATES`): a bot fighting
  nobody that has been slower than 60 units/s for 20 ticks with a still
  teammate (under 10 units/s) within 48 units toward its path's next
  node plans round the nodes still teammates stand on for the next 200
  ticks, or to a clear node beside its destination when one stands on
  it (`bots::driver`'s `mates_in_way`, through
  `Follower::blocked`); let go at once, the bot walked back into him
  and paced. And a still bot whose node a held-up teammate within 96
  units plans round steps 64 units away from him, its post or zone
  spot notwithstanding (`give_way`, `BotView::give_way`). VERIFIED (the
  runs below).
- `bot_match` before is b7630cbb for the objective gametypes and b7630cbb with the walk to
  a gun on another floor (docs/bots.md, "CoD4") for `war` and `dm`;
  after is both rules:

Per-map table (b7630cbb to both rules) superseded by 12.8's; VERIFIED (measured) headlines: `koth` mp_backlot 74 stalled bot-ticks to 0, mp_strike 31 to 0; `war` mp_crash 0 to 52; `dm` unchanged.

- `dm` does not move (no teammates). The first rule fired 14 times over
  three `war` mp_crash seeds, yet any firing sends a match down another
  course: over `war` mp_crash seeds 1-8 kills went 364 to 341 with it.
  `war` mp_crash's new 52 is an allies bot climbing at (-91, -897, 141
  to 233), its nearest teammate 750 off; `sd` mp_crash's 25 is still
  the mantle. `koth` lost kills on mp_backlot and mp_strike (78 to 52
  over both seeds) with the captures unchanged: not looked into.
  INFERRED (the stall lines).
- mp_backlot's two SAWs: docs/bots.md ("CoD4") has the cause and the
  walk over the graph to a gun on another floor, with its numbers.

### 12.8 A mantle's landing, the shipment yard's stand

Read off traced `bot_match` runs (`GAME=cod4`, `STALLS=1`, the
follower's path per tick) on 8e1d545d:

- `sd` mp_crash's stall at (745, 964, 174) (12.5 to 12.7) is the mantle
  edge from (704.1, 964.3, 173.9) to (800.0, 964.4, 139.0) and its
  neighbour off (736.1, 967.0, 174.2). The graph's probe climbs off the
  first facing 315 and lands at (787.6, 953.1, 140.3), in the column of
  the node at (800, 964); 12.6's check ran the walk again facing that
  landing, which climbs. A bot follows the edge at its node, and run at
  (800, 964) from 96 spots between (690, 950) and (745, 985), 5 are
  offered the climb; run at the landing, 52 are, every one north of y
  964, where the bots come from. VERIFIED (1.7's pmove stepped over the
  clip map, `a_mantle_edge_keeps_the_landing_its_probe_climbed_to`). The
  graph now keeps each mantle edge's landing (`NavGraph::mantle_lands`)
  and a CoD4 bot on the edge runs at it until it stands within 24 units
  of it, then on to the node (`BotWorld::MANTLE_AIMS`). INFERRED (the
  runs below).
- mp_shipment's `koth` and `dom` stalls are axis and allies bots pressed
  against the yard's south wall at y -617. Their goal was the radio's
  stand, (2.8, -672, 192), behind that wall. mp_shipment's graph is two
  floods that never meet: the yard the spawns stand in (1771 nodes,
  x -733..709) and, larger, the out-of-bounds grounds round it (3048
  nodes, x -1244..1187), flooded from a ladder's ends at (62, 927, 192)
  to 320 with no edge either way. `NavGraph::zone_stand` took its stand
  from the largest component, the grounds, so every zone's stand fell
  outside the yard, at the node nearest its middle. VERIFIED (the
  graph). A zone's stand, and the roams' safe component, now come from
  the largest component a spawn's seed lies in (`NavGraph::seeds`); on
  a graph whose largest component holds a seed nothing moves.
  VERIFIED (`a_zone_stand_comes_from_the_floor_the_spawns_flood`, and
  the mp_crash and mp_strike rows below, the same with and without it).

Six shooting bots, `TICKS=6000`, default limits, seeds 1 and 2 summed,
2026-10-27; before is 8e1d545d, after is both changes:

Per-map table (8e1d545d to both changes, 15 pairs) superseded by 12.9's census; VERIFIED (measured) headlines: `koth` mp_shipment 3177 stalled bot-ticks to 0 (kills 169 to 282), `dom` mp_shipment 3219 to 0 (captures 23 to 70), `sd` mp_crash 25 to 0.

- 1401 kills to 1596, 6476 stalled bot-ticks to 138. mp_shipment's
  `koth` and `dom` carry most of both: the bots now meet in the yard's
  middle. `war` mp_shipment does not move (no zone, no mantle).
- With the short limits the map sweep runs (`scr_sd_timelimit 0.5`, one
  round, `sv_mapRotation` the one level), `sd` mp_crash seeds 1-4
  stalled 862 bot-ticks before the landing aim, 0 after; kills 18 to 20.
- What stalls on mp_crash now (60 in `sab`, 78 in `koth`) are single
  samples at other spots: an axis bot beside a still teammate at (-138,
  1457, 232), climbs at (-79, -914, 173), (521, 1388, 181) and (1541,
  -1951, 114). Not looked into. INFERRED (the stall lines).

### 12.9 The stall census, a curb's border, jumps CoD4 does not make

A census: `bot_match` (`GAME=cod4`, six shooting bots, `TICKS=6000`,
seeds 1 and 2, default limits, `STALLS=1`) on all 21 stock maps under
`war`, `sd`, `koth`, `sab` and `dom`, 210 pairs, on 8a947914 and on this
change, 2026-10-27. VERIFIED (the runs):

| gametype | map | kills | stalled bot-ticks |
|---|---|---|---|
| sab | mp_carentan | 63 → 80 | 4073 → 0 |
| koth | mp_killhouse | 123 → 123 | 1245 → 1245 |
| sab | mp_cargoship | 54 → 54 | 1052 → 1052 |
| sab | mp_creek | 65 → 56 | 858 → 0 |
| koth | mp_cargoship | 74 → 74 | 725 → 725 |
| koth | mp_crash | 80 → 74 | 660 → 140 |
| koth | mp_crossfire | 71 → 71 | 301 → 301 |
| dom | mp_broadcast | 42 → 42 | 264 → 264 |
| war | mp_cargoship | 71 → 71 | 178 → 178 |
| war | mp_creek | 57 → 74 | 125 → 0 |
| dom | mp_showdown | 139 → 154 | 80 → 0 |
| sab | mp_farm | 62 → 56 | 73 → 0 |
| war | mp_bloc | 88 → 85 | 0 → 166 |
| koth | mp_crash_snow | 80 → 80 | 31 → 129 |

All 210 pairs: 8718 kills to 8674, 10120 stalled bot-ticks to 4606.
Every other pair moved by under 50 stalled bot-ticks, most by none (a
map whose bots never meet the new arm replays to the tick).

- `sab` mp_carentan's 4073 was one allies bot for the rest of a match
  at (881, 1857, -48), in the air with no velocity: it had walked south
  along x 881 into the terrain, whose 8-unit curb from (871, 1886) to
  (912, 1872) rises to -39.75, and ended 8 units inside it, all solid.
  The capsule sweep from (881, 1913) to (881, 1857) passed through: the
  curb's triangles face away or along the sweep, and its partition's
  border was met only where its top lay within the capsule's straight
  part, while this one lies 26 under the centre, beside the lower
  sphere. VERIFIED (the clip map traced). 1.7 meets such a border with
  the end sphere (cod4-collision.md 2, "1.7's capsule border"), and the
  CoD4 sweep now does (`Cx::border_sphere`). `sab` mp_creek's and
  `koth` mp_crash's stalls were the same, bots wedged in terrain at
  (540, 4554, -41) and (62, -640, 313). INFERRED (their stall lines
  gone with the change alone).
- What is left: mp_killhouse `koth`'s 1245 is an axis bot jumping again
  and again from a crate top at (1280, 2532, 60) for the jump edges up
  to (1310, 2557, 97) and (1310, 2586, 97), 37 units; mp_cargoship's
  is allies bots at (1346, 36, 52) jumping for (1313, 36, 80), 28 up,
  rising to 91 and falling back to the foot. The graph's walks proved
  them under CoD1's pmove (`GRAPH_RULES`); its CoD4 body does not make
  them as the bot takes them. INFERRED (the follower's path per tick).
  Pruning every jump edge up more than a step that a CoD4 body could
  not land (a probe like the mantle's, run as the bot jumps: pinned two
  ticks or right under the top) cleared mp_killhouse but pruned 875 of
  mp_cargoship's 1179 such edges and left its `sd` bots never meeting
  (32 kills to 0); a lenient probe (any jump tick) kept the bad ones.
  Not taken. A CoD4 walk step for the graph (`nav::WalkStep`, as
  CoD2's `Cod2Walk`) is the root fix. INFERRED (those runs).

### 12.10 The graph walked under CoD4's pmove

`bots::cod4::Cod4Walk` is a `nav::WalkStep` as CoD2's `Cod2Walk` is:
each walk's body is a 1.7 playerstate stepped by
`vcod_cod4::pmove::player_step` over the level's clip map, its cmd
through `to_cmd4` (weapon 0, so the graph's 190 units/s), read back into
the CoD1 body the walk tests. The graph build and the bots' pass
(`NavGraph::moving`) both take it with `VC_NAV_COD4=1`; it is off by
default. Graphs move by under 5% of their edges and every map's spawns
stay in one component. VERIFIED (`nav_build GAME=cod4` on the 21 maps,
both arms).

The census of 12.9 (210 pairs, seeds 1 and 2, `TICKS=6000`), CoD1's
walks (12.9's after) against CoD4's, 2026-10-27. VERIFIED (the runs):

| gametype | map | kills | stalled bot-ticks |
|---|---|---|---|
| koth | mp_killhouse | 123 → 142 | 1245 → 1752 |
| sab | mp_cargoship | 54 → 49 | 1052 → 158 |
| koth | mp_cargoship | 74 → 67 | 725 → 250 |
| koth | mp_crossfire | 71 → 73 | 301 → 9 |
| dom | mp_broadcast | 42 → 30 | 264 → 465 |
| war | mp_bloc | 85 → 74 | 166 → 0 |
| koth | mp_crash_snow | 80 → 97 | 129 → 341 |
| sd | mp_cargoship | 32 → 0 | 0 → 531 |
| dom | mp_cargoship | 151 → 71 | 0 → 552 |
| sd | mp_bloc | 29 → 26 | 61 → 437 |

All 210 pairs: 8674 kills to 8335, 4606 stalled bot-ticks to 5460, so
it stays off.

- mp_killhouse `koth` seed 1 runs clean under it (0 stalled), seed 2
  stalls 1752 at the crates. INFERRED (the stall lines).
- On mp_cargoship the CoD4 walks prove none of the deck's jump edges
  up 28 to 36 that the CoD1 graph had and live bots take, so the
  attackers' plan goes over the rail by the mantle edges down to the
  dock at 16, where each follower re-proof (`Follower::flings`,
  `backs_short`, CoD1 walks of a drop) avoids the edge and the next
  plan takes its neighbour: `sd` ends every round on time with no kill.
  Exempting mantle edges from those re-proofs sent the bots over the
  rail and stalled them on the dock instead (`sd` mp_cargoship 2991
  stalled bot-ticks over the two seeds). INFERRED (the follower's path
  per tick).
- The walk reads a CoD4 body back into CoD1's shape; a mantle the CoD4
  body starts on a pinned jump moves it along the climb's scripted
  path, which the walk's arrival and fall tests were not written for.
  That is the likeliest reason a CoD4 walk proves fewer of the jumps a
  live bot takes. Not looked into. INFERRED.

### 12.11 What the CoD4 walks missed, and the census again

Per rejected edge, the CoD4 walk's body (`Cod4Walk`, per tick) against
the live bot's path on the same edge (`bot_match` traces), 2026-10-27:

- The guess of 12.10 was half right. A pinned jump at a wall that
  offers the climb starts a mantle in 1.7's pmove, and the walk's body
  takes it: off mp_cargoship's deck at (2467, -384, 176) toward the rail
  top at 212, the jump became a `MANTLE_OVER` (`trans_index` 3) over the
  rail and down to the dock at 16, so the CoD1 graph's jump edge onto
  the rail went unproved. A live bot that jumps there mantles over too,
  so that edge was right to go. INFERRED (the walk's per-tick body).
- What broke `sd` and `dom` mp_cargoship was not the walk's graph: both
  graphs route the attackers over the rail by the same mantle edge,
  (2498, 384, 176) down to (2400, 390, 16), and the same 150 nodes. It
  was the bots' own pass, whose re-proofs ran under the CoD4 step once
  the graph carried it (`NavGraph::moving`). A mantle down deeper than a
  jump read as a drop to back down: the brain backed the bot into the
  rail (`BotView::run_off` false), where no mantle starts, and the
  creep's re-proof (`Follower::backs_short`) under 1.7's pmove never
  arrived, so the follower avoided each rail edge in turn. Under a game
  step a mantle edge down now comes to rest at its foot once and is
  taken facing the wall (`Follower::mantle_rests`, set only where the
  graph carries a step; `BotView::run_off` on a mantle edge). INFERRED
  (the follower's path per tick).
- The walk clipped the clip map alone; the live body also clips the
  linked `script_brushmodel`s. mp_cargoship's `pipe_shootable` (`*76`
  at (1300, 61, 104)) caps the head of a jump up the deck's step from
  (1345, 36, 52) to (1313, 36, 80): the walk's body landed on the step,
  live bots rose to 91 and fell back for a whole `war` or `sd` match.
  The walk now clips the map's `script_brushmodel`s where they stand at
  load (`bots::cod4::map_brushes`). VERIFIED (the entity and the walk
  with and without it); that the pipe is the cap is INFERRED.
- Left: mp_killhouse's crate jumps from (1279, 2496, 60) up 37 to
  (1310, 2556, 97). The CoD4 walk proves them with a running start
  aimed 8 units aside; a bot that comes to rest at the foot (the jump's
  wait, `Follower::jump_wait`) jumps from a standstill and falls back,
  and the follower cycles through four parallel edges. INFERRED.

The census of 12.9 again, default (CoD1's walks) against
`VC_NAV_COD4=1` with the three changes, 210 pairs, seeds 1 and 2,
`TICKS=6000`, 2026-10-27. VERIFIED (the runs):

| gametype | map | kills | stalled bot-ticks |
|---|---|---|---|
| koth | mp_killhouse | 123 → 136 | 1245 → 1088 |
| sab | mp_cargoship | 54 → 72 | 1052 → 0 |
| koth | mp_cargoship | 74 → 70 | 725 → 807 |
| koth | mp_crossfire | 71 → 78 | 301 → 0 |
| dom | mp_broadcast | 42 → 44 | 264 → 43 |
| war | mp_bloc | 85 → 96 | 166 → 498 |
| koth | mp_crash_snow | 80 → 84 | 129 → 13 |
| sd | mp_cargoship | 32 → 36 | 0 → 0 |
| dom | mp_cargoship | 151 → 176 | 0 → 0 |

All 210 pairs: 4606 stalled bot-ticks to 3255, 8674 kills to 8622, no
pair without a kill. The stalls fall by 29%, the kills by 0.6%, inside
what two seeds move a row (12.6), but not a win on both, so the
default stays CoD1's walks.

### 12.12 The kill gap over six seeds, killhouse's crate jumps

- mp_killhouse `koth`: per tick, a bot that lands from a jump onto the
  crate tops at z 60 carries 1.7's `PMF_JUMPING` with `pm_time` 1800,
  the jump penalty, and waits it out at the foot (`Follower::jump_wait`,
  `nav::jump_ready`). Once it is ready the plan has moved on to a drop
  off the crates, which it backs toward (`forwardmove` -127, `pm_flags`
  0x20) past the jump edge's top and off the ledge to 28, and the round
  starts again. INFERRED (each cmd and playerstate per tick). Two fixes
  measured on seeds 1 and 2, neither kept: a wait 48 units back from the
  foot for a running start (`koth` mp_killhouse 0/1088 stalled bot-ticks
  to 1362/0), and jump edges re-proved from rest under a game step
  (no change: the walk from rest lands them too). INFERRED (the runs).
- `war` mp_bloc's new stalls are two allies bots holding each other at
  (350, -5700, -24); `koth` mp_cargoship's are both teams on the stair
  at (2830, -100, 280) by the radio, as under CoD1's walks. INFERRED
  (the stall lines).

The census of 12.9 on seeds 1 to 6 (630 matches a setting), CoD1's
walks against `VC_NAV_COD4=1`, 2026-10-27. VERIFIED (the runs):

Over seeds 1 to 6, all: kills 25866 → 25969, stalled bot-ticks 12783 → 13159 (VERIFIED, measured; per-seed rows dropped).

- One setting's kills move by 50 a seed (standard deviation over the six
  seeds); the two settings' per-seed difference by 117. The walk's kills
  are 0.4% up, its stalls 3% up: the 12.11 result on seeds 1 and 2 was
  seed 1's luck. Most of the walk's stalls are `koth`: mp_killhouse 1526
  → 4157, mp_carentan 375 → 1824, mp_crash 274 → 1227, mp_bloc 5 → 954.
  `dom` mp_cargoship 554 → 0 and no pair without a kill.
- So `VC_NAV_COD4` stays off and the nav_build digests stand.

### 12.13 Holding the plan through the jump penalty, not kept

The reading of mp_killhouse `koth` (12.12) traced again per tick
on seed 1 with the follower's plan, its avoided edges and each reset's
call site, 2026-10-27. INFERRED (the trace):

- The plan does not move on by itself while the penalty runs. A bot
  that lands from `J296` (1247, 2524, 60) on the crate top holds still
  at the next node (1242, 2496, 60), `PMF_JUMPING` set; after 20 ticks
  beside a still teammate `mates_in_way` (`PARTS_MATES`, 12.7) plans it
  round him, and `give_way` steps it off. Elsewhere the wait reaches
  `STUCK_TICKS` (40) with no progress and the follower avoids the edge
  under way. Either new plan starts with a drop off the crate.
- What it waits for then fails. With the penalty spent, the jumps from
  the crate top up 37 to (1310, 2557, 97) and (1310, 2586, 97)
  (`J3298`, `J3299`) and from the floor at 28 up 32 to (1280, 2532, 60)
  (`J177`) rise beside the ledge and fall to 28: the follower's
  "fallen off the path" check avoids the edge, the next plan takes a
  parallel one, and the bot cycles. Allies slot 1 stalls there with no
  penalty and no teammate in play.

The change tried: on the CoD4 path only (`BotWorld::HOLDS_PENALTY`),
a bot on the ground under `jump_penalty` whose current or previous edge
is a jump counts no idle ticks toward `STUCK_TICKS`, is not held up by
a teammate for `mates_in_way` and gives no way. The census of 12.12
(630 matches a setting, CoD1's walks, `VC_NAV_COD4` off), 2026-10-27.
The base arm reproduced 12.12's default column to the match. VERIFIED
(the runs):

Over seeds 1 to 6, all: kills 25866 → 25837, stalled bot-ticks 12783 → 14617 (VERIFIED, measured; per-seed rows dropped).

- Kills hold: the per-seed difference's standard deviation is 29, the
  total 0.1% down. Stalls rise 14%. `koth` mp_killhouse 1526 → 2693
  (per seed 1009, 236, 0, 0, 281, 0 → 2088, 380, 0, 0, 225, 0);
  `koth` mp_cargoship 1478 → 829 and mp_crossfire 302 → 91 fall;
  `koth` mp_overgrown 0 → 726 and `dom` mp_showdown 14 → 716 rise.
  No pair without a kill.
- So the hold is not kept. Kept on its plan, the bot only waits longer
  before the same failed jump. The fix is in the jumps: edges up more
  than a step that a body at rest on 1.7's pmove does not land, here
  `J177`, `J3298`, `J3299` (12.9, 12.11's "Left"). INFERRED.

### 12.14 No leaps on CoD4's graph

Why the graph kept mp_killhouse's crate jumps of 12.13. INFERRED (each
edge's flags and walks in the graph, 2026-10-27):

- The edges up 37 from the crate tops at z 60 to (1310, 2557, 97) and
  (1310, 2586, 97) were leaps (`NavGraph::link_leaps`): proved from a
  standstill with a jump on the tick the body would lose the ground
  (`nav::lip_ahead`). A bot jumps a leap only when `BotView::lip` says
  the lip is ahead, and the driver fills it only on a route with pmove
  probes (`BotWorld::PMOVE_PROBES`), which CoD4's has off. So a CoD4 bot
  on a leap never jumps: it runs off the foot and lands on the floor at
  28, the fall that 12.13 read as a failed jump.
- Walked as plain jumps, the way a bot without the lip takes them (a
  pinned jump, `nav::PinJump`, `nav::as_a_bot`) and aimed at the top,
  all five leaps off the crate tops fall back to 28, under CoD1's walk
  and under `bots::cod4::Cod4Walk` alike. Aimed 8 units beside the top
  (`nav::SIDESTEP`), the one from (1242, 2496, 60) lands, and the build
  keeps it as a plain jump with that aim
  (`bots::cod4::tests::a_cod4_graph_keeps_no_leap_its_bots_cannot_jump`). The plain jumps onto the crate tops from
  the floor (J177's kind, up 32) land under both; they were not the
  cause.
- `NavGraph::reprove_jumps` skipped leaps, so no build ever tried them as
  plain jumps.

The change: a CoD4 graph clears its leaps after `link_leaps`, before
`reprove_jumps`, which walks each again as a plain jump edge and drops
those that fall (`bots::cod4::LIP`, `NavGraph::build_opts`,
`NavJob::start_opts`; its cache key carries "(no lip)"). Every CoD4 map
loses its leaps; the 21 maps' graphs lose 1 to 108 edges each
(mp_killhouse 23901 → 23890, mp_creek 56985 → 56877), no spawn leaves
its component. `build_with` and `start_with` pass the lip, so the CoD1,
1.5, UO and CoD2 graphs and their `nav_build` digests do not change.
VERIFIED (`nav_build GAME=cod4`, the census's graph lines, the 1.1 and
UO digests).

The census of 12.12 (630 matches a setting, CoD1's walks, `VC_NAV_COD4`
off), the same tip with and without the change, 2026-10-28. VERIFIED (the
runs):

Over seeds 1 to 6, all: kills 25363 → 25307, stalled bot-ticks 14182 → 11811 (VERIFIED, measured; per-seed rows dropped).

- Stalls fall 17%, kills by 0.2% (the per-seed difference's standard
  deviation is 45). `koth` mp_killhouse 1707 → 20 (per seed 644, 13, 0,
  1050, 0, 0 → 0, 13, 0, 7, 0, 0) with kills 418 → 444; `koth` mp_creek
  180 → 0 and mp_broadcast 221 → 104. No pair without a kill.
- Rises: `koth` mp_crash 403 → 727 and mp_crash_snow 905 → 958, all
  gametypes on mp_crash 301 and mp_crash_snow 284, mp_cargoship 99. Not
  traced. INFERRED.
- The base arm here differs from 12.13's before column (25866 kills,
  12783 stalls): the tip moved by CoD4 combat changes between the two,
  so this table compares two builds of the one tip.

### 12.15 A ladder's top, edges past the follower's reach, brush entities

The rises of 12.14 (`koth` mp_crash and mp_crash_snow, `war`
mp_cargoship) traced per tick with the follower's plan on seeds 1 to
6, `STALLS=1`, 2026-10-28. The tip reproduced 12.14's after column to
the match on the ten pairs run (`koth` mp_crash seeds 1, 4, 6,
mp_crash_snow 3 to 6, `war` mp_crash_snow 4, `sab` mp_crash_snow 6,
`war` mp_cargoship 3), so that column is the before here. VERIFIED (the
runs).

- The ladder at (-91, -890) on mp_crash and mp_crash_snow, foot at 122,
  roof at 272: a bot climbed to 278, its waypoint on the roof 5 under
  its feet and 60 to 126 across the face, and `Bot::steer` backed it
  down (a waypoint below on the rungs is backed onto unless it lies
  within half a body across, 11.4); at 200 the waypoint was above again
  and it climbed. Up and down for the rest of a match (229 stalled
  bot-ticks in `war` mp_crash_snow seed 4, more in `koth` and `sab`).
  INFERRED (the follower's path per tick). 1.7's `PM_LadderMove` is
  CoD2's: the climb is the forward key times the view's pitch, the
  strafe along the face, no flat move into it
  (`vcod_cod4::pmove::ladder::ladder_move`, cod4-pmove.md 7), so the
  rungs let go only past their top. INFERRED. The CoD4 view
  now sets `BotView::tops_out`, as CoD2's does: a head across the face
  is climbed on to.
- mp_crash's roof at (64, -643, 313), reached by a mantle from 272:
  `NavGraph::link_back` walked the mantle edges onto it back as jumps
  158 across to (160, -518, 304) and (-32, -518, 304). The follower
  reads a waypoint more than four grid steps off (128 at spacing 32) as
  a body knocked off its path and plans again, so a bot at the roof's
  node planned the jump, dropped the plan and planned it again, every
  tick, until the match ended (401 stalled bot-ticks, `koth` seed 4).
  INFERRED (the follower's path per tick). The 21 maps' graphs held 61
  such jumps (mp_convoy 13, mp_crash 12, mp_crash_snow 11, mp_carentan
  6, mp_broadcast 5, mp_crossfire and mp_killhouse 4, mp_showdown 3,
  mp_backlot 2, mp_citystreets 1) and 3 such walks (mp_bog, mp_convoy,
  mp_crossfire). VERIFIED (the graphs). A CoD4
  graph now drops every edge but a mantle that long, after
  `reprove_jumps` and before `link_pockets` (`NavGraph::drop_out_of_reach`,
  under `NavGraph::build_opts`' `lip` off, which only CoD4 sets);
  mp_crash's roof keeps its drops to 272.
- mp_cargoship's deck step at (1346, 36, 52): the jump to (1313, 36,
  80) under `pipe_shootable` of 12.11, rising to 91 and falling back.
  The bots' world clipped the clip map alone. It now clips the level's
  solid brush entities too, read once the level has run a frame, when
  the gametype has deleted the other gametypes' objects
  (`bots::cod4::BrushedClip`, `Cod4Level::brushes`, as
  `game::cod4::clip::Cod4Clip` clips a body's pmove). The graph's cache
  key carries the set. VERIFIED (the jump edge gone,
  `a_cod4_graph_clips_the_brush_entities_its_bots_clip`). mp_countdown's
  `war` graph drops from 8499 nodes to 7028: its exploder 500 debris
  (`*2` to `*22` round (-500, 2910)) and `missile06` stand solid at
  load, since `maps\mp\_load::setupExploders` hides and unsolids only
  the `exploder` and `exploderchunk` targetnames. VERIFIED (the stock
  script, the graph lines); that those nodes stood on or behind them is
  INFERRED. `nav_build` builds over the clip map alone, so its digests
  are not the live graphs on maps with brush entities.
- mp_vacant `sd` (cod4-map-sweep.md "Left" 1): 0 stalled bot-ticks on
  all six seeds, before and after. VERIFIED (the runs).

The census of 12.12 (630 matches a setting, CoD1's walks, `VC_NAV_COD4`
off), 12.14's after column against the three changes, 2026-10-28.
VERIFIED (the runs):

Over seeds 1 to 6, all: kills 25307 → 25531, stalled bot-ticks 11811 → 4272 (VERIFIED, measured; per-seed rows dropped).

- Stalls fall 64%, kills rise 0.9% (the per-seed difference's standard
  deviation is 57). By gametype, stalls: `war` 859 → 185, `sd` 949 →
  394, `koth` 5924 → 2844, `sab` 3530 → 366, `dom` 549 → 483. By map,
  all gametypes: mp_cargoship 4623 fewer (`sab` 3235 → 0, `war` 287 →
  0), mp_crash_snow 1051, mp_bog 854, mp_crash 452. No pair without a
  kill.
- Left: `koth` mp_cargoship 1229 (seed 3 833), `koth` mp_showdown 529 →
  935 (seed 5 769), `sab` mp_bloc seed 2 0 → 317. Not traced.
  INFERRED.

### 12.16 A pool left by a mantle, a stair's guard, a roof's parapet

12.15's three pairs traced per tick with the follower's plan,
`STALLS=1`, 2026-10-29. The tip reproduced 12.15's after column on all
three (833, 769, 317 stalled bot-ticks). VERIFIED (the runs).

- `sab` mp_bloc seed 2: an axis bot in the empty pool at (160, -5540,
  -24) with its goal at the bomb zone (-864, -6044, 0) planned nothing
  for 300 ticks: the graph held the pool's 132 nodes as a pocket with
  edges in (walks down off its rim at 24) and none out. VERIFIED (the
  graph). The rim is ringed by mantle strips 12 wide at (168, -5756),
  (168, -5310) and (378, -5536), tops at 28, which 1.7's pmove climbs
  from the floor (row 1, flags 4, 700 ms; `mantle_spots`) and the
  graph's probe climbs too from the pool's nodes. VERIFIED (the
  probe from those nodes). The probe ran once, before the flood off the
  mantles' tops had found the pool, so no node there was probed. A CoD4
  graph (`lip` off) now probes again off each flood's new nodes until a
  round finds none (`NavGraph::build_here`, `link_mantles`' `from`);
  mp_bloc's `sab` graph goes from 6082 nodes to 6039, 649 mantles to
  700, and the pool reaches the zone and back
  (`a_cod4_graph_mantles_out_of_mp_blocs_pool`). VERIFIED (the graph).
  A probe aimed at the nearest mantle face within 45 degrees, tried
  first, found no climb the straight one missed and moved `koth`
  mp_bloc's radio stand out of its zone (seeds 3 and 5, 0 to 1126 and
  392 stalled bot-ticks); not kept. INFERRED (the runs with it on and
  off).
- `koth` mp_showdown seed 5: allies bots on the roof at (708, 608, 192)
  planned the mantle edges over the parapet down to (709, 672, 50); the
  creep's re-proof (`Follower::backs_short`) under CoD1's walks never
  arrived down one, so the follower avoided each in turn, then the jump
  `(763, 512) -> (796, 512, 224)`, which fell back, and the roof held
  them for 600 ticks. INFERRED (the follower's path per tick). 12.11's
  rest-and-go on a mantle down (`Follower::mantle_rests`), set only under
  a game step, is now on for CoD4 under CoD1's walks too
  (`BotWorld::MANTLE_RESTS`).
- `koth` mp_cargoship seed 3: axis bots queued on the stair at (2820,
  -97, 296) to (2848, -103, 288) for the radio at (3032, 0, 367), each
  walk on proved under both CoD1's and 1.7's pmove from where they stood.
  VERIFIED (`walk_as` both ways). The way was held by a teammate
  guarding the zone from the stair's top at (2844, -65, 336), 40 over
  and 30 across: 50 in three dimensions, past `mates_in_way`'s 48, so
  no one was ever held up by him and nobody planned round. INFERRED
  (the stall lines' nearest still mate). On CoD4 a still teammate now
  counts by his distance across, within 64 up or down
  (`BotWorld::PARTS_MATES_FLAT`). With it off, the other two changes
  kept, `koth` mp_cargoship seeds 1, 3, 4, 5 and 6 stall 1606
  bot-ticks; with it on, 726. VERIFIED (the runs).

The census of 12.12 (630 matches a setting, CoD1's walks, `VC_NAV_COD4`
off), the same tip with and without the three changes, 2026-10-29.
VERIFIED (the runs):

Over seeds 1 to 6, all: kills 25559 → 25927, stalled bot-ticks 4359 → 2031 (VERIFIED, measured; per-seed rows dropped).

- Stalls fall 53%, kills rise 1.4% (the per-seed difference's standard
  deviation is 81). By gametype, stalls: `war` 185 → 232, `sd` 394 →
  730, `koth` 2906 → 712, `sab` 396 → 132, `dom` 478 → 225. The three
  pairs: `koth` mp_cargoship seed 3 833 → 0, `koth` mp_showdown seed 5
  769 → 0, `sab` mp_bloc seed 2 317 → 0. No pair without a kill.
- Left: `sd` mp_bog seed 5 5 → 632, one axis bot pacing round a still
  teammate at (4312, 338, -6) by the bomb zone; `koth` mp_showdown seed
  1 108 and mp_cargoship seed 5 49. Not traced. INFERRED.

### 12.17 Two guards a node apart, and the gap between their boxes

12.16's `sd` mp_bog seed 5 traced per tick with the follower's plan and
the live clip, 2026-10-29. VERIFIED (the runs).

- The axis bot in slot 1 was held up in front of two still teammates
  guarding the bomb zone, slot 3 at (4312, 338, -6) and slot 5 at (4239,
  345, -6), 73 apart. With `PARTS_MATES_FLAT` on it was held up by the
  near one (it was not before, at 58 in three dimensions), and
  `mates_in_way` blocked the node nearest each guard (484 and 870). The
  plan went round through node 1111 between them; the body pressed on
  slot 3's and slot 5's boxes there at (4282, 368, -6), 1.7's pmove
  clip naming each in turn (fraction 0.0001, entity 3 then 5), and
  paced back round when the hold ran out, 632 stalled bot-ticks.
  VERIFIED (the per-tick trace). On CoD4 the way round a still teammate
  now keeps off every node within a box's width (30) across of him on
  both axes and a step (18) up or down, not only the nearest
  (`BotWorld::PARTS_MATES_BOX`, `NavGraph::nodes_in_box`). Up to
  `MATE_RISE` (64) instead blocked a mantle's foot 57 under a guard on
  mp_crash's ledge at (1414, 208, 183): `war` seed 4 went 0 to 334;
  with the step, 0. VERIFIED (the runs).

The census of 12.16 (630 matches, CoD1's walks, `VC_NAV_COD4` off),
12.16's tip against it with the box, 2026-10-29. VERIFIED (the runs):

| gametype | kills | stalled bot-ticks |
|---|---|---|
| `war` | 5990 → 5955 | 232 → 156 |
| `sd` | 2371 → 2346 | 730 → 174 |
| `koth` | 6080 → 6125 | 712 → 650 |
| `sab` | 4241 → 4272 | 132 → 152 |
| `dom` | 7245 → 7214 | 225 → 295 |
| all | 25927 → 25912 | 2031 → 1427 |

- Stalls fall 30%; kills hold (-15, the per-seed difference's standard
  deviation 78). `war` and `sd` are under 12.15's 185 and 394 again.
  `sd` mp_bog seed 5 632 → 78; 12.16's three pairs stay at 0.
- `war`'s other rise in 12.16 (mp_countdown 6, mp_carentan 1,
  mp_showdown 4, 12 to 40 each) and `sd`'s (mp_bog 1, mp_bloc 2 and 6,
  25 to 40) were single episodes of one or two stall lines with no
  teammate in the way, gone or moved in this census. INFERRED (the
  stall lines).
- `dom` rises 70 over five matches of 40 to 86 each; not traced.

### 12.18 Jump edges on CoD4, and goals no body reaches, 2026-10-29

12.17's `dom` rise (225 → 295) traced per tick with the follower's plan,
`STALLS=1`, 2026-10-29. The tip reproduced 12.17's after column on every
`dom` and `koth` pair. VERIFIED (the runs).

- The rise is two pairs that moved, not one new spot: mp_pipeline seed 6
  (0 → 86) and mp_broadcast seed 2 (14 → 36), against mp_broadcast seeds
  1 and 5 (48 → 36, 26 → 0). VERIFIED (the census lines). Every `dom`
  stall of 12.17's census sits on a run of jump edges: mp_pipeline's
  crates at (-48, 3262), rising 38 and 31 to 32 and 63; mp_broadcast's
  crates at (-1250, 340), rising 35 and 38 to 208 and 182, and the step at
  (160, -709), 38; mp_showdown's stair at (1055, 630), 20 to 35 a step.
  There bots jumped from rest at the foot and fell back, waited out 1.7's
  jump slowdown at the foot (`Follower::jump_wait`; 1.2 to 1.8 s,
  cod4-pmove.md 10), queued behind a teammate waiting so, and missed a
  jump down onto a crate's top for the floor beside it. INFERRED (the
  follower's path per tick). On mp_pipeline the way round slot 4
  waiting at the crates' foot was planned by 12.17's box
  (`PARTS_MATES_BOX`) but the plan under way kept its node.
  INFERRED (the same trace). Of `koth`'s largest pairs mp_showdown seed
  1, mp_crash seed 3 and mp_carentan seed 6 sit on the same kind of
  edges, mp_crash seed 4 behind a still teammate and mp_overgrown seed 1
  at a mantle. INFERRED (the stall lines).
- Three ways round the jumps, measured, none kept. VERIFIED (the runs):

| arm | `war` | `sd` | `koth` | `sab` | `dom` | all | kills |
|---|---|---|---|---|---|---|---|
| tip (12.17) | 156 | 174 | 650 | 152 | 295 | 1427 | 25912 |
| every jump edge 64 longer in A* | 276 | 550 | 469 | 373 | 67 | 1735 | 25956 |
| jump edges rising 30 or more, 64 | 158 | 342 | 671 | 960 | 99 | 2230 | 25997 |

  A third, a jump under the slowdown taken once the cut jump
  (`vcod_cod4::pmove` `jump_curve`'s 39 over the curve) clears the
  edge's rise by 4, ran only on the 66 stalled `dom` and `koth` pairs:
  `dom` 295 → 30, `koth` 624 → 721 (mp_overgrown seed 4 0 → 277).
  Each arm takes 12.17's pairs to 0 or near it (all of `dom`'s, `koth`
  mp_crash seeds 3 and 4, mp_showdown seed 3) and opens others; the kill
  difference's per-seed standard deviation is 64 and 52.
- What the new stalls were, traced: the two A* arms' three worst pairs
  (`sab` mp_overgrown seed 5 765, `sab` mp_bog seed 6 247, `koth`
  mp_bloc seed 5 196) are one shape: a bot whose plan has run out (the
  path done, the goal its waypoint) circling 50 to 80 units off its
  goal for 20 to 50 s, the goal a spot no body
  stands on (the `sab` bomb lying at (872, -3841, -152) on mp_overgrown,
  at (2655, -158, -8) on mp_bog; `koth`'s radio at (-2112, -4470, 52) on
  mp_bloc, two bots circling it). The tip shows it too (`koth` mp_convoy
  seed 4 at (134, -451)). INFERRED (the follower's state on each stall
  line). The others: a bot on mp_crash's ladder at (-91, -890) again
  (`war` seed 4, 74), one held in the air with no speed at (1174, -141,
  45) on mp_cargoship (`war` seed 2, 165), teammates at an `sd` zone.
  INFERRED (the same).
- One match swings the census by hundreds (765 above), so a change that
  moves every trajectory needs more than six seeds to show a drop under
  1427. INFERRED (the per-seed totals: 108 to 560 and 20 to 925 against
  177 to 304).

### 12.19 Goals a plan's end leaves out of reach, the jump cost again, 2026-10-30

12.18's two unreachable goals traced per tick with the follower's plan
and the objective's trigger, on 12.18's tip with every jump edge 64
longer in A*, `STALLS=1`, 2026-10-29. VERIFIED (the runs).

- What the stock scripts take. VERIFIED (`maps/mp/gametypes/*.gsc` in
  1.7's `common_mp.ff`): the `sab` bomb is a carry object on its
  `sab_bomb_pickup_trig`, a `trigger_multiple` (`createCarryObject`,
  `allowCarry("any")`), taken by any live player of a team that may
  carry it whose `"trigger"` it hears (`carryObjectProxThink`), not
  while the bomb `isResetting`, while he throws a grenade, or while
  `canPickupObject` is off, which `pickupObjectDelay` holds until a
  player who just dropped it is 64 from where he dropped it. A drop
  moves the trigger to `playerPhysicsTrace` from 20 over the carrier to
  2000 under him (`setDropped`), and `pickupTimeout` sends it home after
  `autoResetTime` (60 s, `sab.gsc` `onPickup`). The `koth` radio is a
  use object on its `radiotrigger`, a `trigger_multiple`, counting a
  player only while he `isTouching` it (12.1). Neither accepts a body
  near the trigger: the bomb wants the touch, a zone the body inside the
  volume, which `inside_site` already tests (12.4).
- `sab` mp_bog seed 6 (247 stalled bot-ticks): an allies bot with the
  bomb, its goal the axis zone's stand at (2656, -158, -8) inside
  `sab_bomb_axis` (`*46`, x 2543..2705 here), paced at (2725..2790,
  -260..-130) for 14 s, east of the zone. Its follower had counted the
  plan walked at (2762, -75) (`Follower::arrival`) and the bot ran at
  the stand over the mantle edge at the zone's east side (1968 to 1214,
  (2720, -96) to (2656, -120)) it never climbed, back and forth into
  the crates. INFERRED (the follower's state per tick).
- `koth` mp_bloc seed 5 (196): the radio's `radiotrigger` (`*19` at
  (-1977, -4462, 116), bounds x -2085..-1869, y -4647..-4277) holds no
  node of the graph, so its stand (`NavGraph::zone_stand`'s fallback)
  is node 5599 at (-2112, -4470, 52), 27 west of the trigger. Two allies
  bots walked out their plans there and circled the stand, each pushed
  off it by the other, for 11 s. VERIFIED (the zone's stand and
  `nav_build` `NEAR=` round the trigger); why the room's floor (z 52,
  open from x -1990 to -1830 under a 30-wide box) has no node is not
  traced.
- On CoD4 a bot whose follower walked out its plan to an objective's
  point (`Follower::walked_out`) and comes no 16 units nearer it in 3 s
  (`SHORT_TICKS`, `SHORT_GAIN`), the objective not reached, gives the
  point up for 15 s (`SHY_TICKS`): the objective reads as a guard round
  the point (`Bot::track_reach`, `BotWorld::GIVES_UP_GOALS`), held as
  any guard is, then it walks at the point again. A guard ring and a
  breach stand are never given up. With the jump cost on, mp_bog seed
  6 247 → 0 and mp_bloc seed 5 196 → 2. VERIFIED (the runs). The bomb
  is not taken any sooner by this; the plan's end short of a mantle and
  the radio room's missing nodes are left.

The census (630 matches a setting, CoD1's walks, `VC_NAV_COD4` off),
seeds 1 to 12 (1260 matches), 2026-10-29 and 30. The tip is 12.18's
(seeds 1 to 6 are 12.17's after column), the fix is the give-up, and
the third column the give-up with every jump edge 64 longer in A*
(`BotWorld::JUMP_COST`). VERIFIED (the runs):

| seed | kills tip → fix → +jump cost | stalled bot-ticks |
|---|---|---|
| 1 | 4402 → 4399 → 4333 | 212 → 212 → 108 |
| 2 | 4329 → 4328 → 4347 | 212 → 212 → 450 |
| 3 | 4251 → 4254 → 4304 | 276 → 276 → 71 |
| 4 | 4342 → 4344 → 4408 | 177 → 150 → 236 |
| 5 | 4263 → 4263 → 4304 | 246 → 246 → 116 |
| 6 | 4325 → 4325 → 4257 | 304 → 304 → 313 |
| 7 | 4314 → 4328 → 4411 | 987 → 968 → 157 |
| 8 | 4291 → 4291 → 4482 | 322 → 322 → 70 |
| 9 | 4359 → 4359 → 4343 | 338 → 338 → 318 |
| 10 | 4306 → 4306 → 4368 | 335 → 335 → 95 |
| 11 | 4221 → 4231 → 4411 | 225 → 225 → 139 |
| 12 | 4245 → 4245 → 4325 | 493 → 493 → 287 |
| all | 51648 → 51673 → 52293 | 4127 → 4081 → 2360 |

| gametype | kills tip → fix → +jump cost | stalled bot-ticks |
|---|---|---|
| `war` | 11797 → 11797 → 11970 | 282 → 282 → 440 |
| `sd` | 4673 → 4670 → 4788 | 1032 → 1032 → 881 |
| `koth` | 12211 → 12237 → 12323 | 1654 → 1608 → 815 |
| `sab` | 8530 → 8532 → 8678 | 399 → 399 → 157 |
| `dom` | 14437 → 14437 → 14534 | 760 → 760 → 67 |

- The give-up alone moves 9 of 1260 matches (`koth` mp_convoy seeds 4
  and 7, 27 and 19 → 0, the rest kills only): without the jump cost the
  unreachable goals are rare. Kept. INFERRED (the matches that moved).
- With the jump cost on top stalls fall 42% and kills rise 1.2%, 9 of
  12 seeds lower (the per-seed kill difference's standard deviation 77,
  the stall difference's 244). `dom` falls 760 → 67 and `koth` halves;
  `war` rises 282 → 440. VERIFIED (the runs). The earlier wins hold but
  one: `koth` mp_cargoship seed 3, mp_showdown seed 5 and `sab` mp_bloc
  seed 2 stay 0, `sd` mp_bog seed 5 is 92 (78 on the tip), and `war`
  mp_crash seed 4 goes 0 → 74: an axis bot on the ladder at (-91, -884,
  192..237) whose plan, made on the rungs, starts at (-31, -868, 272)
  off the ladder's top at (-63, -895, 319), so it climbs and slides on
  the rungs for 4 s. INFERRED (the follower's path per tick; 12.18 saw
  the same bot under this arm). So the jump cost stays 0 until that
  ladder plan is fixed; the switch is in. The largest stalls left with
  it on: `sd` mp_crash seed 2 234, `war` mp_cargoship seed 2 165 (12.18's
  body held in the air at (1174, -141, 45)), `koth` mp_crash_snow seed
  12 155. Not traced.
